# rthid-term

Terminal session and frame output for ratatui apps that redraw much of the screen
every frame.

## Quick start

This crate is not published to crates.io. Add a path dependency to your app's
`Cargo.toml`, adjusting the path to your checkout:

```toml
[dependencies]
ratatui = "0.30.2"
rthid-term = { path = "../rthid/crates/rthid-term" }
```

Put this in `src/main.rs`, then run `cargo run` in an interactive terminal:

```rust,no_run
use ratatui::crossterm::event;
use rthid_term::{run_terminal, synchronized_draw};

fn main() -> std::io::Result<()> {
    run_terminal(|terminal| loop {
        synchronized_draw(terminal, |terminal| {
            terminal.draw(|frame| {
                frame.render_widget("Hello! Press any key to quit.", frame.area());
            }).map(|_| ())
        })?;
        if event::read()?.is_key_press() {
            return Ok(());
        }
    })
}
```

No app type or event loop trait is required. The closure receives a normal
ratatui `Terminal`, so existing widgets and rendering code work unchanged.

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
  Other terminals ignore the sequences. It returns the closure's successful value.
- **Session setup and restore.** `run_terminal` enters raw mode and the alternate
  screen and restores them afterwards. Initialization failures return an error.
- **Optional input modes.** `SessionOptions` opts into mouse capture and bracketed
  paste. The session enables and disables them, including cleanup on failure.

## Mouse capture and bracketed paste

Both modes and any future fields default to off. Use `run_terminal_with_options` when your app
handles mouse or paste events:

```rust,no_run
use ratatui::crossterm::event;
use rthid_term::{SessionOptions, run_terminal_with_options, synchronized_draw};

fn main() -> std::io::Result<()> {
    let options = SessionOptions {
        mouse_capture: true,
        bracketed_paste: true,
        ..SessionOptions::default()
    };
    run_terminal_with_options(options, |terminal| loop {
        synchronized_draw(terminal, |terminal| {
            terminal.draw(|frame| {
                frame.render_widget("Mouse and paste enabled. Press any key to quit.", frame.area());
            }).map(|_| ())
        })?;
        match event::read()? {
            event::Event::Key(key) if key.is_press() => return Ok(()),
            event::Event::Mouse(_mouse) => { /* Update your app here. */ }
            event::Event::Paste(_text) => { /* Update your app here. */ }
            _ => {}
        }
    })
}
```

Do not enable these modes separately or manage their cleanup guards yourself.
For custom writers or viewports, construct `Terminal<ElidingBackend<W>>` directly
and keep using `synchronized_draw`; terminal session setup is then your responsibility.
Use `writer()` or `writer_mut()` on the backend to inspect or access the output writer.
The backend implements `Debug` when its writer does.

The value returned by `synchronized_draw` cannot borrow the terminal because ending
the update requires mutable access to the backend. `Terminal::draw` returns a borrowed
`CompletedFrame`, so the examples discard it with `.map(|_| ())` inside the closure.

## Results and session ownership

Both run functions return the closure's successful value in `std::io::Result<T>`.
Initialization, final flush and cleanup failures also return `io::Error`. If more
than one operation fails, the application error takes precedence, then the flush
error, then the cleanup error. Cleanup is attempted even after an earlier error.
Closures use `io::Error`; convert application-specific errors at this seam if needed.
Setup errors name the failed step and preserve the original `io::ErrorKind`.

Only one session can run at a time in a process. Nested or concurrent calls return
`io::ErrorKind::AlreadyExists`; they do not take over another session's terminal.

The crate installs a process-wide panic hook once. It restores the terminal only
for a panic on the active session's owning thread and is inactive outside that
session. If you install a hook afterwards, call the previous hook to retain this
cleanup. Other threads' panics continue through the previous hook without ending
the session.

Catch panics outside the session. On panic, its buffered terminal is leaked so
pending output cannot arrive after restoration. If a panic is caught inside the
closure, the session returns an error when the closure finishes; its terminal
must no longer be used.

See the crate docs (`cargo doc -p rthid-term --open`) for the API.
