# Changelog

All notable changes to rthid-term are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - Unreleased

### Added

- Terminal session that enters raw mode and the alternate screen, restores them
  on exit and on panic, and never lets buffered output land after a panic
  restore
- Frame output through a 1 MB buffer, so a full frame reaches the terminal in a
  few writes
- `synchronized_draw` for synchronized frame updates on terminals that support it
- `ElidingBackend`, which sends only the foreground or background color that
  changed and skips the foreground of plain spaces
- `SessionOptions` and `run_terminal_with_options` for session-owned mouse capture
  and bracketed paste, both disabled by default
- Copyable quick-start and input-mode examples, compiled as documentation tests

### Changed

- Breaking, unreleased: replace the `MouseCapture` and `BracketedPaste` unit
  guards and manual enable commands with `SessionOptions`
- Run functions return initialization, final flush and cleanup errors, preserving
  an application error when multiple operations fail
- Run functions accept successful closure return values beyond `()`
- Reject nested or concurrent sessions with `io::ErrorKind::AlreadyExists`
- Install one panic hook that only restores the active session on its owning
  thread and remains inactive outside sessions
