//! Background filesystem work with latest reads and updates, plus writes per destination.
//!
//! This is not a queue that guarantees every submission will run. See [`IO`] for replacement
//! and shutdown policies.

use crate::dir::Listing;
use image::RgbaImage;
use std::{
    any::Any,
    collections::VecDeque,
    fmt,
    fs::File,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// Work kinds with snapshot writes separated by destination path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Open an image or directory.
    Open,
    /// List a directory.
    List,
    /// Write an export. Coalesces by destination path.
    Export,
    /// Replace a text file under its lock. Coalesces across paths, so each edit carries every
    /// unsaved change.
    Update,
    /// Write a directory snapshot, coalescing per directory.
    Save,
    /// Read an owned value, keeping only the newest request.
    Load,
}

/// Replacement text computed from the current file contents while its lock is held.
pub trait Edit: Send {
    /// Returns the new text for the current contents, which are `None` when the file is missing.
    ///
    /// # Errors
    /// Returns an error if the edit cannot produce replacement text.
    fn apply(&self, text: Option<String>) -> io::Result<String>;
}

/// Owned filesystem work for the shared thread.
///
/// [`IO::submit`] replaces queued work of the same variant. Export and Save coalesce per path.
pub enum Job {
    /// Decode an image, or list it when the path is a directory.
    Open {
        /// Entered path, with optional home expansion.
        path: PathBuf,
        /// Base directory for relative paths.
        dir: Option<PathBuf>,
        /// Protected export destinations that must never be opened as the source.
        protect: Vec<PathBuf>,
    },
    /// List the given directory.
    List(PathBuf),
    /// Runs an encoder into a temporary file and replaces the destination only on success.
    ///
    /// A newer Export replaces a queued Export for the same destination path.
    Export {
        /// Export destination.
        path: PathBuf,
        /// Encoder carrying its owned output snapshot.
        // Archive formats may need Seek, and the file stays the single owner of the temporary path.
        #[allow(clippy::type_complexity)]
        write: Box<dyn FnOnce(&mut File) -> io::Result<()> + Send>,
    },
    /// Rewrite a text file from its current contents under an advisory lock.
    ///
    /// A newer Update replaces a queued Update even for a different file. Each edit must include
    /// every unsaved change for the destination; do not submit incremental edits assuming earlier
    /// submissions will run. Use [`crate::atomic::update`] when every edit must be applied.
    Update {
        /// File to replace.
        path: PathBuf,
        /// Longest wait for the lock.
        wait: Duration,
        /// Computes the replacement text, carrying all changes not yet confirmed saved.
        edit: Box<dyn Edit>,
    },
    /// Writes an owned snapshot on the worker, coalescing per directory.
    Save {
        /// Destination directory.
        dir: PathBuf,
        /// Writer carrying its snapshot.
        #[allow(clippy::type_complexity)]
        write: Box<dyn FnOnce(&Path) -> io::Result<()> + Send>,
    },
    /// Reads an application value on the worker, keeping the newest request.
    Load {
        /// Path passed to the reader.
        path: PathBuf,
        /// Reader carrying any application context.
        #[allow(clippy::type_complexity)]
        read: Box<dyn FnOnce(&Path) -> io::Result<Box<dyn Any + Send>> + Send>,
    },
    /// Trigger a panic to verify worker recovery.
    #[cfg(test)]
    Panic(PathBuf),
    /// Hold the worker so tests can queue work deterministically.
    #[cfg(test)]
    Pause(Sender<()>, Receiver<()>),
}

impl Job {
    /// Returns the kind used for coalescing.
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::Open { .. } => Kind::Open,
            Self::List(_) => Kind::List,
            Self::Export { .. } => Kind::Export,
            Self::Update { .. } => Kind::Update,
            Self::Save { .. } => Kind::Save,
            Self::Load { .. } => Kind::Load,
            #[cfg(test)]
            Self::Panic(_) => Kind::Open,
            #[cfg(test)]
            Self::Pause(..) => Kind::List,
        }
    }

    // TODO: Kind, op and path are derived again in `Done`, `Pending` and `failure`. Store them as
    //       explicit `Done` fields with `execute` returning the resolved path, and move the test
    //       only Panic and Pause jobs behind an injected executor.
    fn context(&self) -> (Op, PathBuf) {
        match self {
            Self::Open { path, .. } => (Op::Decode, path.clone()),
            Self::List(path) => (Op::List, path.clone()),
            Self::Export { path, .. } => (Op::Encode, path.clone()),
            Self::Update { path, .. } => (Op::Save, path.clone()),
            Self::Save { dir, .. } => (Op::Save, dir.clone()),
            Self::Load { path, .. } => (Op::Decode, path.clone()),
            #[cfg(test)]
            Self::Panic(path) => (Op::Decode, path.clone()),
            #[cfg(test)]
            Self::Pause(..) => (Op::List, PathBuf::new()),
        }
    }
}

/// Result of one job, carrying its work kind in the variant.
#[derive(Debug)]
pub enum Outcome {
    /// An image or directory was opened.
    Opened(Result<Opened, IoError>),
    /// A directory was listed.
    Listed(Result<Listing, IoError>),
    /// A file was exported.
    Exported(Result<PathBuf, IoError>),
    /// A text file was replaced.
    Updated(Result<Updated, IoError>),
    /// A directory snapshot was written.
    Saved(Result<PathBuf, IoError>),
    /// An application value was read.
    Loaded(Result<Output, IoError>),
}

/// Owned reader result with the path retained for diagnostics.
pub struct Output {
    /// Path given to the reader.
    pub path: PathBuf,
    /// Application value to downcast at the caller.
    pub value: Box<dyn Any + Send>,
}

