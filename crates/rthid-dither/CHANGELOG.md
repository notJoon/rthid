# Changelog

All notable changes to rthid-dither are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.2.0] - Unreleased

### Added

- Bayer 2, 4 and 8 ordered dithering and seeded noise thresholds

- Floyd Steinberg, Jarvis Judice Ninke, Stucki and Sierra Lite kernels selected
  through `Algorithm`

### Changed

- **Breaking**: Move `serpentine` and `strength` from `Options` into the `Diffusion`
  payload of `Algorithm::Diffusion`. Atkinson stays the default with unchanged output.

  Before:
  ```rust,ignore
  Options { serpentine: false, strength: 0.8, ..Options::default() }
  ```

  After:
  ```rust,ignore
  Options {
      algorithm: Algorithm::Diffusion(Diffusion {
          serpentine: false,
          strength: 0.8,
          ..Diffusion::default()
      }),
      ..Options::default()
  }
  ```

- Reject algorithm strength outside `0.0..=1.0`
- Report actual and expected values in clearer panic messages with caller locations
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
