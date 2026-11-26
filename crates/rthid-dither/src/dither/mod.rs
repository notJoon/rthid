//! Palette indices from error diffusion or thresholds.
//!
//! Atkinson is the default and spreads only 3/4 of the error to keep highlights
//! and shadows clean. Tone mode quantizes luminance. Color mode picks the
//! nearest palette color after diffusion or a threshold offset.
//!
//! # Two independent choices
//!
//! [`Algorithm`] decides how the pattern is made, from quantization error or
//! from thresholds. [`Mode`] decides what a pixel is compared against to pick a
//! palette index. Every algorithm supports both modes, so for example Floyd
//! Steinberg can run with Color and Bayer 4 with Tone.
//!
//! # Two execution paths
//!
//! The diffusion kernels share one path where each pixel passes its error to
//! pixels not yet visited, so the result depends on visiting order. Bayer and
//! Noise share another path that only reads a threshold per position and never
//! changes neighbors. Both paths share preprocessing and the palette selection
//! rule.
//!
//! # Data flow
//!
//! A [`dither`] call validates the input and numeric options, returns all zeros
//! for a single color palette, normalizes palette colors when Color mode needs
//! them, converts RGB to `f32` channel buffers with [`Tone`] applied, collapses
//! them to luma for Tone mode, then runs the selected path. The result is one
//! palette index per pixel in row major order.
//!
//! Inputs are borrowed and never mutated. Diffusion only changes buffers owned by
//! the call, so comparing algorithms on the same input leaves no error behind.
//! Alpha is ignored and transparent pixels keep their RGB, so composite onto a
//! background before calling if that matters.

mod diffusion;
mod threshold;

pub use diffusion::{Diffusion, Kernel};
pub use threshold::bayer::BayerSize;

/// A palette color in byte channels.
///
/// The bytes are the only stored form. Color mode derives normalized values once
/// per call, so there is no second copy for callers to keep in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaletteColor {
    /// Bytes used to paint the preview and the PNG.
    pub rgb: [u8; 3],
}

/// Per channel adjustments applied before dithering, in the order sharpen,
/// contrast, brightness, gamma, then black and white points. The defaults leave
/// pixels unchanged. All values must be finite and gamma must be positive.
///
/// These are preprocessing settings, unrelated to [`Mode::Tone`], which is a
/// quantization mode. Black and white closer than 0.02, or reversed, are not an
/// error. The span is floored at 0.02 instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tone {
    /// Offset added after contrast, with the result clipped to 0..=1.
    pub brightness: f64,
    /// Scales the distance from middle gray by `1 + contrast`, so 0 keeps it.
    pub contrast: f64,
    /// Positive gamma. Channels are raised to `1 / gamma`, so values above 1 brighten midtones.
    pub gamma: f64,
    /// Level mapped to 0 after gamma. Darker values clip to black.
    pub black: f64,
    /// Level mapped to 1 after gamma. Lighter values clip to white. The span from
    /// black to white is floored at 0.02.
    pub white: f64,
    /// Unsharp mask strength against a 3x3 box blur. 0 disables it.
    pub sharpen: f64,
}

impl Default for Tone {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 0.0,
            gamma: 1.0,
            black: 0.0,
            white: 1.0,
            sharpen: 0.0,
        }
    }
}

/// Color model used to quantize, chosen independently of [`Algorithm`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Quantizes luminance to evenly spaced levels with diffusion or thresholds.
    /// Palette colors do not affect the choice, so use evenly spaced brightness.
    ///
    /// With N colors, luma maps to N - 1 equal steps and index 0 is the darkest
    /// step. This assigns ink levels by brightness rather than finding
    /// the closest color, so it suits palettes sorted darkest first with even gaps.
    #[default]
    Tone,
    /// Picks the nearest palette color after RGB diffusion or a threshold offset.
    /// Squared distance weights red, green and blue by 2, 4 and 3.
    ///
    /// Ties keep the earlier palette index. The distance is a fixed weighted RGB
    /// metric, with no Lab distance or color management.
    Color,
}

