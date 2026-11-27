# Changelog

All notable changes to rthid-render are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - 2026-11-28

### Added

- A CPU benchmark for PNG encoding
- Self-contained deterministic SVG mass-study analysis sheets with embedded PNG previews
- Optional DPI records PNG resolution metadata in pixels per metre on both axes
- `check_output_size` exposes output validation before job submission
- Direct PNG encoding preserves existing bytes when DPI is omitted

- Owned `Grid` with public indices, dimensions in dots and palette, deriving `Debug` and `Clone`
- `Grid::new` validates grid invariants independently of output size
- `preview_size` chooses the preview output size in pixels with an integer dot size from 1 to 4
- `write_png(writer, &grid, (width, height), dpi)` samples dots with nearest neighbor
  sampling and encodes opaque RGBA PNG
- `MAX_OUTPUT_PIXELS` limits output area to bound the RGBA buffer to about 1 GiB
- `write_png` together with `rthid_io::atomic::replace` replaces `rthid_io::png::write_atomic`
- Validation errors include the actual sizes and indices
- Writer errors are preserved and the writer is flushed on success
