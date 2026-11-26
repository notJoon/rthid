# Changelog

All notable changes to rthid-dither are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.2.0] - Unreleased

### Breaking

- Move rasterization: `rthid_dither::dither::rasterize` -> `rthid_render::rasterize`

- Move `serpentine` and `strength` from `Options` into the `Diffusion` payload of
  `Algorithm::Diffusion`. Atkinson stays the default with unchanged output.

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

- `PaletteColor::f` removed. Migration: `color.f` -> `color.rgb.map(|v| f64::from(v) / 255.0)`
- `make_palette` returns `PaletteError`. Migration: string errors -> match `Count` or `Parse`, or call `error.to_string()`
- `rasterize` takes size tuples. Migration: `rasterize(idx, gw, gh, ow, oh, color_of)` -> `rasterize(idx, (gw, gh), (ow, oh), color_of)`
- Tone mode panics on an unsorted palette with every algorithm. Migration: arbitrary palette order -> nondecreasing luma order from `make_palette`

### Added

- Bayer 2, 4 and 8 ordered dithering and seeded noise thresholds
- Floyd Steinberg, Jarvis Judice Ninke, Stucki and Sierra Lite kernels selected
  through `Algorithm`
- `Copy`, `Eq`, and `Hash` for `PaletteColor`, plus `Eq` and `Hash` for `Mode`
- `PaletteError` with count and parse variants, `Display`, and `std::error::Error`, exported from the crate root

### Changed

- Reject algorithm strength outside `0.0..=1.0`
- Report actual and expected values in clearer panic messages with caller locations
- Return an empty raster for zero output width or height, including with an empty grid
- Include actual lengths and expected dimensions in RGBA and index mismatch panics
- Document rasterization size checks and callback panic propagation
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
