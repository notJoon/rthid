//! Shared background filesystem work with independent latest results for each kind.

use crate::{dir::Listing, png};
use image::RgbaImage;
use std::{
    collections::VecDeque,
    fmt, io,
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

/// Work kinds. Jobs of the same kind coalesce, different kinds never replace each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Open an image or directory.
    Open,
    /// List a directory.
    List,
    /// Write a PNG grid. Coalesces across paths because the companion path is fixed for a run.
    Export,
}

/// Owned filesystem work for the shared thread.
pub enum Job {
    /// Decode an image, or list it when the path is a directory.
    Open {
        /// Entered path, with optional home expansion.
        path: PathBuf,
        /// Base directory for relative paths.
        dir: Option<PathBuf>,
        /// Companion PNG path that must never be opened as the source.
        protect: Option<PathBuf>,
    },
    /// List the given directory.
    List(PathBuf),
    /// Encode a grid at the destination.
    Export {
        /// PNG destination.
        path: PathBuf,
        /// Pixels and palette to encode.
        grid: png::Grid,
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
    /// A PNG was exported.
    Exported(Result<PathBuf, IoError>),
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
    /// Submission id shared across all kinds.
    pub id: u64,
    /// Caller value given to `submit`, returned unchanged.
    pub tag: u64,
    /// Measured duration of the work or wait before stopping.
    pub elapsed: Duration,
    /// Typed result of the work.
    pub outcome: Outcome,
}

impl Done {
    /// Returns the kind carried by the outcome.
    pub(crate) fn kind(&self) -> Kind {
        match self.outcome {
            Outcome::Opened(_) => Kind::Open,
            Outcome::Listed(_) => Kind::List,
            Outcome::Exported(_) => Kind::Export,
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
            Outcome::Opened(Err(error))
            | Outcome::Listed(Err(error))
            | Outcome::Exported(Err(error)) => (error.op, &error.path),
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
    /// Encode a PNG.
    Encode,
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Decode => "decode",
            Self::List => "list",
            Self::Encode => "encode",
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
    /// The companion PNG resolves to the source image.
    IsSource,
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
            Self::IsSource => f.write_str("--preview-png must not point at the source image"),
            Self::Panicked(message) => write!(f, "IO job panicked: {message}"),
            Self::Stopped => f.write_str("IO worker stopped"),
            Self::TimedOut => f.write_str("operation did not finish before quitting"),
        }
    }
}

impl fmt::Display for IoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if matches!(self.cause, Cause::IsSource) {
            return write!(f, "open: {}", self.cause);
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

/// Stable worker name used to isolate caught panics from terminal panic hooks.
pub const WORKER_NAME: &str = "rthid-io";

/// Longest wait for the worker when the queue shuts down.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

/// Text of a panic payload, for reporting worker panics.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic")
}

struct Queue {
    slots: Mutex<[Option<(u64, u64, Job)>; 3]>,
    ready: Condvar,
}

/// One filesystem thread with an independent newest result for each work kind.
#[allow(clippy::upper_case_acronyms)]
pub struct IO {
    jobs: Arc<Queue>,
    results: Receiver<Done>,
    local: VecDeque<Done>,
    pending: [Option<Pending>; 3],
    next_id: u64,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    // Disconnects when the worker exits, including by panic.
    exited: Receiver<()>,
}

const KINDS: [Kind; 3] = [Kind::Open, Kind::List, Kind::Export];

impl IO {
    /// Starts the worker. `home` expands `~` in Open paths without reading the environment.
    pub fn new(home: Option<PathBuf>) -> io::Result<Self> {
        let jobs = Arc::new(Queue {
            slots: Mutex::new([None, None, None]),
            ready: Condvar::new(),
        });
        let queued = Arc::clone(&jobs);
        let (finished, results) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let (alive, exited) = mpsc::channel();
        let thread = thread::Builder::new()
            .name(WORKER_NAME.into())
            .spawn(move || {
                let _alive: Sender<()> = alive;
                run(queued, finished, home, stop)
            })?;
        Ok(Self {
            jobs,
            results,
            local: VecDeque::new(),
            pending: [None, None, None],
            next_id: 0,
            stopping,
            thread: Some(thread),
            exited,
        })
    }

