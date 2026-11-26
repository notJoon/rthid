# Dither engine design

This note records why `rthid-dither` and the app's dither settings are built the
way they are. For usage and the input contract see
[the crate README](../crates/rthid-dither/README.md).

## Goals and non goals

Goals:

- Keep the default Atkinson usage and its existing output unchanged.
- Run similar error diffusion algorithms by swapping kernel data only.
- Give Bayer and Noise, which do not diffuse error, their own path.
- Let every algorithm work with both Tone and Color.
- Keep settings that mean nothing to an algorithm out of the library types.
- Keep the user's settings in the app when switching algorithms.
- Give reproducible output for the same input and settings.
- Keep UI, file formats and the engine as separate responsibilities.

Non goals:

- No algorithm registration framework, public trait or dynamic plugin point.
  The scope is the enums, private functions and static kernel data the current
  list needs.
- No public custom kernels, preprocessing channels or shared loops.

## Data flow

One `dither` call runs in this order:

1. Validate the input and numeric options.
2. Return all zeros early for a single color palette.
3. Normalize palette colors when Color mode needs them.
4. Convert RGB to `f32` channel buffers and apply the `Tone` adjustments
   (optional unsharp mask from a 3×3 box blur, contrast around mid gray,
   brightness, gamma as a `1/gamma` power, then black and white remapping with a
   minimum gap of 0.02).
5. Reduce to one luminance channel in Tone mode.
6. Run the diffusion or threshold path.
7. Return row major `Vec<u8>` palette indices.

The caller's RGBA bytes and `Options` are only borrowed. Diffusion mutates the
channel buffers built inside the call, so comparing algorithms on the same input
never carries error over from an earlier call. Buffers cover the whole image.
There is no streaming or row buffer variant.

## Two execution paths

| Path | Algorithms | Neighbor access | Scan order |
| --- | --- | --- | --- |
| Diffusion | Atkinson, Floyd Steinberg, Jarvis Judice Ninke, Stucki, Sierra Lite | writes error into pixels not yet processed | top to bottom, optional serpentine |
| Threshold | Bayer 2, 4 and 8, Noise | reads channels only | every row left to right |

Nine choices do not mean nine engines. The five kernels share one diffusion loop
and the four threshold algorithms share one threshold loop. Both reuse the same
preprocessing and the same quantizers (`tone_level` and `nearest`), so a change
to color choice lives in one place.

The paths stay separate because their dependencies differ. Diffusion has a
pixel to pixel dependency, where each result depends on error from earlier
pixels. Thresholds are read only and depend on position alone. Forcing both into
one trait or one loop would add settings and branches that one side ignores.

Module layout:

| File | Role |
| --- | --- |
| `crates/rthid-dither/src/dither/mod.rs` | public types, entry points, validation, palettes, tone adjustment, shared quantizers, path selection |
| `crates/rthid-dither/src/dither/diffusion/mod.rs` | `Diffusion`, `Kernel`, private `Weights`, the diffusion loop |
| `crates/rthid-dither/src/dither/diffusion/*.rs` | one weight table per kernel |
| `crates/rthid-dither/src/dither/threshold/mod.rs` | shared threshold loop for Tone and Color |
| `crates/rthid-dither/src/dither/threshold/bayer.rs` | `BayerSize`, matrix construction, threshold preparation |
| `crates/rthid-dither/src/dither/threshold/noise.rs` | indexed SplitMix64 thresholds |

Implementation modules are private. `Diffusion`, `Kernel` and `BayerSize` are
reexported from `dither`, so callers never import by file layout.

## Algorithm and Mode

The two types answer different questions.

- `Algorithm`: how quantization error or thresholds shape the pattern.
- `Mode`: which value picks the palette index.

Mixing algorithm names into `Mode` would tangle pattern generation with color
choice. As separate types every pairing is valid and each setting keeps one
meaning. All nine algorithms support both modes.

