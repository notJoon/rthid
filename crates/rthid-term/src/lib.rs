//! Terminal session and frame output for a ratatui app.
//!
//! This crate owns the raw mode and alternate screen session, the buffered writer that frames go
//! through, the synchronized update that brackets each frame, and the guards that release mouse
//! capture and bracketed paste when the session ends. It does not know the app. The caller passes
//! the work to run inside the session and the drawing to do inside each frame.
//!
//! Frames are buffered because panning is bound by terminal throughput, and stdout's line buffer
//! would split each frame into many small writes.

use std::{
    io::{self, BufWriter, Stdout, Write},
    mem::ManuallyDrop,
};

use ratatui::crossterm::{
    event::{DisableBracketedPaste, DisableMouseCapture},
    execute, queue,
    terminal::{
        BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen, enable_raw_mode,
    },
};
use ratatui::{Terminal, backend::Backend};

mod backend;

pub use backend::ElidingBackend;

// 1 MB holds a typical full frame, so it reaches the terminal in a few writes instead of
// filling stdout's small line buffer over and over.
const FRAME_BUFFER_CAPACITY: usize = 1024 * 1024;

/// The terminal that [`run_terminal`] hands to its closure.
///
/// Output collects in a large buffer and reaches the terminal only when the buffer fills or is
/// flushed, which [`synchronized_draw`] does at the end of every frame. Anything written through
/// the backend, such as mode changes, stays in order with the frames.
pub type BufferedTerminal = Terminal<ElidingBackend<BufWriter<Stdout>>>;

/// Runs `f` inside a raw mode and alternate screen session and restores the terminal afterwards.
///
/// This mirrors `ratatui::run`, except that frames go through a large buffer. Like `ratatui::run`,
/// it panics if the terminal cannot be initialized. On normal exit, including when `f` returns an
/// error, it flushes the buffered output before restoring the terminal and returns the result of
/// `f`.
///
/// It installs a panic hook that ends any open synchronized update and restores the terminal
/// before the previous hook runs. A panic leaks the terminal instead of dropping it while
/// unwinding, so buffered bytes are never written after the restore. A hook that `f` installs
/// later wraps this one and should still call it.
pub fn run_terminal(f: impl FnOnce(&mut BufferedTerminal) -> io::Result<()>) -> io::Result<()> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // A panic inside a frame leaves the update open, and the terminal could hold back the
        // restore and the panic message.
        let _ = execute!(io::stdout(), EndSynchronizedUpdate);
        ratatui::restore();
        previous(info);
    }));
    // Unwinding leaks the terminal, so pending output never follows the panic hook's restore.
    let mut terminal = ManuallyDrop::new(
        (|| {
            enable_raw_mode()?;
            let mut writer = BufWriter::with_capacity(FRAME_BUFFER_CAPACITY, io::stdout());
            execute!(writer, EnterAlternateScreen)?;
            Terminal::new(ElidingBackend::new(writer))
        })()
        .expect("failed to initialize terminal"),
    );
    let result = f(&mut terminal);
    let _ = Write::flush(terminal.backend_mut());
    // The restore writes to stdout directly, so every buffered byte has to land first.
    drop(ManuallyDrop::into_inner(terminal));
    ratatui::restore();
    result
}

/// Wraps one frame in a synchronized update (mode 2026) so the terminal paints it whole.
///
/// Many modern terminal emulators honor mode 2026 and show the frame only once it is complete,
/// which avoids tearing while panning. Terminals without support ignore the sequences.
///
/// The begin sequence is queued before `draw` runs. The end sequence is always sent and flushed,
/// even when `draw` fails, so no update is left open and no frame bytes stay in the buffer. The
/// error from `draw` takes precedence over an error from ending the update.
pub fn synchronized_draw<B: Backend + Write>(
    terminal: &mut Terminal<B>,
    draw: impl FnOnce(&mut Terminal<B>) -> io::Result<()>,
) -> io::Result<()> {
    queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let result = draw(terminal);
    // End the update even when drawing fails, before any direct stdout cleanup.
    let end = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    result.and(end)
}

/// Disables mouse capture on drop, so it is released even when the terminal session fails.
///
/// Create it before [`run_terminal`] and enable capture through the buffered terminal so the
/// enable stays in order with the frames. The guard writes directly to stdout, so it must drop
/// after the buffered terminal is flushed. Dropping it after [`run_terminal`] returns satisfies
/// this, because [`run_terminal`] flushes on normal exit.
pub struct MouseCapture;

impl Drop for MouseCapture {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

/// Disables bracketed paste on drop, so it is released even when the terminal session fails.
///
/// The same ordering rules as [`MouseCapture`] apply. The guard writes directly to stdout, so it
/// must drop after the buffered terminal is flushed, which [`run_terminal`] guarantees on normal
/// exit.
pub struct BracketedPaste;

impl Drop for BracketedPaste {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                Ok(())
            });
            assert_eq!(result.is_err(), fail);
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