    /// Queues work without waiting, returning an id that increases across kinds.
    /// The result echoes `tag` so callers can tell who asked for it.
    pub fn submit(&mut self, job: Job, tag: u64) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        let kind = job.kind();
        let (op, path) = job.context();
        self.pending[kind as usize] = Some(Pending {
            id,
            tag,
            op,
            path,
            submitted: Instant::now(),
        });
        if self.thread.as_ref().is_none_or(JoinHandle::is_finished) {
            self.local.push_back(
                self.pending[kind as usize]
                    .as_ref()
                    .unwrap()
                    .fail(kind, Cause::Stopped),
            );
        } else {
            self.jobs.slots.lock().unwrap()[kind as usize] = Some((id, tag, job));
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
                        return KINDS.into_iter().find_map(|kind| {
                            self.pending[kind as usize]
                                .take()
                                .map(|pending| pending.fail(kind, Cause::Stopped))
                        });
                    }
                }
            };
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
        self.pending.iter().any(Option::is_some)
    }

    /// Skips queued reads and waits up to five seconds for the newest export.
    /// Returns available results and detaches a worker still running at the deadline.
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
        done
    }

    fn stop_timeout(&mut self, timeout: Duration) {
        self.stopping.store(true, Ordering::SeqCst);
        // Hold the mutex while notifying so an idle worker cannot miss shutdown.
        let slots = self.jobs.slots.lock().unwrap();
        self.jobs.ready.notify_one();
        drop(slots);
        // A worker still running at the deadline is detached.
        if let Some(thread) = self.thread.take()
            && matches!(
                self.exited.recv_timeout(timeout),
                Err(RecvTimeoutError::Disconnected)
            )
        {
            let _ = thread.join();
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
        },
    }
}