`Mode::Tone` maps luminance (`0.299R + 0.587G + 0.114B`) to N - 1 equal steps and
does not look at the actual palette brightness. `Mode::Color` picks the palette
entry with the smallest `2*dr² + 4*dg² + 3*db²` and keeps the earlier index on
ties. The `Tone` struct is unrelated to `Mode::Tone`. It holds preprocessing
adjustments.

Each `Algorithm` variant carries only its own settings. `Diffusion` holds
`kernel`, `serpentine` and `strength`, `Bayer` holds a `BayerSize` and `Noise`
holds a `u64` seed. A Bayer strength or a Noise serpentine cannot be expressed,
so no value is silently ignored. Moving `strength` and `serpentine` out of
`Options` is the breaking change of 0.2.0.

## Kernels as data

`Weights` is a private type with `taps` and a `divisor`. Each tap is
`(dx, dy, weight)` relative to the current pixel. Kernel files hold only this
data and `diffuse` does the traversal.

| Kernel | Taps | Weight sum | Divisor | Share spread at interior pixels |
| --- | --- | --- | --- | --- |
| Atkinson | 6 | 6 | 8 | 3/4 |
| Floyd Steinberg | 4 | 16 | 16 | 1 |
| Jarvis Judice Ninke | 12 | 48 | 48 | 1 |
| Stucki | 12 | 42 | 42 | 1 |
| Sierra Lite | 3 | 4 | 4 | 1 |

- **Explicit divisor.** The divisor is never derived from the weight sum.
  Atkinson's 6/8 is intended, and normalizing it would change existing output.
  Strength 1 means the kernel's own weights, not the full error.
- **Boundary taps dropped.** Taps outside the image are skipped and their share
  is not redistributed. The table describes interior pixels, not error kept over
  the whole image.
- **Order dependency.** Every tap points right on the same row or to later rows.
  Error only accumulates in pixels not yet processed, so results cannot be
  assumed equal under another order or independent tiles.
- **Serpentine.** With serpentine on, odd rows (counting from 0) run right to
  left and every tap's `dx` is negated. Reversing only the pixel order would
  send error to pixels already processed.

Tone mode diffuses the luminance error in one channel. Color mode diffuses the
RGB difference to the chosen palette color per channel. At strength 0 nothing is
diffused, and Tone still uses even luminance steps while Color still picks the
nearest palette RGB.

## Numeric reproducibility

Output is guarded bit for bit, so arithmetic that looks redundant is kept.

- Channels are stored as `f32`, widened to `f64` for math, and stored back to
  `f32` after every write.
- The error share keeps a different form per mode:

  ```text
  Tone:  e * weight / divisor
  Color: e * (weight / divisor)
  ```

  These are equal in real arithmetic but can differ in the last bit,
  especially for divisors that are not powers of two. One flipped bit can change
  a later quantization choice and ripple across the image. Do not merge them as
  duplicate code.
- Golden index files guard these rules. Any cleanup or optimization must keep
  them passing.

These changes alter output and count as compatibility changes. They need new
goldens and a changelog entry:

- kernel tables (taps, weights or divisors)
- the Tone and Color share forms or the `f32` store points
- the SplitMix64 constants or the index formula
- the Bayer construction or normalization formula
- the Color threshold spread rule
- the luminance or RGB distance weights

## Thresholds

The threshold loop takes a function from `(x, y)` to a threshold in `[0, 1)`
and quantizes with it. The shared quantization is:

```text
Tone:   index = clamp(floor(v * (N - 1) + threshold), 0, N - 1)
Color:  spread = 1.6 / max(1, N - 1)
        offset = (threshold - 0.5) * spread
        add offset to R, G and B, then pick the nearest palette color
```

The Color rule adds the same offset to all three channels. It is not an ordered
quantizer that optimizes the mix between two candidate colors, and no option
reinterprets diffusion strength as a pattern strength.

### Bayer

`BayerSize` only has `Two`, `Four` and `Eight`, so unsupported sizes cannot be
requested at run time. The matrix starts at `[[0]]` and doubles each step. Every
previous rank is multiplied by 4 and offset per quadrant by

```text
0 2
3 1
```

