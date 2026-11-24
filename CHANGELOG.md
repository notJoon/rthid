# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.0.1] - Unreleased

### Added

- Preview overview that fits the whole dither grid using Unicode sextants or
  quadrants, with aspect correction and a two-color fallback per cell ([#13])
- Exact 1:1 preview with keyboard, page, mouse drag, and wheel/trackpad panning ([#13])
- Minimap in 1:1 view that dims cells outside the viewport; click or drag to jump ([#13])
- `--preview-png <path>` to write a pixel-exact PNG after each re-dither ([#13])
- `--glyphs sextant|quadrant` to choose overview glyphs; quadrants are the
  default in macOS Terminal.app ([#13])
- `RTHID_DEBUG_LOG` for terminal rendering diagnostics ([#13])
- CI on Linux x86_64 and macOS arm64 ([#13])

### Fixed

- Large grids no longer alias the dither pattern in the preview ([#13])
- `Esc` returns focus to the parameter panel instead of quitting ([#13])
- Overview no longer shows `?` boxes in terminals without sextant glyphs ([#13])
- Quitting waits for the final preview PNG and reports write failures ([#13])
- `--preview-png` refuses to overwrite the source image ([#13])

[#13]: https://github.com/notJoon/rthid/pull/13
