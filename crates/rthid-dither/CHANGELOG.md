# Changelog

All notable changes to rthid-dither are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - Unreleased

### Added

- Atkinson dithering in tone and color modes with serpentine traversal, strength
  and tone adjustments
- Palettes parsed from hex colors and sorted by luminance
- `rasterize`: expands palette indices into RGBA pixels with nearest neighbor sampling
- PNG export with a background writer that writes the latest queued grid
- Atomic file replacement through a temporary file beside the destination
