# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `?` opens a full keyboard and mouse reference outside dialogs

### Changed

- The help line shows only currently usable keys, fits the terminal width, and
  puts Enter and Esc first in dialogs

## [0.2.0] - 2026-11-28

### Added

- End-to-end terminal QA covers image loading, zoom, mouse input, resize cleanup,
  and terminal mode restoration through a pseudo terminal

- Local CPU benchmarks and a profiling build profile, with CodSpeed simulation in CI
- `--mass-svg PATH` exports a self-contained mass-study analysis sheet with embedded preview, vector tone layers, fitted ellipses and settings

- A deterministic test guards terminal bytes per frame during mouse-drag pans at 100% and 200% zoom

- `--dpi 1..4800` sizes companion PNGs from the card including bleed and records
  resolution metadata. It requires `--preview-png`. The status shows PNG dimensions
  and DPI. Oversized outputs are rejected before submission. Omitting DPI preserves
  preview sizing and bytes

- `f` mirrors left and right on the card after rotation, keeping pan directions
  and the current layer. The status shows `Flip` beside rotation. Templates keep
  the current flip, while opening an image resets it

- A Mass layer shows a flat four tone study of the undithered card grid, so
  large dark and light shapes can be judged without the dither pattern. `a`
  shows it and returns to the previous layer. The minimap shows the mass study
  by default and `M` switches it to the main layer. `--mass-tones palette|mono|warm`
  selects the tones, following the dither palette by default, and `--mass-invert`
  treats light pixels as ink

### Changed

- The minimap frame fits the map instead of a fixed box. A drag that starts on
  the map keeps centering after the pointer leaves it, and a main preview drag
  no longer jumps when the pointer crosses the minimap

- Image is the default card and template fallback. It keeps one dot per source pixel,
  follows rotation and adds margin and bleed around the image. Dot sets its physical
  size for the status and DPI export. Fixed cards keep their existing behavior

- Scale is replaced by card, dot, margin, bleed, zoom and pan controls. Rotation
  turns the image inside fixed card grids, and uncovered areas stay paper
- The status line starts with card size, dot count and dot size, ahead of the
  layer and navigation fields, so they stay visible in 80 columns. Templates save physical
  print settings and ignore old `scale` fields, retaining config version 1

- Faster panning of a zoomed image: Original cells use 256 colors while the
  preview is dragged and return to full color on release, and cells whose two
  halves share a color are drawn as a background space
- Each frame is written in one buffered pass and wrapped in a synchronized
  update, so supporting terminals paint whole frames
- Frames send only the foreground or background color that changed and skip
  the foreground of plain spaces, which cuts output at 100% zoom by about a
  quarter

### Removed

- `D` no longer resets all parameters and composition. `d` still resets the
  selected parameter

### Fixed

- The hidden cursor stays on the status line between frames, so terminals that
  mark the cursor row, like iTerm2 with its cursor guide, no longer show that
  mark jumping around the preview
- Cells next to a one-column inline image, such as a very tall image at Fit,
  are redrawn instead of keeping the previous image
- Resizing the window in iTerm2 no longer leaves copies of the rthid screen in
  the shell's scrollback after quitting
- Original at Fit is visible again in iTerm2, which answered the Kitty graphics
  query but drew nothing for its placeholder cells, so the preview went blank
  at Fit and after zooming back out; iTerm2 now uses its own image protocol
- Saving the same preset or template value again while an earlier save was
  still running could drop the newer edit, and quitting right after it left
  another instance's value in the config file
- A config save stalled before the file lock could overwrite the value written
  on quit once shutdown stopped waiting for it
- Errors resolved while an image was loading, such as an invalid preset name
  that was then corrected, no longer reappear after the image opens
- Config save failures stay on the status line until a save succeeds instead
  of disappearing behind another message or a key press
- Listing errors from an open dialog that was already closed are ignored

## [0.1.0] - 2026-11-26

### Added

- Decode, directory listing, grid sync, config save and PNG encode timings in
  `RTHID_DEBUG_LOG`

- Optional image argument with an empty start screen and image drag and drop
  through terminal bracketed paste
- `o` to browse directories or type an image path, with home directory expansion
  and errors that keep the dialog open

- Palette preset manager (`P`) and settings template manager (`T`), saved to a
  configurable TOML file with overwrite and removal confirmation for templates
- Unified preview zoom from Fit through 25%, 33%, 50%, 100%, 200%, 300%, and
  400%, with global `+` / `=` / `-`, `0` for Fit, and center preservation
- Panning and a dimmed minimap whenever the zoomed image overflows the preview

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

### Changed

- Replaced the Overview / 1:1 toggle and `z` with zoom levels that skip scales no
  larger than Fit, with crisp enlarged dots and aspect corrected reduced scales
- Minimap toggle `m` now works in either focus. Mouse and keyboard navigation
  follow the current zoom, and terminal images are used only for Original at Fit
- Preview status and help show zoom controls and inclusive dot ranges
- Config saves run in the background, so the UI no longer waits on the config
  lock or disk sync

### Fixed

- Large grids no longer alias the dither pattern in the preview ([#13])
- `Esc` returns focus to the parameter panel instead of quitting ([#13])
- Overview no longer shows `?` boxes in terminals without sextant glyphs ([#13])
- Quitting waits for the final preview PNG and reports write failures ([#13])
- `--preview-png` refuses to overwrite the source image ([#13])

[#13]: https://github.com/notJoon/rthid/pull/13