/// How pixels become palette indices. Defaults to `Diffusion::default()`.
///
/// Each variant carries only the settings it uses. Strength and serpentine live in
/// [`Diffusion`], so a Bayer strength or a Noise serpentine cannot be expressed
/// and no setting is silently ignored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Algorithm {
    /// Passes quantization error to neighboring pixels.
    Diffusion(Diffusion),
    /// Ordered dithering with a recursive Bayer matrix. Has no serpentine or
    /// strength and scans rows left to right.
    Bayer(BayerSize),
    /// Random thresholds from a seeded generator, reproducible for the same seed.
    /// Has no serpentine or strength and scans rows left to right.
    ///
    /// Each threshold depends only on the seed and the row major pixel index, so
    /// the same input, size, options and seed always give the same output.
    Noise {
        /// Selects the threshold sequence. Any `u64` is valid.
        seed: u64,
    },
}

impl Default for Algorithm {
    fn default() -> Self {
        Self::Diffusion(Diffusion::default())
    }
}

/// Palette and processing settings for dithering. Defaults to a black and white
/// palette, Tone mode, `Algorithm::default()` and default tone.
///
/// The default algorithm is serpentine Atkinson at strength 1, even though
/// [`BayerSize`] on its own defaults to Four.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// Colors referenced by output indices. Tone mode requires nondecreasing
    /// luma, with ties allowed, as built by `make_palette`. Color mode accepts
    /// any order.
    pub palette: Vec<PaletteColor>,
    /// Color model used to quantize.
    pub mode: Mode,
    /// Method used to quantize pixels.
    pub algorithm: Algorithm,
    /// Adjustments applied before quantization.
    pub tone: Tone,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            palette: make_palette(&["#000", "#fff"]).unwrap(),
            mode: Mode::Tone,
            algorithm: Algorithm::default(),
            tone: Tone::default(),
        }
    }
}

// Rec. 601 luma.
fn luma([r, g, b]: [f64; 3]) -> f64 {
    0.299 * r + 0.587 * g + 0.114 * b
}

fn hex_to_rgb(hex: &str) -> Result<[u8; 3], &'static str> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if !matches!(hex.len(), 3 | 6) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("expected a 3- or 6-digit hex color");
    }
    let s = hex.len() / 3;
    let scale = if s == 1 { 17 } else { 1 };
    Ok(std::array::from_fn(|i| {
        u8::from_str_radix(&hex[i * s..(i + 1) * s], 16).unwrap() * scale
    }))
}

/// Invalid palette input, preserving the count or color that failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteError {
    /// The palette must contain 1 to 256 colors.
    Count {
        /// Number of colors supplied.
        len: usize,
    },
    /// A color was not valid hex.
    Parse {
        /// Zero based position in the input.
        index: usize,
        /// Original input text.
        value: String,
    },
}

impl std::fmt::Display for PaletteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Count { .. } => write!(f, "palette must have 1 to 256 colors"),
            Self::Parse { index, value } => write!(
                f,
                "color {} ({value:?}): expected a 3- or 6-digit hex color",
                index + 1
            ),
        }
    }
}

impl std::error::Error for PaletteError {}

