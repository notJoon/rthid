# Changelog

All notable changes to rthid-io are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.2.0] - Unreleased

### Breaking

- Remove the `png` module. Use `rthid_render` for grids and PNG encoding,
  and `rthid_io::atomic::replace` for atomic file replacement.
  `Job::Export { grid }` becomes `Job::Export { write }`. The closure chooses the
  output size. Grid errors no longer embed the path, read `IoError::path`.

  Before:
  ```rust,ignore
  use rthid_io::{png::{Grid, preview_scale, write_atomic}, io::Job};

  let grid = Grid::new(indices, w, h, palette)?;
  write_atomic(&path, &grid, preview_scale(grid.w, grid.h))?;
  let job = Job::Export { path, grid };
  ```

  After:
  ```rust,ignore
  use rthid_render::{Grid, preview_size, write_png};
  use rthid_io::{atomic::replace, io::Job};

  let grid = Grid::new(indices, w, h, palette)?;
  replace(&path, |file| write_png(file, &grid, preview_size(grid.w, grid.h), None))?;
  let job = Job::Export {
      path,
      write: Box::new(move |file| {
          write_png(file, &grid, preview_size(grid.w, grid.h), None)
      }),
  };
  ```

- Open filesystem errors: `Cause::Decode(ImageError::IoError(error))` -> `Cause::Io(error)`, match `kind()` directly
- Source protection: `Cause::IsSource` -> `Cause::IsSource(path)`, match the carried source path
- Error text: config wording and generic grid failures -> path aware file errors and specific grid checks, update text assertions while retaining error kind checks
- Shutdown text: load failure wording -> skipped job wording for `Cause::Stopped`, collect reads with `poll` before `finish`

### Added

- Error contracts for fallible APIs and process panic hook guidance for `WORKER_NAME` and `panic_message`
- Executable, compile-checked examples for synchronous config updates and background exports
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
