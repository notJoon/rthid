# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.0.1] - Unreleased

### Added

- CLI help and version output via clap, with `--glyphs=quadrant` option syntax

- Real-pixel Original preview via Kitty, iTerm2, or Sixel with automatic detection,
  cached encoding, half-block fallback, and `--graphics auto|off`

- Truecolor Original preview on startup (lossless at 1:1, box-averaged in
  overview) with `Space` to toggle original / dithered;
  changing dither parameters reveals the dithered result
- Atkinson error diffusion engine with tone (luminance levels) and color
  (nearest palette color) modes, serpentine scanning, and diffusion strength
- Engine tone adjustments: brightness, contrast, gamma, black and white points,
  and sharpening
- Palettes from hex colors (`#rgb` or `#rrggbb`), ordered darkest to lightest
- Nearest-neighbor upscaling of dither results to RGBA
- Terminal editor: open an image (`rthid [image]`), scale 10–200%, rotate in
  90° steps, and adjust mode, palette preset, strength, serpentine, brightness,
  and contrast
- `d` / `D` to reset the selected parameter / all parameters and rotation
- Large sources are downscaled on load, and held-down keys re-dither once
- CI on Linux x86_64 with formatting, clippy, and tests
- Preview overview that fits the whole dither grid using Unicode sextants or
  quadrants, with aspect correction and a two-color fallback per cell ([#13])
- Exact 1:1 preview with keyboard, page, mouse drag, and wheel/trackpad panning ([#13])
- Minimap in 1:1 view that dims cells outside the viewport; click or drag to jump ([#13])
- `--preview-png <path>` to write a pixel-exact PNG after each re-dither ([#13])
- `--glyphs sextant|quadrant` to choose overview glyphs; quadrants are the
  default in macOS Terminal.app ([#13])
- `RTHID_DEBUG_LOG` for terminal rendering diagnostics ([#13])
- CI on macOS arm64 ([#13])

### Fixed

- Large grids no longer alias the dither pattern in the preview ([#13])
- `Esc` returns focus to the parameter panel instead of quitting ([#13])
- Overview no longer shows `?` boxes in terminals without sextant glyphs ([#13])
- Quitting waits for the final preview PNG and reports write failures ([#13])
- `--preview-png` refuses to overwrite the source image ([#13])

[#13]: https://github.com/notJoon/rthid/pull/13