/// Parses 1 to 256 hex colors (`#rgb` or `#rrggbb`, `#` optional) and sorts them
/// darkest first by luma, so the last, lightest color acts as paper.
/// Returns [`PaletteError`] for invalid counts or colors. Invalid colors retain
/// their input text and zero based position, displayed as a 1 based position.
///
/// Dither indices refer to this sorted order, not the input order. Callers who
/// edit `rgb` afterwards must keep nondecreasing luma for Tone mode, which
/// [`dither`] checks.
pub fn make_palette(hexes: &[&str]) -> Result<Vec<PaletteColor>, PaletteError> {
    if hexes.is_empty() || hexes.len() > 256 {
        return Err(PaletteError::Count { len: hexes.len() });
    }
    let mut palette = hexes
        .iter()
        .enumerate()
        .map(|(index, hex)| {
            hex_to_rgb(hex)
                .map(|rgb| PaletteColor { rgb })
                .map_err(|_| PaletteError::Parse {
                    index,
                    value: (*hex).into(),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let lum = |p: &PaletteColor| luma(p.rgb.map(f64::from)) / 255.0;
    palette.sort_by(|a, b| lum(a).total_cmp(&lum(b)));
    Ok(palette)
}

fn to_channels(rgba: &[u8], w: usize, h: usize, tone: &Tone) -> [Vec<f32>; 3] {
    let n = rgba.len() / 4;
    let mut channels = [vec![0.0; n], vec![0.0; n], vec![0.0; n]];
    let cf = 1.0 + tone.contrast;
    let span = (tone.white - tone.black).max(0.02);
    let g = 1.0 / tone.gamma;
    for (c, channel) in channels.iter_mut().enumerate() {
        for (value, pixel) in channel.iter_mut().zip(rgba.as_chunks::<4>().0) {
            *value = (f64::from(pixel[c]) / 255.0) as f32;
        }
        if tone.sharpen > 0.0 {
            let blurred = box_blur3(channel, w, h);
            for (value, blur) in channel.iter_mut().zip(blurred) {
                let v = f64::from(*value);
                *value = (v + tone.sharpen * (v - f64::from(blur))) as f32;
            }
        }
        for value in channel {
            let v = ((f64::from(*value) - 0.5) * cf + 0.5 + tone.brightness).clamp(0.0, 1.0);
            *value = ((v.powf(g) - tone.black) / span).clamp(0.0, 1.0) as f32;
        }
    }
    channels
}

fn box_blur3(channel: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut blurred = vec![0.0; channel.len()];
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0;
            let mut count = 0;
            for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    sum += f64::from(channel[yy * w + xx]);
                    count += 1;
                }
            }
            blurred[y * w + x] = (sum / f64::from(count)) as f32;
        }
    }
    blurred
}

// Checks every array and numeric contract up front so the loops never index out
// of range or quietly produce wrong output. Returns the pixel count. Panic
// messages carry the actual and expected values, and track_caller points them at
// the public entry point's caller.
#[track_caller]
fn validate(rgba: &[u8], w: usize, h: usize, options: &Options) -> usize {
    let palette = &options.palette;
    assert!(
        !palette.is_empty() && palette.len() <= 256,
        "palette must have 1 to 256 colors, got {}",
        palette.len()
    );
    let n = w.checked_mul(h).expect("image size overflow");
    let bytes = n.checked_mul(4).expect("image size overflow");
    assert!(
        rgba.len() == bytes,
        "rgba has {} bytes, expected {w}x{h}x4 = {bytes}",
        rgba.len()
    );
    // Tone mode maps luma steps to indices darkest first, whatever the algorithm,
    // so every Tone path relies on this order.
    if options.mode == Mode::Tone {
        for (index, pair) in palette.windows(2).enumerate() {
            assert!(
                luma(pair[0].rgb.map(f64::from)) / 255.0
                    <= luma(pair[1].rgb.map(f64::from)) / 255.0,
                "Tone palette is out of order at index {}",
                index + 1
            );
        }
    }
    match options.algorithm {
        Algorithm::Diffusion(d) => {
            assert!(
                d.strength.is_finite(),
                "algorithm strength must be finite, got {}",
                d.strength
            );
            assert!(
                (0.0..=1.0).contains(&d.strength),
                "algorithm strength must be within 0..=1, got {}",
                d.strength
            );
        }
        Algorithm::Bayer(_) | Algorithm::Noise { .. } => {}
    }
    for (name, value) in [
        ("brightness", options.tone.brightness),
        ("contrast", options.tone.contrast),
        ("gamma", options.tone.gamma),
        ("black", options.tone.black),
        ("white", options.tone.white),
        ("sharpen", options.tone.sharpen),
    ] {
        assert!(value.is_finite(), "tone.{name} must be finite, got {value}");
    }
    assert!(
        options.tone.gamma > 0.0,
        "tone.gamma must be positive, got {}",
        options.tone.gamma
    );
    n
}