The matrix lives in a fixed 8×8 array with only the needed part filled. Each
threshold is `(rank + 0.5) / (n * n)`, prepared once per call, and the pattern
repeats with `x % n` and `y % n`.

### Noise

Thresholds come from an indexed SplitMix64 mapping, using wrapping `u64`
arithmetic:

```text
i = y * width + x
z = seed + (i + 1) * 0x9E3779B97F4A7C15
z = (z ^ (z >> 30)) * 0xBF58476D1CE4E5B9
z = (z ^ (z >> 27)) * 0x94D049BB133111EB
z = z ^ (z >> 31)
threshold = (z >> 11) / 2^53
```

Each threshold depends on the seed and the pixel index only. Nothing depends on
how many values earlier pixels consumed, and no global generator, clock or
hidden state is read. Because the index includes the width, the pattern is not
independent of image size. The contract is reproducibility for the same input,
size, options and seed. A new seed does not guarantee a different image.

Noise is random threshold dithering. It is not blue noise, cryptographic
randomness or a model of print grain.

## Validation and errors

Invalid numbers and arrays keep the panic contract. `dither` checks everything
at the entry point so loops never hit range errors or silently wrong results:

- palette length is 1 to 256
- `w * h` and the RGBA byte count do not overflow
- `rgba.len()` equals `w * h * 4`
- diffusion strength is finite and within `0.0..=1.0`
- every `Tone` field is finite and gamma is positive

An empty image that meets the contract returns empty indices. A single color
palette returns all zeros only after validation, so it never bypasses the gamma
or strength checks. Messages include the actual and expected values where
possible. `dither` and the validation helpers use `#[track_caller]` so panics
point at the caller. Rasterization now lives in `rthid-render`.

Palette parsing is a user editing path, so `make_palette` returns
`Result<Vec<PaletteColor>, String>` with the invalid text and its one based
position. These two error styles are deliberately not merged.

Validation does not promise that any size can be allocated or that extreme
finite values never overflow internally. UI ranges and resource limits belong to
the app.

## Library options and app Settings

The library should receive only values valid for the current run. The app must
remember edited values across algorithm switches. So the two shapes differ on
purpose.

`Settings` in `src/settings.rs` stores `dither`, `strength`, `serpentine` and
`seed` directly, along with `palette`, `mode` and `tone`. `Settings::options()`
builds `Options` on demand. `Dither::kernel()` is the single mapping from app
choices to `Kernel`, and `Dither::algorithm()` uses only the parameters that
apply. For example, diffusion at strength 0.9 without serpentine switched to
Bayer keeps both values in `Settings`, sends only `Algorithm::Bayer` to the
engine, and rebuilds the same `Diffusion` when switching back.

`Settings` never stores a finished `Options` next to a separate algorithm
choice. That would be duplicated state that could drift from the panel.

The app keeps its own `Dither` enum for order and labels, so the engine has no
ratatui or serde dependency.

### Cache key

`GridCache` in `src/grid.rs` keys the grid by size and rotation, and the dither
result by grid version plus `Settings::options()`. Remembered values that do not
apply to the current algorithm are not in its `Options`, so they never
invalidate the cache. Replacing the key with the whole `Settings`, or adding a
duplicate algorithm state to the app, would break this.

Dithering is deferred while Original is shown without a companion PNG. The
companion PNG reuses the same indices and palette, so Noise is not drawn again
for export.

### Panel behavior

The Dither row cycles Atkinson, Floyd Steinberg, Jarvis, Stucki, Sierra Lite,
Bayer 2, Bayer 4, Bayer 8, Noise and back. Strength and Serpentine are active
only for diffusion and Seed only for Noise. Inactive rows stay visible and
dimmed. Adjusting or resetting an inactive row changes nothing and does not
switch to Dithered.

Seed adjustment saturates at 0 and `u32::MAX` instead of wrapping. Enter on an
active Seed row draws a new seed. The app passes clock based entropy,
`Settings::reseeded` turns it into a seed and adds 1 with wrapping when the
candidate equals the current seed. Reading the clock stays in the app, the seed
rule stays in `Settings`, and the engine only sees a seed, which keeps each part
testable.

