# Changelog

All notable changes to rthid-dither are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.2.0] - Unreleased

### Changed

- Keep palette RGB bytes as the single source of color data; derive normalized
  channels privately when dithering in Color mode
- Include the color's position and input text in palette parsing errors
- Reject non-finite numeric options and non-positive gamma before processing
- Add a complete quick-start example, compiled and executed as a documentation test

## [0.1.0] - 2026-11-26

### Added

- Atkinson dithering in tone and color modes with serpentine traversal, strength
  and tone adjustments
- Palettes parsed from hex colors and sorted by luminance
- `rasterize`: expands palette indices into RGBA pixels with nearest neighbor sampling
