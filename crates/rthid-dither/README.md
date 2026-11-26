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
needed. Input is row major RGBA bytes and output is one palette index per pixel:

```rust
use rthid_dither::dither::{Options, dither};

fn main() {
    let rgba = [0, 0, 0, 255, 255, 255, 255, 255];
    let options = Options::default();
    let indices = dither(&rgba, 2, 1, &options);
    assert_eq!(indices, [0, 1]);
}
```

Defaults use a black and white palette, Tone mode, serpentine Atkinson error
diffusion at strength 1, and no tone adjustment.

Use [rthid-render](../rthid-render) to rasterize indices and encode PNG output.

## Choosing an algorithm

`Options` holds two independent choices. `algorithm` decides how the pattern is
made and `mode` decides how a palette index is picked. Every algorithm works
with both `Mode::Tone` and `Mode::Color`.

Nine algorithms come in two families:

| Family | `Algorithm` | Choices | Settings |
| --- | --- | --- | --- |
| Error diffusion | `Diffusion(Diffusion)` | `Kernel::Atkinson`, `FloydSteinberg`, `JarvisJudiceNinke`, `Stucki`, `SierraLite` | `kernel`, `serpentine`, `strength` |
| Thresholds | `Bayer(BayerSize)` | `BayerSize::Two`, `Four`, `Eight` | matrix size |
| Thresholds | `Noise { seed }` | one per `u64` seed | `seed` |

Error diffusion passes each pixel's quantization error to neighbors that have
not been processed yet. Thresholds compare each pixel with a value that depends
only on its position and never touch neighbors. Bayer and Noise scan every row
left to right and have no strength or serpentine setting, so those values cannot
be passed to them.

Change only the kernel and keep the other diffusion defaults:

```rust
# use rthid_dither::dither::{Algorithm, Diffusion, Kernel, Options};
let options = Options {
    algorithm: Algorithm::Diffusion(Diffusion { kernel: Kernel::Stucki, ..Diffusion::default() }),
    ..Options::default()
};
```

Ordered Bayer thresholds and seeded Noise thresholds:

```rust
# use rthid_dither::dither::{Algorithm, BayerSize, Options, dither};
let bayer = Options { algorithm: Algorithm::Bayer(BayerSize::Eight), ..Options::default() };
let noise = Options { algorithm: Algorithm::Noise { seed: 42 }, ..Options::default() };

let rgba = [128, 128, 128, 255].repeat(16);
assert_eq!(dither(&rgba, 4, 4, &noise), dither(&rgba, 4, 4, &noise));
assert_eq!(dither(&rgba, 4, 4, &bayer).len(), 16);
```

`BayerSize` on its own defaults to `Four`, but `Algorithm::default()` is
Atkinson diffusion.

### Diffusion settings

- **strength**: scales the kernel weights within `0.0..=1.0`. At 1 each kernel
  uses its own weights unchanged. Atkinson then still spreads only 3/4 of the
  error, while the other kernels spread all of it. At 0 no error is spread and
  each pixel is quantized on its own.
- **serpentine**: rows 1, 3, 5 and so on (counting from 0) run right to left
  with the kernel mirrored, so error still flows to unprocessed pixels.
- Neighbors outside the image are skipped and their share is not given to the
  remaining neighbors.

### Tone and Color

`Mode::Tone` converts each pixel to luminance and maps it to evenly spaced
levels, one per palette color from darkest to lightest. It does not search for
the closest palette color, so it suits palettes sorted darkest first with even
brightness gaps. `Mode::Color` picks the nearest palette color by a weighted RGB
distance. These modes are quantization methods. The `Tone` struct is a separate
thing that holds adjustments applied before dithering.

### Noise reproducibility

Each Noise threshold is computed from the seed and the row major pixel index.
No global generator, clock or hidden state is read.

- The same input, size, options and seed always give the same output.
- A new seed picks new thresholds but does not guarantee a different image. A
  solid input or a single color palette can give the same result.
- The image width is part of the pattern, so the same `(x, y)` gets a different
  threshold when the width changes.

## Custom palettes and adjustments

`make_palette` accepts 1 to 256 hex colors and sorts them darkest first. Returned
indices refer to this sorted palette:

```rust
use rthid_dither::dither::{Algorithm, Diffusion, Kernel, Mode, Options, Tone, dither, make_palette};
use rthid_dither::PaletteError;

fn main() -> Result<(), PaletteError> {
    let options = Options {
        palette: make_palette(&["#172438", "#c45238", "#f3e8c8"])?,
        mode: Mode::Color,
        algorithm: Algorithm::Diffusion(Diffusion {
            kernel: Kernel::FloydSteinberg,
            ..Diffusion::default()
        }),
        tone: Tone { gamma: 1.2, ..Tone::default() },
        ..Options::default()
    };
    let indices = dither(&[196, 82, 56, 255], 1, 1, &options);
    assert_eq!(indices.len(), 1);
    Ok(())
}
```

`PaletteColor::rgb` is the source of color data, so edits are reflected on the
next call. Keep palettes darkest first when using Tone mode.
`PaletteColor` supports `Copy`, `Eq`, and `Hash`. `Mode` supports `Eq` and `Hash`.
For normalized channels, use `rgb.map(|v| f64::from(v) / 255.0)` in place of the
removed `PaletteColor::f` field.

## Input requirements and errors

`make_palette` returns `PaletteError::Count { len }` for invalid lengths or
`PaletteError::Parse { index, value }` for invalid colors. The error is exported
from the crate root and the `dither` module and implements `std::error::Error`.
`index` is zero based. Display text retains the input text and its one based
position, so use `error.to_string()` when a string is needed.

`dither` panics on mismatched RGBA dimensions or palettes outside 1 to 256 colors.
All numeric options must be finite, gamma must be positive, and diffusion strength
must be within `0.0..=1.0`. These checks also apply to single color palettes.
Other finite adjustments are accepted without imposing UI specific ranges.
Panic messages report the caller's location.
Tone mode panics at the first palette index with lower luma than its predecessor,
whatever the algorithm, because every Tone path maps luma steps to indices
darkest first. Equal luma is allowed and `make_palette` output always satisfies
this ordering. Color mode accepts any palette order. Image size overflow also
panics.

## Not provided

- Alpha compositing. Alpha is ignored, so composite onto a background first.
- Color management. Color mode uses a fixed weighted RGB distance with no Lab
  distance or profiles.
- Blue noise. Noise is plain random threshold dithering.

See the crate docs (`cargo doc -p rthid-dither --open`) for the API and
[the design notes](../../docs/dither-design.md) for why the engine is built this way.
