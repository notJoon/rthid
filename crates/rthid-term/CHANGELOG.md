# Changelog

All notable changes to rthid-term are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - Unreleased

### Added

- A CPU benchmark for full-frame `ElidingBackend` output
- Terminal session that enters raw mode and the alternate screen, restores them
  on exit and on panic, and never lets buffered output land after a panic
  restore
- Frame output through a 1 MB buffer, so a full frame reaches the terminal in a
  few writes
- `synchronized_draw` for synchronized frame updates on terminals that support it,
  returning the closure's successful value when it does not borrow the terminal
- `ElidingBackend`, which sends only the foreground or background color that
  changed and skips the foreground of plain spaces
- `ElidingBackend::writer` and `writer_mut` for access to the output writer, and
  `Debug` when the writer supports it
- `ElidingBackend` parks the hidden cursor at the start of the last row after
  each frame instead of leaving it on the last cell the frame wrote
- `ElidingBackend` moves the cursor explicitly after a cell whose symbol holds
  escape sequences, such as an inline image, instead of assuming where the
  terminal left it
- `ElidingBackend` clears the whole screen one line at a time instead of with
  erase-display, which some terminals, like iTerm2, copy into the shell's
  scrollback
- `SessionOptions` and `run_terminal_with_options` for session-owned mouse capture
  and bracketed paste, with current and future fields defaulting to off
- Copyable quick-start and input-mode examples, compiled as documentation tests
- Run functions return initialization, final flush and cleanup errors, preserving
  an application error when multiple operations fail
- Setup errors identify the failed step and preserve the original `io::ErrorKind`
- Run functions accept successful closure return values beyond `()`
- Reject nested or concurrent sessions with `io::ErrorKind::AlreadyExists`
- Install one panic hook that only restores the active session on its owning
  thread and remains inactive outside sessions