// TODO: One thread runs every kind, so a listing or open waits behind a running export.
//       Give each kind its own thread on the same slots, or at least split Export off.
fn run(queued: Arc<Queue>, finished: Sender<Done>, home: Option<PathBuf>, stop: Arc<AtomicBool>) {
    loop {
        let (id, tag, job) = {
            let mut slots = queued.slots.lock().unwrap();
            loop {
                if let Some(index) = slots
                    .iter()
                    .enumerate()
                    .filter_map(|(index, job)| job.as_ref().map(|(id, ..)| (index, *id)))
                    .min_by_key(|(_, id)| *id)
                    .map(|(index, _)| index)
                {
                    break slots[index].take().unwrap();
                }
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                slots = queued.ready.wait(slots).unwrap();
            }
        };
        let kind = job.kind();
        let (mut op, mut path) = job.context();
        let started = Instant::now();
        let result = catch_unwind(AssertUnwindSafe(|| {
            if stop.load(Ordering::SeqCst) && kind != Kind::Export {
                return Err(Cause::Stopped);
            }
            execute(job, home.as_deref(), &mut op, &mut path)
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
            crate::image::check_not_source(path, protect.as_deref())?;
            let image = crate::image::load_image(path).map_err(Cause::Decode)?;
            Ok(Outcome::Opened(Ok(Opened::Image {
                path: path.clone(),
                image,
            })))
        }
        Job::List(_) => listing(path).map(|listing| Outcome::Listed(Ok(listing))),
        Job::Export { grid, .. } => {
            png::write_atomic(path, &grid, png::preview_scale(grid.w, grid.h))
                .map_err(Cause::Io)?;
            Ok(Outcome::Exported(Ok(path.clone())))
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
    use rthid_dither::dither::make_palette;

    fn export(path: PathBuf, white: bool) -> Job {
        Job::Export {
            path,
            grid: png::Grid {
                indices: vec![u8::from(white)],
                w: 1,
                h: 1,
                palette: make_palette(&["#000", "#fff"]).unwrap(),
            },
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
        worker.submit(Job::Pause(entered, paused), 0);
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        resume
    }

    fn open(path: PathBuf) -> Job {
        Job::Open {
            path,
            dir: None,
            protect: None,
        }
    }

    #[test]
    fn exports_coalesce_and_only_the_latest_completion_is_polled() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let resume = pause(&mut worker);
        let mut last = 0;
        for n in 0..100 {
            last = worker.submit(export(dir.path().join(format!("{n}.png")), n == 99), 0);
        }
        assert!(worker.busy());
        assert_eq!(
            worker.jobs.slots.lock().unwrap().iter().flatten().count(),
            1
        );
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
        let image = image::open(path).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (4, 4));
        assert!(image.pixels().all(|pixel| pixel.0 == [255; 4]));
        assert!(!worker.busy());
        assert!(worker.poll().is_none());
        assert!(worker.finish().is_empty());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn open_and_list_keep_separate_results_and_resolve_open_paths() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(Some(dir.path().to_path_buf())).unwrap();
        let resume = pause(&mut worker);
        let opened = worker.submit(open(PathBuf::from("~")), 7);
        let listed = worker.submit(Job::List(dir.path().to_path_buf()), 8);
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
        worker.submit(
            Job::Open {
                path: "source.png".into(),
                dir: Some(dir.path().to_path_buf()),
                protect: None,
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
        worker.submit(
            Job::Open {
                path: path.clone(),
                dir: None,
                protect: Some(path),
            },
            0,
        );
        assert!(matches!(
            wait(&mut worker).outcome,
            Outcome::Opened(Err(IoError {
                cause: Cause::IsSource,
                ..
            }))
        ));
    }

    #[test]
    fn panic_is_reported_and_the_thread_runs_the_next_job() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let id = worker.submit(Job::Panic(dir.path().join("panic")), 0);
        let done = wait(&mut worker);
        assert_eq!(done.id, id);
        assert!(matches!(done.outcome, Outcome::Opened(Err(IoError {
            cause: Cause::Panicked(message), ..
        })) if message == "test job panicked"));
        let id = worker.submit(Job::List(dir.path().to_path_buf()), 0);
        let done = wait(&mut worker);
        assert_eq!(done.id, id);
        assert!(matches!(done.outcome, Outcome::Listed(Ok(_))));
    }

    #[test]
    fn finish_skips_queued_reads_and_flushes_only_the_newest_export() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let resume = pause(&mut worker);
        let opened = worker.submit(open(dir.path().to_path_buf()), 0);
        let listed = worker.submit(Job::List(dir.path().to_path_buf()), 0);
        worker.submit(export(dir.path().join("old.png"), false), 0);
        let exported = worker.submit(export(dir.path().join("final.png"), true), 0);
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
            [opened, listed, exported]
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
        assert!(dir.path().join("final.png").exists());
        assert!(!dir.path().join("old.png").exists());
    }

    #[test]
    fn stale_results_are_discarded_and_a_stopped_thread_still_returns_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        worker.submit(export(dir.path().join("old.png"), false), 0);
        let resume = pause(&mut worker);
        let latest = worker.submit(export(dir.path().join("latest.png"), true), 0);
        // The earlier Export completed before Pause began and must still be discarded.
        assert!(worker.poll().is_none());
        resume.send(()).unwrap();
        assert_eq!(wait(&mut worker).kind(), Kind::List);
        assert_eq!(wait(&mut worker).id, latest);
        worker.stop_timeout(SHUTDOWN_WAIT);
        for _ in 0..2 {
            let id = worker.submit(open(dir.path().to_path_buf()), 3);
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
        let id = worker.submit(Job::List("missing".into()), 5);
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
        let opened = worker.submit(open(dir.path().to_path_buf()), 0);
        let resume = pause(&mut worker);
        let exported = worker.submit(export("unused.png".into(), false), 6);
        let started = Instant::now();
        let done = worker.finish_timeout(Duration::from_millis(50));
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
        ] {
            let error = IoError {
                op,
                path: "photo.png".into(),
                cause: Cause::Io(io::Error::other("failed")),
            };
            assert_eq!(error.to_string(), expected);
            assert!(std::error::Error::source(&error).is_some());
        }
        let error = IoError {
            op: Op::Decode,
            path: "photo.png".into(),
            cause: Cause::IsSource,
        };
        assert_eq!(
            error.to_string(),
            "open: --preview-png must not point at the source image"
        );
        let dir = tempfile::tempdir().unwrap();
        let mut worker = IO::new(None).unwrap();
        let path = dir.path().join("missing.png");
        worker.submit(open(path.clone()), 0);
        let done = wait(&mut worker);
        assert_eq!(done.path(), path);
        assert!(matches!(
            done.outcome,
            Outcome::Opened(Err(IoError {
                cause: Cause::Decode(_),
                ..
            }))
        ));
    }
}