The library seed is `u64`. The app edits a `u32` and widens it. Library docs
must not describe the seed as `u32`.

### Templates and file names

Templates store the algorithm, the seed and the remembered diffusion values.
They copy palette colors so later preset edits do not change them, and applying
one keeps the current rotation.

`FileDither` in `src/store.rs` owns the file names:

| App choice | Stored name |
| --- | --- |
| Atkinson | `atkinson` |
| Floyd Steinberg | `floyd-steinberg` |
| Jarvis | `jarvis-judice-ninke` |
| Stucki | `stucki` |
| Sierra Lite | `sierra-lite` |
| Bayer 2, 4, 8 | `bayer2`, `bayer4`, `bayer8` |
| Noise | `noise` |

The short label Jarvis and the stored name `jarvis-judice-ninke` differ on
purpose, and library enum names are not used as the file format. Missing fields
default, so older files read as Atkinson with seed 0. The file seed is read as
`u64` and clamped to `u32::MAX` for the app. Unknown algorithm names are not
mapped to a default. They follow the existing per field parsing, error display
and original text policy. The library never depends on serde.

## Extending

### A new diffusion kernel

1. Add `crates/rthid-dither/src/dither/diffusion/<name>.rs` with a `Weights`
   constant and an explicit divisor.
2. Add a `Kernel` variant with a short doc and map it to the table in
   `diffusion/mod.rs`. Do not copy the diffusion loop.
3. Add the kernel to the table checks in `diffusion/mod.rs`, the small image
   checks in `dither/mod.rs`, and both golden lists in
   `crates/rthid-dither/tests/hsl.rs` with new fixtures.
4. Update the README table and the changelog.

To offer it in the app, also update `Dither` (order, label, `kernel()`),
`FileDither` and its name test in `src/store.rs`, the panel cycle tests in
`src/app.rs`, and the algorithm names in `docs/preview.md`.

### Another algorithm

If it only needs a threshold per position, add a module under `threshold/`
returning a `(x, y)` threshold function and a new `Algorithm` variant that holds
only its own settings. If it needs a different execution model, add a separate
private path rather than bending one of the existing loops or introducing a
shared trait. Tests and app wiring follow the kernel steps above.

## Tests that guard the contracts

| Area | Where | What |
| --- | --- | --- |
| Shared contract | `dither/mod.rs` tests | every algorithm in both modes on small, one row, one column and empty images, output length and index range for several palette sizes, single color palettes, RGBA mismatch and overflow, invalid strength and `Tone` values with exact messages, edited RGB reaching Color mode |
| Diffusion | `diffusion/mod.rs` tests | weight sums and intended share, taps in range and pointing at unprocessed pixels, mirrored second row under serpentine |
| Thresholds | `threshold/` tests | Bayer 2 and 4 ranks, Bayer 8 covering 0 to 63, repeated runs matching, fixed first SplitMix64 value, per seed mapping and `[0, 1)` range |
| Goldens | `tests/hsl.rs` | two tests each run all nine algorithms, 18 index files in total including the original Atkinson `hsl_tone.idx` and `hsl_color.idx`, with Color also covering contrast, black, white and sharpen |
| App and store | `src/app.rs`, `src/settings.rs`, `src/store.rs` | panel cycle both ways, inactive rows ignoring input and keeping values, Seed changes only under Noise, seed saturation and reseed rule, stored names, defaults for older files, seed clamping and round trips, grid and dither caches |

Goldens prove the output is unchanged on the tested platforms. They do not by
themselves establish image quality or results on every platform.

## Out of scope and planned later

The algorithm engine and its app, settings and cache wiring are done.
`rthid-render` owns rasterization and PNG encoding. The app passes an owned
encoder closure to `rthid-io` for atomic replacement. These are later steps:

- Cards, bleed, margins, dot size, composition and exact print sizes from DPI.
- A ZIP export with one plate per ink, a composite PNG and a description file,
  all built from the same indices.
- Captions composited into the indices.

Guidance on choosing algorithms by speed or quality waits for real measurements
and image comparisons.