impl fmt::Debug for Output {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Output")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// The file replaced by an Update job.
#[derive(Debug)]
pub struct Updated {
    /// Path given to the job.
    pub path: PathBuf,
    /// Text now stored in the file.
    pub text: String,
}

/// The image or directory selected by an Open job.
#[derive(Debug)]
pub enum Opened {
    /// A decoded and bounded source image.
    Image {
        /// Resolved source path.
        path: PathBuf,
        /// RGBA pixels after shrinking.
        image: RgbaImage,
    },
    /// A directory selected instead of an image.
    Dir(Listing),
}

/// A completed job and its measured duration.
#[derive(Debug)]
pub struct Done {
    /// Submission id shared across all kinds, used to drop superseded results.
    pub(crate) id: u64,
    /// Caller value given to `submit`, returned unchanged.
    pub tag: u64,
    /// Measured duration of the work or wait before stopping.
    pub elapsed: Duration,
    /// Typed result of the work.
    pub outcome: Outcome,
}

impl Done {
    /// Builds a result outside the worker, as tests of callers need.
    pub fn new(tag: u64, elapsed: Duration, outcome: Outcome) -> Self {
        Self {
            id: 0,
            tag,
            elapsed,
            outcome,
        }
    }

    /// Returns the kind carried by the outcome.
    pub(crate) fn kind(&self) -> Kind {
        match self.outcome {
            Outcome::Opened(_) => Kind::Open,
            Outcome::Listed(_) => Kind::List,
            Outcome::Exported(_) => Kind::Export,
            Outcome::Updated(_) => Kind::Update,
            Outcome::Saved(_) => Kind::Save,
            Outcome::Loaded(_) => Kind::Load,
        }
    }

    /// Returns the operation to use in timing logs.
    pub fn op(&self) -> Op {
        self.context().0
    }

    /// Returns the path used by the operation.
    pub fn path(&self) -> &Path {
        self.context().1
    }

    fn context(&self) -> (Op, &Path) {
        match &self.outcome {
            Outcome::Opened(Ok(Opened::Image { path, .. })) => (Op::Decode, path),
            Outcome::Opened(Ok(Opened::Dir(listing))) | Outcome::Listed(Ok(listing)) => {
                (Op::List, &listing.dir)
            }
            Outcome::Exported(Ok(path)) => (Op::Encode, path),
            Outcome::Saved(Ok(path)) => (Op::Save, path),
            Outcome::Loaded(Ok(output)) => (Op::Decode, &output.path),
            Outcome::Updated(Ok(updated)) => (Op::Save, &updated.path),
            Outcome::Opened(Err(error))
            | Outcome::Listed(Err(error))
            | Outcome::Exported(Err(error))
            | Outcome::Saved(Err(error))
            | Outcome::Loaded(Err(error))
            | Outcome::Updated(Err(error)) => (error.op, &error.path),
        }
    }
}

/// Operation names used in timing logs and error notices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Decode an image.
    Decode,
    /// List a directory.
    List,
    /// Run an export encoder.
    Encode,
    /// Replace a text file.
    Save,
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Decode => "decode",
            Self::List => "list",
            Self::Encode => "encode",
            Self::Save => "save",
        })
    }
}

/// Filesystem failure with its operation and path retained.
#[derive(Debug)]
pub struct IoError {
    /// Operation that failed.
    pub op: Op,
    /// Path used by the failed operation.
    pub path: PathBuf,
    /// Typed reason for the failure.
    pub cause: Cause,
}

/// Failure from filesystem work or worker lifecycle.
#[derive(Debug)]
pub enum Cause {
    /// Filesystem operation failed.
    Io(io::Error),
    /// Image decoding failed.
    Decode(image::ImageError),
    /// The protected export destination resolves to this source path, which must not be overwritten.
    IsSource(PathBuf),
    /// A job panicked, with its message when available.
    Panicked(String),
    /// The worker stopped before executing the job.
    Stopped,
    /// The operation was still pending at the shutdown deadline.
    TimedOut,
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::Decode(error) => error.fmt(f),
            Self::IsSource(path) => write!(
                f,
                "{} is the source image and cannot be overwritten",
                path.display()
            ),
            Self::Panicked(message) => write!(f, "IO job panicked: {message}"),
            Self::Stopped => f.write_str("skipped because the IO worker shut down"),
            Self::TimedOut => f.write_str("operation did not finish before quitting"),
        }
    }
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if matches!(self.cause, Cause::IsSource(_)) {
            return write!(f, "open: {}", self.cause);
        }
        if matches!(self.cause, Cause::Stopped) {
            return write!(f, "{}: {}: {}", self.op, self.path.display(), self.cause);
        }
        match self.op {
            Op::Decode => write!(
                f,
                "open: Could not load {}: {}",
                self.path.display(),
                self.cause
            ),
            Op::List => write!(f, "open: {}: {}", self.path.display(), self.cause),
            Op::Encode => write!(f, "PNG {}: {}", self.path.display(), self.cause),
            Op::Save => write!(f, "save {}: {}", self.path.display(), self.cause),
        }
    }
}

impl std::error::Error for IoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.cause {
            Cause::Io(error) => Some(error),
            Cause::Decode(error) => Some(error),
            _ => None,
        }
    }
}

struct Pending {
    kind: Kind,
    id: u64,
    tag: u64,
    op: Op,
    path: PathBuf,
    submitted: Instant,
}

impl Pending {
    fn fail(&self, kind: Kind, cause: Cause) -> Done {
        failure(
            self.id,
            self.tag,
            kind,
            self.submitted.elapsed(),
            self.op,
            self.path.clone(),
            cause,
        )
    }
}

/// Thread name for process panic hooks that need to recognize the IO worker.
pub const WORKER_NAME: &str = "rthid-io";

