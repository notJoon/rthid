# rthid-dither

Terminal independent palette dithering used by the rthid binary.

## Quick start

This crate is not published to crates.io. Add it to your app's `Cargo.toml`,
adjusting the path to your checkout:

```toml
[dependencies]
rthid-dither = { path = "../rthid/crates/rthid-dither" }
```

Put this in `src/main.rs` and run `cargo run`. No image decoder or terminal is
needed; input and output are row-major RGBA bytes:

```rust
use rthid_dither::dither::{Options, dither, rasterize};

fn main() {
    let rgba = [0, 0, 0, 255, 255, 255, 255, 255];
    let options = Options::default();
    let indices = dither(&rgba, 2, 1, &options);
    assert_eq!(indices, [0, 1]);

    let output = rasterize(&indices, 2, 1, 2, 1, |index| {
        options.palette[usize::from(index)].rgb
    });
    assert_eq!(output, rgba);
}
```

Defaults use a black-and-white palette, Tone mode, serpentine Atkinson error
diffusion at full strength, and no tone adjustment. `rasterize` expands the
indices to opaque RGBA pixels using nearest-neighbor sampling; its output width
and height may differ from the input grid's dimensions.

## Custom palettes and adjustments

`make_palette` accepts 1 to 256 hex colors and sorts them darkest first. Returned
indices refer to this sorted palette. Tone mode quantizes evenly spaced luminance
levels; use Color mode to choose the nearest actual RGB color:

```rust
use rthid_dither::dither::{Mode, Options, Tone, dither, make_palette};

fn main() -> Result<(), String> {
    let options = Options {
        palette: make_palette(&["#172438", "#c45238", "#f3e8c8"])?,
        mode: Mode::Color,
        tone: Tone { gamma: 1.2, ..Tone::default() },
        ..Options::default()
    };
    let indices = dither(&[196, 82, 56, 255], 1, 1, &options);
    assert_eq!(indices.len(), 1);
    Ok(())
}
```

`PaletteColor::rgb` is the source of color data, so edits are reflected on the
next call. Keep palettes darkest first when using Tone mode. The `rasterize`
color callback can map indices differently without changing dithering.

## Input requirements and errors

`make_palette` returns a `String` error for invalid lengths or colors. Color
errors include the input text and its 1-based position.

`dither` panics on mismatched RGBA dimensions or palettes outside 1 to 256 colors.
All numeric options must be finite, and gamma must be positive. These checks also
apply to single-color palettes. Other finite adjustments are accepted without
imposing UI-specific ranges. Alpha is ignored rather than composited.

`rasterize` panics on an empty or mismatched source grid or overflowing output
dimensions. Its callback must handle every index in the grid.

See the crate docs (`cargo doc -p rthid-dither --open`) for the API.
