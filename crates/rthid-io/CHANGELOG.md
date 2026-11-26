# Changelog

All notable changes to rthid-io are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.1] - Unreleased

### Added

- Executable, compile-checked examples for synchronous config updates and background PNG exports
- Public contracts for latest-only jobs across paths, full unsaved Update snapshots, bounded
  shutdown, temporary-file cleanup, and preview image limits

### Fixed

- A config save that reached the file lock after shutdown gave up waiting for
  the IO worker could overwrite the newer value written on quit. A detached
  worker now leaves the file alone, and saves that finish within the shutdown
  wait still write as before

## [0.1.0] - 2026-11-26

### Added

- Source image decoding and shrinking with protection from overwriting the source
- Directory listings, home directory expansion and terminal drop path parsing
- PNG export with a background writer that writes the latest queued grid
- Atomic file replacement through a temporary file beside the destination
- Background PNG write durations when debug logging is enabled