/// Longest wait for the worker when the queue shuts down.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

/// Panic payload text for a process panic hook that recognizes the IO worker.
/// Returns "unknown panic" for payloads other than `String` or `&str`.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic")
}

struct Queued {
    slots: [Option<(u64, u64, Job)>; 6],
    exports: Vec<(PathBuf, (u64, u64, Job))>,
}

struct Queue {
    queued: Mutex<Queued>,
    ready: Condvar,
}

/// One filesystem thread with an independent newest result for each work kind.
///
/// Open, Load, List and Update each have one queued slot. Export and Save have slots per path.
/// A submission replaces queued work in its slot, and [`Self::poll`] discards superseded results.
/// Work already running may still complete its filesystem changes.
///
/// Dropping this value can block for up to five seconds. Like [`Self::finish`], it skips queued
/// reads and updates, attempts queued exports and snapshots, and detaches a worker still running at
/// the deadline. Drop discards all results; call `finish` when the caller needs to inspect them.
/// Keep unsaved configuration changes until a matching successful result, then synchronously
/// save any that remain with [`crate::atomic::update`] after `finish`.
#[allow(clippy::upper_case_acronyms)]
pub struct IO {
    jobs: Arc<Queue>,
    results: Receiver<Done>,
    local: VecDeque<Done>,
    pending: [Option<Pending>; 6],
    export_pending: Vec<Pending>,
    next_id: u64,
    stopping: Arc<AtomicBool>,
    abandoned: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    // Disconnects when the worker exits, including by panic.
    exited: Receiver<()>,
}

const KINDS: [Kind; 6] = [
    Kind::Open,
    Kind::List,
    Kind::Export,
    Kind::Update,
    Kind::Save,
    Kind::Load,
];

impl IO {
    /// Starts the worker. `home` expands `~` in Open paths without reading the environment.
    ///
    /// # Errors
    /// Returns an error if the worker thread cannot be started.
    pub fn new(home: Option<PathBuf>) -> io::Result<Self> {
        let jobs = Arc::new(Queue {
            queued: Mutex::new(Queued {
                slots: [None, None, None, None, None, None],
                exports: Vec::new(),
            }),
            ready: Condvar::new(),
        });
        let queued = Arc::clone(&jobs);
        let (finished, results) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let abandoned = Arc::new(AtomicBool::new(false));
        let abandon = Arc::clone(&abandoned);
        let (alive, exited) = mpsc::channel();
        let thread = thread::Builder::new()
            .name(WORKER_NAME.into())
            .spawn(move || {
                let _alive: Sender<()> = alive;
                run(queued, finished, home, stop, abandon)
            })?;
        Ok(Self {
            jobs,
            results,
            local: VecDeque::new(),
            pending: [None, None, None, None, None, None],
            export_pending: Vec::new(),
            next_id: 0,
            stopping,
            abandoned,
            thread: Some(thread),
            exited,
        })
    }

    /// Queues work without waiting. The result echoes `tag` so callers can tell who asked for it.
    ///
    /// Replaces queued work of the same kind. Export and Save replace only the same destination.
    /// Superseded jobs have no result; already running work is not cancelled, but its result is
    /// discarded if superseded. Update jobs must carry all unsaved changes.
    pub fn submit(&mut self, job: Job, tag: u64) {
        self.enqueue(job, tag);
    }

