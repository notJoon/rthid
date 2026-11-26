//! Raw mode and alternate screen sessions, buffered frame output and synchronized updates per frame.
//! Terminal mode guards release mouse capture and bracketed paste when the session ends.
//! This crate does not know the app.

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
use ratatui::{Terminal, backend::CrosstermBackend};

// Keep typical full frames together instead of filling stdout's small line buffer.
const FRAME_BUFFER_CAPACITY: usize = 1024 * 1024;

/// A terminal with buffered stdout for frame output.
pub type BufferedTerminal = Terminal<CrosstermBackend<BufWriter<Stdout>>>;

/// Sets up raw mode and the alternate screen like `ratatui::run`, with frames written through a large buffer.
pub fn run_terminal(f: impl FnOnce(&mut BufferedTerminal) -> io::Result<()>) -> io::Result<()> {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
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
            Terminal::new(CrosstermBackend::new(writer))
        })()
        .expect("failed to initialize terminal"),
    );
    let result = f(&mut terminal);
    let _ = terminal.backend_mut().flush();
    drop(ManuallyDrop::into_inner(terminal));
    ratatui::restore();
    result
}

/// Brackets one frame in a synchronized update so supporting terminals paint it whole.
pub fn synchronized_draw<W: Write>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    draw: impl FnOnce(&mut Terminal<CrosstermBackend<W>>) -> io::Result<()>,
) -> io::Result<()> {
    queue!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let result = draw(terminal);
    // End the update even when drawing fails, before any direct stdout cleanup.
    let end = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    result.and(end)
}

/// Disables mouse capture on drop, so it is released even when the terminal session fails.
pub struct MouseCapture;

impl Drop for MouseCapture {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

/// Disables bracketed paste even when the terminal session fails.
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
            CrosstermBackend::new(BufWriter::with_capacity(
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
            terminal.backend_mut().flush().unwrap();
            assert_eq!(output.borrow().len(), end);
            let output = output.borrow();
            let frame = &output[start..];
            assert!(frame.starts_with(b"\x1b[?2026h"));
            assert!(frame.ends_with(b"pending\x1b[?2026l"));
        }
    }
}