// Tone index for luma `v` among `levels + 1` evenly spaced steps. Diffusion
// passes 0.5 to round and the threshold path passes its threshold. Both paths
// call this and `nearest` so the selection rule lives in one place.
fn tone_level(v: f64, levels: f64, offset: f64) -> usize {
    (v * levels + offset).floor().clamp(0.0, levels) as usize
}

// Color index with the smallest 2, 4, 3 weighted squared RGB distance. The strict
// comparison keeps the earliest index on ties.
fn nearest(px: [f64; 3], colors: &[[f64; 3]]) -> usize {
    let mut nearest = 0;
    let mut best_distance = f64::INFINITY;
    for (j, color) in colors.iter().enumerate() {
        let [dr, dg, db] = std::array::from_fn(|c| px[c] - color[c]);
        let distance = 2.0 * dr * dr + 4.0 * dg * dg + 3.0 * db * db;
        if distance < best_distance {
            best_distance = distance;
            nearest = j;
        }
    }
    nearest
}

/// Returns one index into `options.palette` per pixel, in row major order. Alpha
/// is ignored.
///
/// Panics if the palette is empty or has more than 256 colors, or if `rgba` is
/// not exactly `w` by `h` pixels, if the image byte count overflows, if any
/// numeric option is not finite, or if gamma is not positive or algorithm
/// strength is outside 0..=1. Tone mode also panics at the first palette index
/// whose luma is lower than the previous color's luma, with every algorithm.
/// Palette indices are zero based and equal luma is allowed. Panics report the
/// caller's location. An empty image that meets these contracts returns an empty
/// vector.
#[track_caller]
pub fn dither(rgba: &[u8], w: usize, h: usize, options: &Options) -> Vec<u8> {
    let n = validate(rgba, w, h, options);
    let palette = &options.palette;
    // One color needs no work, and tone mode would divide by zero levels. This
    // runs after validation so a single color palette cannot hide invalid options.
    if palette.len() == 1 {
        return vec![0; n];
    }
    // Normalized palette for Color mode, built once per call. Tone mode never
    // reads palette colors.
    let colors: Vec<_> = match options.mode {
        Mode::Tone => Vec::new(),
        Mode::Color => palette
            .iter()
            .map(|color| color.rgb.map(|value| f64::from(value) / 255.0))
            .collect(),
    };
    let mut channels = to_channels(rgba, w, h, &options.tone);
    let levels = (palette.len() - 1) as f64;
    // Tone mode quantizes luminance alone.
    let work = match options.mode {
        Mode::Tone => {
            // Luminance replaces the red buffer, rounded to f32 like every stored value.
            let [red, green, blue] = &mut channels;
            for ((r, g), b) in red.iter_mut().zip(green).zip(blue) {
                *r = luma([*r, *g, *b].map(f64::from)) as f32;
            }
            &mut channels[..1]
        }
        Mode::Color => &mut channels[..],
    };
    // Diffusion mutates `work` in place, while thresholds only read it.
    match options.algorithm {
        Algorithm::Diffusion(d) => {
            diffusion::diffuse(work, w, h, options.mode, levels, &colors, &d)
        }
        Algorithm::Bayer(size) => threshold::threshold(
            work,
            w,
            h,
            options.mode,
            levels,
            &colors,
            threshold::bayer::threshold(size),
        ),
        Algorithm::Noise { seed } => threshold::threshold(
            work,
            w,
            h,
            options.mode,
            levels,
            &colors,
            threshold::noise::threshold(seed, w),
        ),
    }
}