    // Returns the id, which increases across kinds.
    fn enqueue(&mut self, job: Job, tag: u64) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let kind = job.kind();
        let (op, path) = job.context();
        let pending = Pending {
            kind,
            id,
            tag,
            op,
            path: path.clone(),
            submitted: Instant::now(),
        };
        let stopped = self
            .thread
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
            .then(|| pending.fail(kind, Cause::Stopped));
        if matches!(kind, Kind::Export | Kind::Save) {
            if let Some(slot) = self
                .export_pending
                .iter_mut()
                .find(|slot| slot.path == path && slot.kind == kind)
            {
                *slot = pending;
            } else {
                self.export_pending.push(pending);
            }
        } else {
            self.pending[kind as usize] = Some(pending);
        }
        if let Some(stopped) = stopped {
            self.local.push_back(stopped);
        } else {
            let mut queued = self.jobs.queued.lock().unwrap();
            if matches!(kind, Kind::Export | Kind::Save) {
                if let Some(slot) = queued
                    .exports
                    .iter_mut()
                    .find(|slot| slot.0 == path && slot.1.2.kind() == kind)
                {
                    slot.1 = (id, tag, job);
                } else {
                    queued.exports.push((path, (id, tag, job)));
                }
            } else {
                queued.slots[kind as usize] = Some((id, tag, job));
            }
            self.jobs.ready.notify_one();
        }
        id
    }

    /// Returns a newest result once, discarding completions replaced by later submissions.
    pub fn poll(&mut self) -> Option<Done> {
        loop {
            let done = if let Some(done) = self.local.pop_front() {
                done
            } else {
                match self.results.try_recv() {
                    Ok(done) => done,
                    Err(mpsc::TryRecvError::Empty) => return None,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let regular = KINDS.into_iter().find_map(|kind| {
                            self.pending[kind as usize]
                                .take()
                                .map(|pending| pending.fail(kind, Cause::Stopped))
                        });
                        return regular.or_else(|| {
                            (!self.export_pending.is_empty()).then(|| {
                                let pending = self.export_pending.remove(0);
                                pending.fail(pending.kind, Cause::Stopped)
                            })
                        });
                    }
                }
            };
            if matches!(done.kind(), Kind::Export | Kind::Save) {
                if let Some(index) = self
                    .export_pending
                    .iter()
                    .position(|pending| pending.id == done.id)
                {
                    self.export_pending.remove(index);
                    return Some(done);
                }
                continue;
            }
            let pending = &mut self.pending[done.kind() as usize];
            if pending
                .as_ref()
                .is_some_and(|pending| pending.id == done.id)
            {
                *pending = None;
                return Some(done);
            }
        }
    }

    /// Whether the newest id of any kind has not yet been polled.
    pub fn busy(&self) -> bool {
        self.pending.iter().any(Option::is_some) || !self.export_pending.is_empty()
    }

    /// Skips queued reads and updates and waits up to five seconds for queued exports and snapshots.
    /// Returns available results and detaches a worker still running at the deadline.
    ///
    /// Queued jobs may be skipped at shutdown. Collect reads with [`Self::poll`] before
    /// calling `finish`. A running operation may finish during the wait. Skipped jobs return
    /// [`Cause::Stopped`]. Jobs still pending at the deadline return [`Cause::TimedOut`].
    /// A timeout does not cancel an operation already running. Inspect matching Update results,
    /// then synchronously save remaining unsaved changes with [`crate::atomic::update`].
    /// Update jobs still require the caller to save unconfirmed configuration changes.
    pub fn finish(self) -> Vec<Done> {
        self.finish_timeout(SHUTDOWN_WAIT)
    }

    fn finish_timeout(mut self, timeout: Duration) -> Vec<Done> {
        self.stop_timeout(timeout);
        let mut done: Vec<_> = std::iter::from_fn(|| self.poll()).collect();
        for kind in KINDS {
            if let Some(pending) = self.pending[kind as usize].take() {
                done.push(pending.fail(kind, Cause::TimedOut));
            }
        }
        done.extend(
            self.export_pending
                .drain(..)
                .map(|pending| pending.fail(pending.kind, Cause::TimedOut)),
        );
        done
    }

    fn stop_timeout(&mut self, timeout: Duration) {
        self.stopping.store(true, Ordering::SeqCst);
        // Hold the mutex while notifying so an idle worker cannot miss shutdown.
        let slots = self.jobs.queued.lock().unwrap();
        self.jobs.ready.notify_one();
        drop(slots);
        // A worker still running at the deadline is detached.
        if let Some(thread) = self.thread.take() {
            match self.exited.recv_timeout(timeout) {
                Err(RecvTimeoutError::Disconnected) => {
                    let _ = thread.join();
                }
                Err(RecvTimeoutError::Timeout) => self.abandoned.store(true, Ordering::SeqCst),
                Ok(()) => {}
            }
        }
    }
}

impl Drop for IO {
    fn drop(&mut self) {
        self.stop_timeout(SHUTDOWN_WAIT);
    }
}

fn failure(
    id: u64,
    tag: u64,
    kind: Kind,
    elapsed: Duration,
    op: Op,
    path: PathBuf,
    cause: Cause,
) -> Done {
    let error = IoError { op, path, cause };
    Done {
        id,
        tag,
        elapsed,
        outcome: match kind {
            Kind::Open => Outcome::Opened(Err(error)),
            Kind::List => Outcome::Listed(Err(error)),
            Kind::Export => Outcome::Exported(Err(error)),
            Kind::Update => Outcome::Updated(Err(error)),
            Kind::Save => Outcome::Saved(Err(error)),
            Kind::Load => Outcome::Loaded(Err(error)),
        },
    }
}

// TODO: One thread runs every kind, so a listing or open waits behind a running export.
//       Give each kind its own thread on the same slots, or at least split Export off.
fn run(
    queued: Arc<Queue>,
    finished: Sender<Done>,
    home: Option<PathBuf>,
    stop: Arc<AtomicBool>,
    abandoned: Arc<AtomicBool>,
) {
    loop {
        let (id, tag, job) = {
            let mut queued_jobs = queued.queued.lock().unwrap();
            loop {
                let regular = queued_jobs
                    .slots
                    .iter()
                    .enumerate()
                    .filter_map(|(index, job)| job.as_ref().map(|(id, ..)| (index, *id)))
                    .min_by_key(|(_, id)| *id);
                let export = queued_jobs
                    .exports
                    .iter()
                    .enumerate()
                    .map(|(index, (_, (id, ..)))| (index, *id))
                    .min_by_key(|(_, id)| *id);
                match (regular, export) {
                    (Some((index, id)), Some((_, export_id))) if id <= export_id => {
                        break queued_jobs.slots[index].take().unwrap();
                    }
                    (Some((index, _)), None) => break queued_jobs.slots[index].take().unwrap(),
                    (_, Some((index, _))) => break queued_jobs.exports.remove(index).1,
                    (None, None) => {}
                }
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                queued_jobs = queued.ready.wait(queued_jobs).unwrap();
            }
        };
        let kind = job.kind();
        let (mut op, mut path) = job.context();
        let started = Instant::now();
        let result = catch_unwind(AssertUnwindSafe(|| {
            if stop.load(Ordering::SeqCst) && !matches!(kind, Kind::Export | Kind::Save) {
                return Err(Cause::Stopped);
            }
            execute(job, home.as_deref(), &mut op, &mut path, &abandoned)
        }));
        let result = result
            .unwrap_or_else(|payload| Err(Cause::Panicked(panic_message(&*payload).to_owned())));
        let outcome = match result {
            Ok(outcome) => Done {
                id,
                tag,
                elapsed: started.elapsed(),
                outcome,
            },
            Err(cause) => failure(id, tag, kind, started.elapsed(), op, path, cause),
        };
        if finished.send(outcome).is_err() {
            break;
        }
    }
}

fn listing(path: &Path) -> Result<Listing, Cause> {
    crate::dir::list_dir(path).map_err(Cause::Io)
}

