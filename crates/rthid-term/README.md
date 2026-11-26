# rthid-term

Terminal session and frame output for ratatui apps that redraw much of the screen
every frame.

## Why

Apps that repaint many colored cells per frame, such as image previews,
animations or panning views, are often bound by how fast the terminal emulator
takes in output rather than by rendering. In one profile, most of the main thread
waited in write(2) while rendering took under 1 percent. Stdout's line buffer
makes this worse: a frame contains no newlines, so it leaves in many small writes.

## What it does

- **Buffered frames.** `run_terminal` hands the closure a `BufferedTerminal` that
  writes through a 1 MB `BufWriter`, so a full frame takes a few writes instead
  of hundreds. This cuts syscalls and encode time, but it does not raise the frame
  rate when the terminal's intake is the limit.
- **Fewer color escapes.** `ElidingBackend` sends only changed color channels and
  skips foreground changes for spaces without reverse, underline or strikeout,
  reducing the bytes the terminal must process while preserving visible output.
- **Synchronized updates.** `synchronized_draw` wraps one frame in a synchronized
  update (mode 2026) and flushes it, so supporting terminals paint whole frames.
  Other terminals ignore the sequences.
- **Session setup and restore.** `run_terminal` enters raw mode and the alternate
  screen and restores them afterwards, like `ratatui::run`. On panic it leaks the
  terminal, so buffered output never lands after the panic hook has restored the
  screen.
- **Mode guards.** `MouseCapture` and `BracketedPaste` disable their modes on
  drop. Create them before `run_terminal` and enable the modes through the
  buffered terminal, so the enable sequences stay in order with the frames.

## Usage

```rust
use ratatui::crossterm::{event::{self, EnableBracketedPaste, EnableMouseCapture}, execute};
use rthid_term::{BracketedPaste, MouseCapture, run_terminal, synchronized_draw};

// run_terminal supplies Terminal<ElidingBackend<BufWriter<Stdout>>>.
let _mouse = MouseCapture;
let _paste = BracketedPaste;
run_terminal(|terminal| {
    execute!(terminal.backend_mut(), EnableMouseCapture, EnableBracketedPaste)?;
    while !app.quit {
        synchronized_draw(terminal, |terminal| {
            terminal.draw(|frame| app.render(frame))?;
            Ok(())
        })?;
        app.handle(event::read()?);
    }
    Ok(())
})?;
```

See the crate docs (`cargo doc -p rthid-term --open`) for the API.
