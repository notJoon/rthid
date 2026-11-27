#![doc = include_str!("../README.md")]

use std::{
    cell::Cell,
    io::{self, BufWriter, Stdout, Write},
    mem::ManuallyDrop,
    sync::{
        Once,
        atomic::{AtomicBool, Ordering},
    },
};

use ratatui::crossterm::{
    cursor::Show,
    event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture},
    execute, queue,
    terminal::{
        BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
        disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{Terminal, backend::Backend};

mod backend;

pub use backend::ElidingBackend;

// 1 MB holds a typical full frame, avoiding repeated small stdout writes.
const FRAME_BUFFER_CAPACITY: usize = 1024 * 1024;
static SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: Once = Once::new();
thread_local! {
    static SESSION_OPTIONS: Cell<Option<SessionOptions>> = const { Cell::new(None) };
}

/// The terminal supplied to the session closure.
///
/// This is a standard ratatui terminal. Both [`Terminal::draw`] and [`synchronized_draw`]
/// flush output; direct backend writes stay buffered until a flush or until the buffer fills.
pub type BufferedTerminal = Terminal<ElidingBackend<BufWriter<Stdout>>>;

/// Optional input modes owned by a terminal session. Both are disabled by default.
/// New fields will also default to off.
#[derive(Clone, Copy, Debug, Default)]
pub struct SessionOptions {
    /// Capture mouse events for the duration of the session.
    pub mouse_capture: bool,
    /// Receive pasted text as bracketed paste events for the duration of the session.
    pub bracketed_paste: bool,
}

/// Runs `f` in a fullscreen raw-mode session with default input modes.
///
/// See [`run_terminal_with_options`] for error handling and panic behavior.
pub fn run_terminal<T>(f: impl FnOnce(&mut BufferedTerminal) -> io::Result<T>) -> io::Result<T> {
    run_terminal_with_options(SessionOptions::default(), f)
}

/// Runs `f` with buffered frame output and restores terminal modes on exit.
///
/// Setup errors include the failed step and preserve the original [`io::ErrorKind`].
/// The closure's error takes precedence over a final flush error, which takes precedence over a
/// cleanup error. Otherwise its value is returned.
/// Nested or concurrent sessions return [`io::ErrorKind::AlreadyExists`].
///
/// The first session installs a process-wide panic hook, which restores only an active session
/// on its owning thread before calling the previous hook. The hook remains installed but is inert
/// outside a session. Later hooks must call the previous hook to preserve this behavior.
/// A panic leaks buffered output so it cannot land after restoration. Catch panics outside the
/// session; catching a panic inside `f` does not make its terminal usable again.
pub fn run_terminal_with_options<T>(
    options: SessionOptions,
    f: impl FnOnce(&mut BufferedTerminal) -> io::Result<T>,
) -> io::Result<T> {
    let session = Session::acquire(options)?;
    enable_raw_mode().map_err(|e| setup_error("enable raw mode (is stdin a terminal?)", e))?;
    let mut writer = BufWriter::with_capacity(FRAME_BUFFER_CAPACITY, io::stdout());
    execute!(writer, EnterAlternateScreen).map_err(|e| setup_error("enter alternate screen", e))?;
    enable_modes(&mut writer, options).map_err(|e| setup_error("enable input modes", e))?;
    // Panic restoration happens before unwinding: never drop pending buffered output afterwards.
    let mut terminal = ManuallyDrop::new(
        Terminal::new(ElidingBackend::new(writer))
            .map_err(|e| setup_error("initialize terminal", e))?,
    );
    let result = f(&mut terminal);
    if SESSION_OPTIONS.get().is_none() {
        return result.and(Err(io::Error::other(
            "terminal session was interrupted by a caught panic",
        )));
    }
    let result = finish_terminal(terminal, result);
    let cleanup = session.restore();
    result.and_then(|value| cleanup.map(|()| value))
}

fn setup_error(step: &str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("rthid-term: failed to {step}: {error}"),
    )
}

fn finish_terminal<T, B: Backend<Error = io::Error> + Write>(
    mut terminal: ManuallyDrop<Terminal<B>>,
    result: io::Result<T>,
) -> io::Result<T> {
    let cursor = terminal.show_cursor();
    let flush = Write::flush(terminal.backend_mut());
    drop(ManuallyDrop::into_inner(terminal));
    result.and_then(|value| flush.and(cursor).map(|()| value))
}

struct Session;

impl Session {
    fn acquire(options: SessionOptions) -> io::Result<Self> {
        SESSION_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "a terminal session is already active",
                )
            })?;
        let session = Self;
        PANIC_HOOK.call_once(|| {
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                if let Some(options) = SESSION_OPTIONS.replace(None) {
                    let _ = restore_terminal(options);
                }
                previous(info);
            }));
        });
        SESSION_OPTIONS.set(Some(options));
        Ok(session)
    }

    fn restore(&self) -> io::Result<()> {
        match SESSION_OPTIONS.replace(None) {
            Some(options) => restore_terminal(options),
            None => Ok(()),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.restore();
        SESSION_ACTIVE.store(false, Ordering::Release);
    }
}

fn enable_modes(writer: &mut impl Write, options: SessionOptions) -> io::Result<()> {
    if options.mouse_capture {
        execute!(writer, EnableMouseCapture)?;
    }
    if options.bracketed_paste {
        execute!(writer, EnableBracketedPaste)?;
    }
    Ok(())
}

fn restore_terminal(options: SessionOptions) -> io::Result<()> {
    let raw = disable_raw_mode();
    let output = restore_modes(&mut io::stdout(), options);
    raw.and(output)
}

