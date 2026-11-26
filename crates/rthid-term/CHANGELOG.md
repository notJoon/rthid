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
- Synchronized update around every frame for terminals that support it
- `ElidingBackend`, which sends only the foreground or background color that
  changed and skips the foreground of plain spaces
- Mouse capture and bracketed paste guards that release their modes on drop