fn execute(
    job: Job,
    home: Option<&Path>,
    op: &mut Op,
    path: &mut PathBuf,
    abandoned: &AtomicBool,
) -> Result<Outcome, Cause> {
    match job {
        Job::Open { dir, protect, .. } => {
            if let Some(text) = path.to_str() {
                *path = crate::path::expand_tilde(text, home);
            }
            if path.is_relative()
                && let Some(dir) = dir
            {
                *path = dir.join(&path);
            }
            if path.is_dir() {
                *op = Op::List;
                return listing(path).map(|listing| Outcome::Opened(Ok(Opened::Dir(listing))));
            }
            crate::image::check_not_source(path, &protect)?;
            let image = crate::image::load_image(path).map_err(|error| match error {
                image::ImageError::IoError(error) => Cause::Io(error),
                error => Cause::Decode(error),
            })?;
            Ok(Outcome::Opened(Ok(Opened::Image {
                path: path.clone(),
                image,
            })))
        }
        Job::Save { write, .. } => {
            write(path).map_err(Cause::Io)?;
            Ok(Outcome::Saved(Ok(path.clone())))
        }
        Job::Load { read, .. } => {
            let value = read(path).map_err(Cause::Io)?;
            Ok(Outcome::Loaded(Ok(Output {
                path: path.clone(),
                value,
            })))
        }
        Job::List(_) => listing(path).map(|listing| Outcome::Listed(Ok(listing))),
        Job::Export { write, .. } => {
            crate::atomic::replace(path, write).map_err(Cause::Io)?;
            Ok(Outcome::Exported(Ok(path.clone())))
        }
        Job::Update { wait, edit, .. } => {
            let text = crate::atomic::update(path, wait, |text| {
                // A detached save must not overwrite the final save after shutdown.
                if abandoned.load(Ordering::SeqCst) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "IO worker stopped",
                    ));
                }
                let text = edit.apply(text)?;
                Ok((text.clone(), text))
            })
            .map_err(Cause::Io)?;
            Ok(Outcome::Updated(Ok(Updated {
                path: path.clone(),
                text,
            })))
        }
        #[cfg(test)]
        Job::Panic(_) => panic!("test job panicked"),
        #[cfg(test)]
        Job::Pause(entered, resume) => {
            entered.send(()).unwrap();
            resume.recv().unwrap();
            listing(path).map(|listing| Outcome::Listed(Ok(listing)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn export(path: PathBuf, white: bool) -> Job {
        Job::Export {
            path,
            write: Box::new(move |file| file.write_all(&[u8::from(white)])),
        }
    }

    fn wait(worker: &mut IO) -> Done {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(done) = worker.poll() {
                return done;
            }
            assert!(Instant::now() < deadline, "IO job timed out");
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn pause(worker: &mut IO) -> Sender<()> {
        let (entered, started) = mpsc::channel();
        let (resume, paused) = mpsc::channel();
        worker.enqueue(Job::Pause(entered, paused), 0);
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        resume
    }

    fn open(path: PathBuf) -> Job {
        Job::Open {
            path,
            dir: None,
            protect: Vec::new(),
        }
    }

    #[test]
    fn saves_coalesce_per_directory_and_flush_independently_of_updates() {
        let temp = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let resume = pause(&mut worker);
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        for (dir, value, tag) in [(&first, 1, 10), (&second, 2, 11), (&first, 3, 12)] {
            worker.submit(
                Job::Save {
                    dir: dir.clone(),
                    write: Box::new(move |dir| {
                        std::fs::create_dir_all(dir)?;
                        std::fs::write(dir.join("value"), [value])
                    }),
                },
                tag,
            );
        }
        worker.submit(
            Job::Update {
                path: temp.path().join("config"),
                wait: Duration::ZERO,
                edit: Box::new(Append("config")),
            },
            13,
        );
        assert_eq!(worker.jobs.queued.lock().unwrap().exports.len(), 2);
        resume.send(()).unwrap();
        let results = worker.finish();
        assert!(
            results
                .iter()
                .any(|done| done.tag == 12 && matches!(done.outcome, Outcome::Saved(Ok(_))))
        );
        assert!(
            results
                .iter()
                .any(|done| done.tag == 11 && matches!(done.outcome, Outcome::Saved(Ok(_))))
        );
        assert!(!results.iter().any(|done| done.tag == 10));
        assert_eq!(std::fs::read(first.join("value")).unwrap(), [3]);
        assert_eq!(std::fs::read(second.join("value")).unwrap(), [2]);
    }

    #[test]
    fn loads_keep_only_the_newest_value() {
        let mut worker = IO::new(None).unwrap();
        let resume = pause(&mut worker);
        for value in 1..=3u64 {
            worker.submit(
                Job::Load {
                    path: PathBuf::from(format!("read-{value}")),
                    read: Box::new(move |_| Ok(Box::new(value))),
                },
                value,
            );
        }
        resume.send(()).unwrap();
        assert_eq!(wait(&mut worker).kind(), Kind::List);
        let done = wait(&mut worker);
        assert_eq!(done.tag, 3);
        assert_eq!(done.path(), Path::new("read-3"));
        let Outcome::Loaded(Ok(output)) = done.outcome else {
            panic!("expected reader output")
        };
        assert_eq!(*output.value.downcast::<u64>().unwrap(), 3);
        assert!(!worker.busy());
    }

    #[test]
    fn running_load_results_are_discarded_when_superseded() {
        let mut worker = IO::new(None).unwrap();
        let (entered, started) = mpsc::channel();
        let (resume, paused) = mpsc::channel();
        worker.submit(
            Job::Load {
                path: "old".into(),
                read: Box::new(move |_| {
                    entered.send(()).unwrap();
                    paused.recv().unwrap();
                    Ok(Box::new(1u64))
                }),
            },
            1,
        );
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        worker.submit(
            Job::Load {
                path: "new".into(),
                read: Box::new(|_| Ok(Box::new(2u64))),
            },
            2,
        );
        resume.send(()).unwrap();
        assert_eq!(wait(&mut worker).tag, 2);
        assert!(!worker.busy());
    }

    #[test]
    fn application_closure_panics_become_errors_and_worker_recovers() {
        let mut worker = IO::new(None).unwrap();
        worker.submit(
            Job::Save {
                dir: "save".into(),
                write: Box::new(|_| panic!("save panic")),
            },
            1,
        );
        let done = wait(&mut worker);
        assert_eq!(done.path(), Path::new("save"));
        assert!(
            matches!(done.outcome, Outcome::Saved(Err(IoError { cause: Cause::Panicked(message), .. })) if message == "save panic")
        );
        worker.submit(
            Job::Load {
                path: "load".into(),
                read: Box::new(|_| panic!("load panic")),
            },
            2,
        );
        assert!(
            matches!(wait(&mut worker).outcome, Outcome::Loaded(Err(IoError { cause: Cause::Panicked(message), .. })) if message == "load panic")
        );
        worker.submit(
            Job::Load {
                path: "ok".into(),
                read: Box::new(|_| Ok(Box::new(()))),
            },
            3,
        );
        assert!(matches!(wait(&mut worker).outcome, Outcome::Loaded(Ok(_))));
    }

    #[test]
    fn failed_export_preserves_destination_and_reports_encode_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export");
        std::fs::write(&path, b"old").unwrap();
        let mut worker = IO::new(None).unwrap();
        worker.submit(
            Job::Export {
                path: path.clone(),
                write: Box::new(|file| {
                    file.write_all(b"partial")?;
                    Err(io::Error::other("encoder failed"))
                }),
            },
            7,
        );
        let done = wait(&mut worker);
        assert_eq!(
            (done.tag, done.op(), done.path()),
            (7, Op::Encode, path.as_path())
        );
        assert!(matches!(done.outcome,
            Outcome::Exported(Err(IoError { cause: Cause::Io(error), .. }))
                if error.to_string() == "encoder failed"));
        assert_eq!(std::fs::read(path).unwrap(), b"old");
        assert!(worker.finish().is_empty());
    }

    #[test]
    fn exports_to_the_same_path_coalesce() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let resume = pause(&mut worker);
        let mut last = 0;
        for n in 0..100 {
            last = worker.enqueue(export(dir.path().join("preview.png"), n == 99), 0);
        }
        assert!(worker.busy());
        assert_eq!(worker.jobs.queued.lock().unwrap().exports.len(), 1);
        resume.send(()).unwrap();
        let paused = wait(&mut worker);
        assert_eq!(paused.kind(), Kind::List);
        let done = wait(&mut worker);
        assert_eq!(done.id, last);
        assert_eq!(done.kind(), Kind::Export);
        assert_eq!(done.op().to_string(), "encode");
        assert!(done.elapsed > Duration::ZERO);
        let path = done.path().to_path_buf();
        assert!(matches!(done.outcome, Outcome::Exported(Ok(_))));
        assert_eq!(std::fs::read(path).unwrap(), [1]);
        assert!(!worker.busy());
        assert!(worker.poll().is_none());
        assert!(worker.finish().is_empty());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn exports_to_different_paths_both_complete() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let resume = pause(&mut worker);
        let first = dir.path().join("preview.png");
        let second = dir.path().join("mass.svg");
        worker.enqueue(export(first.clone(), false), 0);
        worker.enqueue(export(second.clone(), true), 0);
        resume.send(()).unwrap();
        assert_eq!(wait(&mut worker).kind(), Kind::List);
        assert!(matches!(
            wait(&mut worker).outcome,
            Outcome::Exported(Ok(_))
        ));
        assert!(matches!(
            wait(&mut worker).outcome,
            Outcome::Exported(Ok(_))
        ));
        assert_eq!(std::fs::read(first).unwrap(), [0]);
        assert_eq!(std::fs::read(second).unwrap(), [1]);
    }

    struct Append(&'static str);

    impl Edit for Append {
        fn apply(&self, text: Option<String>) -> io::Result<String> {
            Ok(text.unwrap_or_default() + self.0)
        }
    }

    struct Fail;

    impl Edit for Fail {
        fn apply(&self, _: Option<String>) -> io::Result<String> {
            Err(io::Error::other("rejected"))
        }
    }

    #[test]
    fn update_reaching_the_lock_checks_abandonment() {
        for abandoned in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let mut path = dir.path().join("config");
            std::fs::write(&path, "current").unwrap();
            let job = Job::Update {
                path: path.clone(),
                wait: Duration::ZERO,
                edit: Box::new(Append("new")),
            };

            // The save passed the worker's initial stop check before shutdown.
            let result = execute(
                job,
                None,
                &mut Op::Save,
                &mut path,
                &AtomicBool::new(abandoned),
            );

            if abandoned {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "current");
                assert!(
                    matches!(result, Err(Cause::Io(error)) if error.kind() == io::ErrorKind::Interrupted)
                );
            } else {
                assert!(
                    matches!(result, Ok(Outcome::Updated(Ok(updated))) if updated.text == "currentnew")
                );
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "currentnew");
            }
        }
    }

    #[test]
    fn update_snapshots_replace_queued_updates_even_across_paths() {
        for different_paths in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let first = dir.path().join("first");
            let latest = if different_paths {
                dir.path().join("latest")
            } else {
                first.clone()
            };
            std::fs::write(&first, "base:").unwrap();
            std::fs::write(&latest, "base:").unwrap();
            let mut worker = IO::new(None).unwrap();
            let resume = pause(&mut worker);
            for (path, snapshot, tag) in [
                (first.clone(), "first", 1),
                (latest.clone(), "first+second", 2),
            ] {
                worker.submit(
                    Job::Update {
                        path,
                        wait: Duration::ZERO,
                        edit: Box::new(Append(snapshot)),
                    },
                    tag,
                );
            }
            resume.send(()).unwrap();
            assert_eq!(wait(&mut worker).kind(), Kind::List);
            let done = wait(&mut worker);
            assert_eq!((done.tag, done.path()), (2, latest.as_path()));
            assert_eq!(
                std::fs::read_to_string(&latest).unwrap(),
                "base:first+second"
            );
            if different_paths {
                assert_eq!(std::fs::read_to_string(&first).unwrap(), "base:");
            }
            assert!(!worker.busy());
            assert!(worker.finish().is_empty());
        }
    }

    #[test]
    fn updates_coalesce_and_return_the_stored_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config");
        let update = |edit: &'static str| Job::Update {
            path: path.clone(),
            wait: Duration::from_secs(1),
            edit: Box::new(Append(edit)),
        };
        let mut worker = IO::new(None).unwrap();
        let resume = pause(&mut worker);
        worker.enqueue(update("old"), 1);
        let latest = worker.enqueue(update("new"), 2);
        resume.send(()).unwrap();
        assert_eq!(wait(&mut worker).kind(), Kind::List);
        let done = wait(&mut worker);
        assert_eq!((done.id, done.tag), (latest, 2));
        assert_eq!((done.op(), done.path()), (Op::Save, path.as_path()));
        assert!(matches!(&done.outcome, Outcome::Updated(Ok(updated)) if updated.text == "new"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        std::fs::write(&path, "version = [").unwrap();
        worker.enqueue(
            Job::Update {
                path: path.clone(),
                wait: Duration::from_secs(1),
                edit: Box::new(Fail),
            },
            3,
        );
        assert!(matches!(
            wait(&mut worker).outcome,
            Outcome::Updated(Err(IoError { op: Op::Save, cause: Cause::Io(error), .. }))
                if error.to_string() == "rejected"
        ));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "version = [");
    }

    #[test]
    fn open_and_list_keep_separate_results_and_resolve_open_paths() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(Some(dir.path().to_path_buf())).unwrap();
        let resume = pause(&mut worker);
        let opened = worker.enqueue(open(PathBuf::from("~")), 7);
        let listed = worker.enqueue(Job::List(dir.path().to_path_buf()), 8);
        resume.send(()).unwrap();
        let done = wait(&mut worker);
        assert_eq!((done.id, done.tag), (opened, 7));
        assert_eq!(done.kind(), Kind::Open);
        assert_eq!(done.op().to_string(), "list");
        assert_eq!(done.path(), dir.path().canonicalize().unwrap());
        assert!(matches!(done.outcome, Outcome::Opened(Ok(Opened::Dir(_)))));
        let done = wait(&mut worker);
        assert_eq!((done.id, done.tag), (listed, 8));
        assert!(matches!(done.outcome, Outcome::Listed(Ok(_))));
        assert!(!worker.busy());

        let path = dir.path().join("source.png");
        RgbaImage::new(2048, 2).save(&path).unwrap();
        worker.enqueue(
            Job::Open {
                path: "source.png".into(),
                dir: Some(dir.path().to_path_buf()),
                protect: Vec::new(),
            },
            0,
        );
        let done = wait(&mut worker);
        assert_eq!(done.op(), Op::Decode);
        assert_eq!(done.path(), path);
        let Outcome::Opened(Ok(Opened::Image { image, .. })) = done.outcome else {
            panic!("expected decoded image");
        };
        assert_eq!(image.dimensions(), (1024, 1));
        worker.enqueue(
            Job::Open {
                path: path.clone(),
                dir: None,
                protect: vec![path],
            },
            0,
        );
        assert!(matches!(
            wait(&mut worker).outcome,
            Outcome::Opened(Err(IoError {
                cause: Cause::IsSource(_),
                ..
            }))
        ));
    }

    #[test]
    fn panic_is_reported_and_the_thread_runs_the_next_job() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let id = worker.enqueue(Job::Panic(dir.path().join("panic")), 0);
        let done = wait(&mut worker);
        assert_eq!(done.id, id);
        assert!(matches!(done.outcome, Outcome::Opened(Err(IoError {
            cause: Cause::Panicked(message), ..
        })) if message == "test job panicked"));
        let id = worker.enqueue(Job::List(dir.path().to_path_buf()), 0);
        let done = wait(&mut worker);
        assert_eq!(done.id, id);
        assert!(matches!(done.outcome, Outcome::Listed(Ok(_))));
    }

    #[test]
    fn finish_skips_queued_reads_and_flushes_each_newest_export() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let resume = pause(&mut worker);
        let opened = worker.enqueue(open(dir.path().to_path_buf()), 0);
        let listed = worker.enqueue(Job::List(dir.path().to_path_buf()), 0);
        let first = worker.enqueue(export(dir.path().join("old.png"), false), 0);
        let exported = worker.enqueue(export(dir.path().join("final.png"), true), 0);
        let stopping = Arc::clone(&worker.stopping);
        let release = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !stopping.load(Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "finish did not stop reads");
                thread::sleep(Duration::from_millis(2));
            }
            resume.send(()).unwrap();
        });
        let done = worker.finish();
        release.join().unwrap();
        assert_eq!(
            done.iter().map(|done| done.id).collect::<Vec<_>>(),
            [opened, listed, first, exported]
        );
        assert!(matches!(
            done[0].outcome,
            Outcome::Opened(Err(IoError {
                cause: Cause::Stopped,
                ..
            }))
        ));
        assert!(matches!(
            done[1].outcome,
            Outcome::Listed(Err(IoError {
                cause: Cause::Stopped,
                ..
            }))
        ));
        assert!(matches!(done[2].outcome, Outcome::Exported(Ok(_))));
        assert!(matches!(done[3].outcome, Outcome::Exported(Ok(_))));
        assert!(dir.path().join("final.png").exists());
        assert!(dir.path().join("old.png").exists());
    }

    #[test]
    fn stale_results_are_discarded_and_a_stopped_thread_still_returns_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let path = dir.path().join("preview.png");
        worker.enqueue(export(path.clone(), false), 0);
        let resume = pause(&mut worker);
        let latest = worker.enqueue(export(path, true), 0);
        // The earlier Export completed before Pause began and must still be discarded.
        assert!(worker.poll().is_none());
        resume.send(()).unwrap();
        assert_eq!(wait(&mut worker).kind(), Kind::List);
        assert_eq!(wait(&mut worker).id, latest);
        worker.stop_timeout(SHUTDOWN_WAIT);
        assert!(!worker.abandoned.load(Ordering::SeqCst));
        for _ in 0..2 {
            let id = worker.enqueue(open(dir.path().to_path_buf()), 3);
            assert!(worker.busy());
            let done = wait(&mut worker);
            assert_eq!((done.id, done.tag), (id, 3));
            assert!(matches!(
                done.outcome,
                Outcome::Opened(Err(IoError {
                    cause: Cause::Stopped,
                    ..
                }))
            ));
            assert!(!worker.busy());
        }
    }

    #[test]
    fn failed_join_returns_stopped_for_outstanding_ids() {
        let mut worker = IO::new(None).unwrap();
        worker.stop_timeout(SHUTDOWN_WAIT);
        let (finished, results) = mpsc::channel();
        let (release, paused) = mpsc::channel();
        let (alive, exited) = mpsc::channel::<()>();
        worker.results = results;
        worker.exited = exited;
        worker.thread = Some(thread::spawn(move || {
            let _alive = alive;
            paused.recv_timeout(Duration::from_secs(5)).unwrap();
            drop(finished);
            panic!("worker failed outside a job");
        }));
        let id = worker.enqueue(Job::List("missing".into()), 5);
        release.send(()).unwrap();
        let done = worker.finish();
        assert_eq!(done.len(), 1);
        assert_eq!((done[0].id, done[0].tag), (id, 5));
        assert!(matches!(
            done[0].outcome,
            Outcome::Listed(Err(IoError {
                cause: Cause::Stopped,
                ..
            }))
        ));
    }

    #[test]
    fn finish_detaches_a_blocked_worker_at_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let opened = worker.enqueue(open(dir.path().to_path_buf()), 0);
        let resume = pause(&mut worker);
        let exported = worker.enqueue(export("unused.png".into(), false), 6);
        let started = Instant::now();
        let abandoned = Arc::clone(&worker.abandoned);
        let done = worker.finish_timeout(Duration::from_millis(50));
        assert!(abandoned.load(Ordering::SeqCst));
        assert_eq!(done.len(), 3);
        assert_eq!(done[0].id, opened);
        assert!(matches!(done[0].outcome, Outcome::Opened(Ok(_))));
        assert_eq!((done[2].id, done[2].tag), (exported, 6));
        assert!(matches!(
            done[2].outcome,
            Outcome::Exported(Err(IoError {
                cause: Cause::TimedOut,
                ..
            }))
        ));
        assert!(matches!(
            done[1].outcome,
            Outcome::Listed(Err(IoError {
                cause: Cause::TimedOut,
                ..
            }))
        ));
        assert!(started.elapsed() < Duration::from_secs(1));
        // Dropping the receiver makes the worker stop before the queued export.
        resume.send(()).unwrap();
    }

    #[test]
    fn notices_preserve_existing_text_and_decode_errors_remain_typed() {
        for (op, expected) in [
            (Op::Decode, "open: Could not load photo.png: failed"),
            (Op::List, "open: photo.png: failed"),
            (Op::Encode, "PNG photo.png: failed"),
            (Op::Save, "save photo.png: failed"),
        ] {
            let error = IoError {
                op,
                path: "photo.png".into(),
                cause: Cause::Io(io::Error::other("failed")),
            };
            assert_eq!(error.to_string(), expected);
            assert!(std::error::Error::source(&error).is_some());
            let stopped = IoError {
                op,
                path: "photo.png".into(),
                cause: Cause::Stopped,
            };
            assert_eq!(
                stopped.to_string(),
                format!("{op}: photo.png: skipped because the IO worker shut down")
            );
        }
        let error = IoError {
            op: Op::Decode,
            path: "photo.png".into(),
            cause: Cause::IsSource("photo.png".into()),
        };
        assert_eq!(
            error.to_string(),
            "open: photo.png is the source image and cannot be overwritten"
        );
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let path = dir.path().join("missing.png");
        worker.enqueue(open(path.clone()), 0);
        let done = wait(&mut worker);
        assert_eq!(done.path(), path);
        let Outcome::Opened(Err(ref error)) = done.outcome else {
            panic!("expected missing source error");
        };
        assert!(
            error
                .to_string()
                .starts_with(&format!("open: Could not load {}: ", path.display()))
        );
        assert!(matches!(
            done.outcome,
            Outcome::Opened(Err(IoError {
                cause: Cause::Io(ref error),
                ..
            })) if error.kind() == io::ErrorKind::NotFound
        ));
        std::fs::write(&path, b"invalid PNG").unwrap();
        worker.enqueue(open(path), 0);
        assert!(matches!(
            wait(&mut worker).outcome,
            Outcome::Opened(Err(IoError {
                cause: Cause::Decode(_),
                ..
            }))
        ));
    }
}