fn restore_modes(writer: &mut impl Write, options: SessionOptions) -> io::Result<()> {
    // Attempt each restoration even if an earlier output operation failed.
    let mut result = execute!(writer, EndSynchronizedUpdate);
    if options.mouse_capture {
        result = result.and(execute!(writer, DisableMouseCapture));
    }
    if options.bracketed_paste {
        result = result.and(execute!(writer, DisableBracketedPaste));
    }
    result = result.and(execute!(writer, Show));
    result.and(execute!(writer, LeaveAlternateScreen))
}

/// Wraps one frame in a synchronized update (mode 2026) so the terminal paints it whole.
///
/// Many modern terminal emulators honor mode 2026 and show the frame only once it is complete,
/// which avoids tearing while panning. Terminals without support ignore the sequences.
///
/// The begin sequence is queued before `draw` runs. The end sequence is always sent and flushed,
/// even when `draw` fails, so no update is left open and no frame bytes stay in the buffer. The
/// error from `draw` takes precedence over an error from ending the update.
/// Returns the closure's value after the update ends successfully. The value cannot borrow
/// the terminal because ending the update needs mutable access to its backend.
pub fn synchronized_draw<B: Backend + Write, R>(
    terminal: &mut Terminal<B>,
    draw: impl FnOnce(&mut Terminal<B>) -> io::Result<R>,
) -> io::Result<R> {
    queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let result = draw(terminal);
    // End the update even when drawing fails, before any direct stdout cleanup.
    let end = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    result.and_then(|value| end.map(|()| value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_errors_preserve_kind_and_include_context() {
        let error = setup_error(
            "enable raw mode (is stdin a terminal?)",
            io::Error::new(io::ErrorKind::PermissionDenied, "access denied"),
        );
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(
            error.to_string(),
            "rthid-term: failed to enable raw mode (is stdin a terminal?): access denied"
        );
    }

    #[test]
    fn modes_are_opt_in_and_cleanup_continues_after_failure() {
        let mut output = Vec::new();
        enable_modes(&mut output, SessionOptions::default()).unwrap();
        assert!(output.is_empty());
        restore_modes(&mut output, SessionOptions::default()).unwrap();
        assert!(!output.windows(7).any(|bytes| bytes == b"\x1b[?1000"));
        assert!(!output.windows(7).any(|bytes| bytes == b"\x1b[?2004"));

        struct FailingWriter {
            output: Vec<u8>,
            flushes: usize,
        }
        impl Write for FailingWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.output.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                self.flushes += 1;
                Err(io::Error::new(
                    if self.flushes == 1 {
                        io::ErrorKind::BrokenPipe
                    } else {
                        io::ErrorKind::Other
                    },
                    "flush failed",
                ))
            }
        }
        let options = SessionOptions {
            mouse_capture: true,
            bracketed_paste: true,
        };
        let mut enabled = Vec::new();
        enable_modes(&mut enabled, options).unwrap();
        assert!(enabled.ends_with(b"\x1b[?2004h"));
        let mut writer = FailingWriter {
            output: Vec::new(),
            flushes: 0,
        };
        assert_eq!(
            restore_modes(&mut writer, options).unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        assert_eq!(writer.flushes, 5);
        assert!(writer.output.ends_with(b"\x1b[?25h\x1b[?1049l"));
        assert!(
            writer
                .output
                .windows(8)
                .any(|bytes| bytes == b"\x1b[?2004l")
        );
    }

    #[test]
    fn final_flush_errors_are_returned_without_replacing_app_errors() {
        use ratatui::{TerminalOptions, Viewport, layout::Rect};

        struct FailingFlush;
        impl Write for FailingFlush {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "flush failed"))
            }
        }
        for app_failed in [false, true] {
            let terminal = Terminal::with_options(
                ElidingBackend::new(FailingFlush),
                TerminalOptions {
                    viewport: Viewport::Fixed(Rect::new(0, 0, 10, 1)),
                },
            )
            .unwrap();
            let app_result = if app_failed {
                Err(io::Error::new(io::ErrorKind::InvalidData, "app failed"))
            } else {
                Ok(42)
            };
            let error = finish_terminal(ManuallyDrop::new(terminal), app_result).unwrap_err();
            assert_eq!(
                error.kind(),
                if app_failed {
                    io::ErrorKind::InvalidData
                } else {
                    io::ErrorKind::BrokenPipe
                }
            );
        }
    }

    #[test]
    fn synchronized_frames_flush_before_cleanup_even_on_error() {
        use std::{cell::RefCell, rc::Rc};

        use ratatui::{TerminalOptions, Viewport, layout::Rect};

        struct SharedWriter(Rc<RefCell<Vec<u8>>>);

        impl Write for SharedWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let output = Rc::new(RefCell::new(Vec::new()));
        let mut terminal = Terminal::with_options(
            ElidingBackend::new(BufWriter::with_capacity(
                FRAME_BUFFER_CAPACITY,
                SharedWriter(Rc::clone(&output)),
            )),
            TerminalOptions {
                viewport: Viewport::Fixed(Rect::new(0, 0, 10, 1)),
            },
        )
        .unwrap();
        for fail in [false, true] {
            let start = output.borrow().len();
            let result = synchronized_draw(&mut terminal, |terminal| {
                terminal.draw(|frame| frame.render_widget("frame", frame.area()))?;
                terminal.backend_mut().write_all(b"pending")?;
                if fail {
                    return Err(io::Error::other("draw failed"));
                }
                Ok(42)
            });
            assert_eq!(result.ok(), (!fail).then_some(42));
            let end = output.borrow().len();
            Write::flush(terminal.backend_mut()).unwrap();
            assert_eq!(output.borrow().len(), end);
            let output = output.borrow();
            let frame = &output[start..];
            assert!(frame.starts_with(b"\x1b[?2026h"));
            assert!(frame.ends_with(b"pending\x1b[?2026l"));
        }
    }
}