/// Convenience helper to turn indices into opaque RGBA for previews and examples.
/// Both `grid` and `out` are `(width, height)`. Uses nearest neighbor sampling.
/// Returns an empty vector if either output dimension is zero.
///
/// Panics if the grid is empty with nonempty output, if the grid size overflows
/// or does not match `idx.len()`, or if the output byte count overflows. Panics
/// report the caller's location. `color_of` must accept every index in `idx`. It
/// is called for sampled indices and any panic inside it propagates.
#[track_caller]
pub fn rasterize(
    idx: &[u8],
    grid: (usize, usize),
    out: (usize, usize),
    color_of: impl Fn(u8) -> [u8; 3],
) -> Vec<u8> {
    let (gw, gh) = grid;
    let (ow, oh) = out;
    let n = gw.checked_mul(gh).expect("grid size overflow");
    assert!(
        idx.len() == n,
        "idx has {} indices, expected {gw}x{gh} = {n}",
        idx.len()
    );
    let size = ow
        .checked_mul(oh)
        .and_then(|n| n.checked_mul(4))
        .expect("output size overflow");
    if size == 0 {
        return Vec::new();
    }
    assert!(
        gw > 0 && gh > 0,
        "source dimensions must be positive, got {gw}x{gh}"
    );
    let mut rgba = vec![0; size];
    for y in 0..oh {
        let row = y * gh / oh * gw;
        for x in 0..ow {
            let color = color_of(idx[row + x * gw / ow]);
            let offset = (y * ow + x) * 4;
            rgba[offset..offset + 3].copy_from_slice(&color);
            rgba[offset + 3] = 255;
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn algorithms_return_valid_indices_for_small_images() {
        let colors = ["#000", "#345", "#789", "#abc", "#fff"];
        for algorithm in [
            Kernel::Atkinson,
            Kernel::FloydSteinberg,
            Kernel::JarvisJudiceNinke,
            Kernel::Stucki,
            Kernel::SierraLite,
        ]
        .map(|kernel| {
            Algorithm::Diffusion(Diffusion {
                kernel,
                ..Diffusion::default()
            })
        })
        .into_iter()
        .chain([
            Algorithm::Bayer(BayerSize::Two),
            Algorithm::Bayer(BayerSize::Four),
            Algorithm::Bayer(BayerSize::Eight),
            Algorithm::Noise { seed: 0 },
        ]) {
            for mode in [Mode::Tone, Mode::Color] {
                for (w, h) in [(1, 1), (7, 1), (1, 7), (0, 0), (0, 3)] {
                    let rgba: Vec<_> = (0..w * h)
                        .flat_map(|i| [((i * 53 + 71) % 256) as u8, 137, 211, 255])
                        .collect();
                    for count in 1..=5 {
                        let options = Options {
                            palette: make_palette(&colors[..count]).unwrap(),
                            mode,
                            algorithm,
                            ..Options::default()
                        };
                        let indices = dither(&rgba, w, h, &options);
                        assert_eq!(indices.len(), w * h);
                        assert!(
                            indices.iter().all(|&index| usize::from(index) < count),
                            "{algorithm:?} {mode:?} {w}x{h} palette {count}"
                        );
                        if count == 1 {
                            assert_eq!(indices, vec![0; w * h]);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn parses_hex_colors() {
        for hex in ["#aBc", "aBc", "#aAbBcC", "aAbBcC"] {
            assert_eq!(hex_to_rgb(hex), Ok([170, 187, 204]));
        }
        for hex in ["", "#abcd", "#ggg", "#가"] {
            assert!(hex_to_rgb(hex).is_err());
        }
    }

    #[test]
    fn sorts_palette_by_luminance() {
        let palette = make_palette(&["#fff", "#0f0", "#00f", "#000", "#f00"]).unwrap();
        assert_eq!(
            palette.iter().map(|p| p.rgb).collect::<Vec<_>>(),
            [[0, 0, 0], [0, 0, 255], [255, 0, 0], [0, 255, 0], [255; 3]]
        );
    }

    #[test]
    fn palette_errors_identify_the_invalid_color() {
        let error = make_palette(&["#000", "badhex", "#fff"]).unwrap_err();
        assert_eq!(
            error,
            PaletteError::Parse {
                index: 1,
                value: "badhex".into(),
            }
        );
        assert_eq!(
            error.to_string(),
            "color 2 (\"badhex\"): expected a 3- or 6-digit hex color"
        );
        for hexes in [vec![], vec!["#000"; 257]] {
            let error = make_palette(&hexes).unwrap_err();
            assert_eq!(error, PaletteError::Count { len: hexes.len() });
            assert_eq!(error.to_string(), "palette must have 1 to 256 colors");
        }
    }

    #[test]
    #[should_panic(expected = "Tone palette is out of order at index 2")]
    fn tone_rejects_the_first_out_of_order_color() {
        let mut options = Options::default();
        options.palette.push(PaletteColor { rgb: [128; 3] });
        options.palette.push(PaletteColor { rgb: [0; 3] });
        dither(&[0; 4], 1, 1, &options);
    }

    #[test]
    fn tone_order_check_covers_every_algorithm() {
        for algorithm in [
            Algorithm::default(),
            Algorithm::Bayer(BayerSize::Two),
            Algorithm::Noise { seed: 0 },
        ] {
            let options = Options {
                palette: vec![PaletteColor { rgb: [255; 3] }, PaletteColor { rgb: [0; 3] }],
                algorithm,
                ..Options::default()
            };
            assert!(std::panic::catch_unwind(|| dither(&[0; 4], 1, 1, &options)).is_err());
            let color = Options {
                mode: Mode::Color,
                ..options
            };
            assert_eq!(dither(&[0; 4], 1, 1, &color).len(), 1);
        }
    }

    #[test]
    fn tone_accepts_sorted_colors_and_luma_ties() {
        let options = Options {
            palette: make_palette(&["#fff", "#0f0", "#00f", "#000", "#f00", "#f00"]).unwrap(),
            ..Options::default()
        };
        assert_eq!(dither(&[0; 4], 1, 1, &options), [0]);
    }

    #[test]
    #[should_panic(expected = "rgba has 3 bytes, expected 1x1x4 = 4")]
    fn rgba_mismatch_reports_actual_and_expected_size() {
        dither(&[0; 3], 1, 1, &Options::default());
    }

    #[test]
    fn color_mode_uses_edited_rgb_values() {
        let mut options = Options {
            mode: Mode::Color,
            ..Options::default()
        };
        let white = [255; 4];
        assert_eq!(dither(&white, 1, 1, &options), [1]);
        options.palette[0].rgb = [255; 3];
        options.palette[1].rgb = [0; 3];
        let indices = dither(&white, 1, 1, &options);
        assert_eq!(indices, [0]);
        assert_eq!(
            rasterize(&indices, (1, 1), (1, 1), |i| options.palette[i as usize]
                .rgb),
            white
        );
    }

    #[test]
    fn rejects_invalid_numeric_options_even_for_one_color() {
        let setters: [fn(&mut Options, f64); 7] = [
            |options, value| {
                options.algorithm = Algorithm::Diffusion(Diffusion {
                    strength: value,
                    ..Diffusion::default()
                });
            },
            |options, value| options.tone.brightness = value,
            |options, value| options.tone.contrast = value,
            |options, value| options.tone.gamma = value,
            |options, value| options.tone.black = value,
            |options, value| options.tone.white = value,
            |options, value| options.tone.sharpen = value,
        ];
        for colors in [&["#000"][..], &["#000", "#fff"][..]] {
            let options = Options {
                palette: make_palette(colors).unwrap(),
                ..Options::default()
            };
            for set in setters {
                for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                    let mut invalid = options.clone();
                    set(&mut invalid, value);
                    assert!(std::panic::catch_unwind(|| dither(&[0; 4], 1, 1, &invalid)).is_err());
                }
            }
            for gamma in [0.0, -1.0] {
                let mut invalid = options.clone();
                invalid.tone.gamma = gamma;
                assert!(std::panic::catch_unwind(|| dither(&[0; 4], 1, 1, &invalid)).is_err());
            }
        }
    }

    #[test]
    #[should_panic(expected = "algorithm strength must be within 0..=1, got -0.1")]
    fn rejects_negative_strength() {
        let options = Options {
            algorithm: Algorithm::Diffusion(Diffusion {
                strength: -0.1,
                ..Diffusion::default()
            }),
            ..Options::default()
        };
        dither(&[0; 4], 1, 1, &options);
    }

    #[test]
    #[should_panic(expected = "algorithm strength must be within 0..=1, got 1.1")]
    fn rejects_strength_above_one() {
        let options = Options {
            algorithm: Algorithm::Diffusion(Diffusion {
                strength: 1.1,
                ..Diffusion::default()
            }),
            ..Options::default()
        };
        dither(&[0; 4], 1, 1, &options);
    }

    #[test]
    #[should_panic(expected = "algorithm strength must be finite, got NaN")]
    fn rejects_nan_strength() {
        let options = Options {
            algorithm: Algorithm::Diffusion(Diffusion {
                strength: f64::NAN,
                ..Diffusion::default()
            }),
            ..Options::default()
        };
        dither(&[0; 4], 1, 1, &options);
    }

    #[test]
    #[should_panic(expected = "palette must have 1 to 256 colors, got 0")]
    fn reports_empty_palette() {
        dither(
            &[],
            0,
            0,
            &Options {
                palette: vec![],
                ..Options::default()
            },
        );
    }

    #[test]
    #[should_panic(expected = "tone.gamma must be positive, got 0")]
    fn reports_nonpositive_gamma() {
        let mut options = Options::default();
        options.tone.gamma = 0.0;
        dither(&[0; 4], 1, 1, &options);
    }

    #[test]
    #[should_panic(expected = "tone.brightness must be finite, got inf")]
    fn reports_nonfinite_tone() {
        let mut options = Options::default();
        options.tone.brightness = f64::INFINITY;
        dither(&[0; 4], 1, 1, &options);
    }

    #[test]
    fn solid_black_and_white_use_palette_endpoints() {
        let mut options = Options {
            palette: make_palette(&["#000", "#555", "#aaa", "#fff"]).unwrap(),
            ..Options::default()
        };
        for mode in [Mode::Tone, Mode::Color] {
            options.mode = mode;
            for (value, index) in [(0, 0), (255, 3)] {
                let rgba = [value, value, value, 255].repeat(32);
                assert_eq!(dither(&rgba, 8, 4, &options), vec![index; 32]);
            }
        }
    }

    #[test]
    fn single_color_fills_the_grid_in_both_modes() {
        let mut options = Options {
            palette: make_palette(&["#123"]).unwrap(),
            ..Options::default()
        };
        for mode in [Mode::Tone, Mode::Color] {
            options.mode = mode;
            assert_eq!(dither(&gradient(), 16, 8, &options), vec![0; 128]);
        }
    }

    #[test]
    fn rejects_invalid_rgba_and_overflow_for_every_palette_size() {
        for colors in [&["#000"][..], &["#000", "#fff"][..]] {
            let options = Options {
                palette: make_palette(colors).unwrap(),
                ..Options::default()
            };
            for (rgba, w, h) in [
                (&[][..], 2, 2),
                (&[0; 3][..], 1, 1),
                (&[][..], usize::MAX / 2 + 1, 2),
                (&[][..], usize::MAX / 4 + 1, 1),
            ] {
                assert!(std::panic::catch_unwind(|| dither(rgba, w, h, &options)).is_err());
            }
        }
    }

    /// Shared pixels for quantization checks.
    pub(crate) fn gradient() -> Vec<u8> {
        (0..128)
            .flat_map(|i| {
                let (x, y) = (i % 16, i / 16);
                [
                    ((x * 16 + y * 7) % 256) as u8,
                    ((x * 11 + y * 29) % 256) as u8,
                    ((x * 23 + y * 13) % 256) as u8,
                    255,
                ]
            })
            .collect()
    }

    #[test]
    fn tone_matches_golden() {
        let options = Options {
            palette: make_palette(&["#fff", "#222", "#aaa", "#555"]).unwrap(),
            ..Options::default()
        };
        assert_eq!(dither(&gradient(), 16, 8, &options), TONE_GOLDEN);
    }

    #[test]
    fn color_matches_golden() {
        let options = Options {
            palette: make_palette(&["#f3e8c8", "#172438", "#c45238"]).unwrap(),
            mode: Mode::Color,
            algorithm: Algorithm::Diffusion(Diffusion {
                kernel: Kernel::Atkinson,
                serpentine: false,
                strength: 0.8,
            }),
            tone: Tone {
                contrast: 0.15,
                sharpen: 0.6,
                black: 0.04,
                white: 0.95,
                ..Tone::default()
            },
        };
        assert_eq!(dither(&gradient(), 16, 8, &options), COLOR_GOLDEN);
    }

    #[test]
    fn tone_adjustments_match_expected_channels() {
        let tone = Tone {
            brightness: 0.05,
            contrast: 0.5,
            gamma: 2.0,
            black: 0.1,
            white: 0.9,
            ..Tone::default()
        };
        assert_eq!(
            to_channels(&[0, 51, 255, 0], 1, 1, &tone),
            [vec![0.0], vec![0.270_284_7], vec![1.0]]
        );
    }

    #[test]
    fn rasterize_scales_nearest_neighbor() {
        let result = rasterize(&[0, 1], (2, 1), (4, 2), |i| {
            if i == 0 {
                [10, 20, 30]
            } else {
                [200, 210, 220]
            }
        });
        let row = [
            10, 20, 30, 255, 10, 20, 30, 255, 200, 210, 220, 255, 200, 210, 220, 255,
        ];
        assert_eq!(result, row.repeat(2));
    }

    #[test]
    fn rasterize_empty_output_accepts_empty_grids() {
        for grid in [(0, 0), (0, 1), (1, 0)] {
            for out in [(0, 0), (0, 2), (2, 0)] {
                assert!(rasterize(&[], grid, out, |_| panic!("empty output")).is_empty());
            }
            assert!(std::panic::catch_unwind(|| rasterize(&[], grid, (1, 1), |_| [0; 3])).is_err());
        }
        assert!(rasterize(&[0], (1, 1), (0, 2), |_| panic!("empty output")).is_empty());
    }

    #[test]
    #[should_panic(expected = "idx has 1 indices, expected 2x3 = 6")]
    fn index_mismatch_reports_actual_and_expected_size() {
        rasterize(&[0], (2, 3), (1, 1), |_| [0; 3]);
    }

    #[test]
    fn rasterize_rejects_size_overflow() {
        for (grid, out) in [
            ((usize::MAX, 2), (1, 1)),
            ((1, 1), (usize::MAX, 2)),
            ((1, 1), (usize::MAX / 4 + 1, 1)),
        ] {
            assert!(std::panic::catch_unwind(|| rasterize(&[0], grid, out, |_| [0; 3])).is_err());
        }
    }

    #[test]
    #[should_panic(expected = "unsupported index 7")]
    fn rasterize_propagates_callback_panics() {
        rasterize(&[7], (1, 1), (1, 1), |i| panic!("unsupported index {i}"));
    }
    const TONE_GOLDEN: [u8; 128] = [
        0, 0, 0, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 0, 0, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2,
        2, 2, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2,
        2, 3, 3, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 1, 1, 0, 1, 1, 1, 2, 2, 2, 2, 2, 3, 2,
        3, 1, 1, 1, 0, 1, 2, 2, 2, 2, 2, 2, 2, 3, 1, 1, 1, 1, 1, 2, 1, 1, 2, 2, 2, 2, 2, 1, 1, 1,
        1, 1, 1, 1, 2, 1, 1, 1,
    ];
    const COLOR_GOLDEN: [u8; 128] = [
        0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 2, 2, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1, 2, 2, 2, 2, 1, 1, 1,
        2, 2, 0, 0, 0, 0, 0, 1, 2, 2, 0, 2, 1, 1, 2, 2, 2, 2, 0, 0, 0, 0, 2, 2, 0, 2, 2, 2, 1, 1,
        2, 2, 2, 2, 0, 0, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 1, 1, 0, 0, 0, 2, 2, 0, 2, 2, 2, 2, 2,
        2, 1, 1, 1, 0, 0, 0, 2, 2, 2, 2, 2, 2, 2, 0, 1, 1, 1, 1, 1, 0, 0, 2, 2, 0, 2, 2, 0, 0, 0,
        1, 1, 1, 1, 1, 0, 0, 2,
    ];
}
