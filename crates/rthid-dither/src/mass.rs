//! Flat four tone composition studies that show where the visual weight of an image sits.
//!
//! A mass study is a rough guide to visual mass built from blurred ink density. It is not
//! segmentation. It does not find objects, edges or meaning, and the same pixels and
//! options always give the same study.
//!
//! [`mass_study`] reduces an RGBA image to a small grid of tone indices. Index 0 is the
//! background and 1, 2 and 3 are the base, shadow and accent tiers. Each tier is a cleaned
//! mask nested inside the tier below it, so a cell's index counts how many masks contain
//! it. [`MassStudy::rgba`] then paints the grid at any output size with four tones, taken
//! from [`tones_from_palette`] or the fixed [`MONO`] and [`WARM`] presets.
//!
//! ```text
//! RGBA source, w x h
//!   | luma stretched to the visible range, scaled by alpha
//! ink field in [0, 1]
//!   | averaged down to at most 450 columns, aspect ratio kept
//! density grid
//!   | Gaussian blur, sigma scaled to the grid width
//!   | cut at the base, shadow and accent thresholds
//! three masks: close gaps, keep the largest regions, fill small holes,
//!   | clip each to the tier below, fit ellipses to compact accents
//! tone index grid (MassStudy) --rgba--> opaque RGBA at any size
//! ```
//!
//! Distances such as the blur sigma and the closing radius are tuned at 450 columns and
//! scale with the real grid width. Area limits are fractions of the grid area. A 150 pixel
//! wide source keeps its own width and gets a third of the blur, so its study keeps the
//! same proportions as one made at full width.
//!
//! # Examples
//!
//! ```
//! use rthid_dither::dither::PaletteColor;
//! use rthid_dither::mass::{MassOptions, mass_study, tones_from_palette};
//!
//! let source = [0, 0, 0, 255, 255, 255, 255, 255];
//! let palette = [PaletteColor { rgb: [0; 3] }, PaletteColor { rgb: [255; 3] }];
//! let options = MassOptions::default();
//! let study = mass_study(&source, 2, 1, &options)?;
//! let tones = tones_from_palette(&palette, &options);
//! let rgba = study.rgba(4, 2, &tones);
//! assert_eq!(rgba.len(), 4 * 2 * 4);
//! # Ok::<(), rthid_dither::MassError>(())
//! ```

use std::fmt;

use image::{ImageBuffer, Luma, imageops::FilterType};

use crate::dither::PaletteColor;

mod contour;
mod smooth;

pub use contour::{Ring, rasterize};
pub use smooth::{PathCommand, RoundedRing};

/// An accepted moment ellipse fitted to an accent component, in analysis cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ellipse {
    /// Cell-center coordinates of the ellipse center.
    pub center: [f64; 2],
    /// Major and minor semi-axis lengths.
    pub radii: [f64; 2],
    /// Major-axis angle in radians in the y-down analysis coordinate system.
    pub angle: f64,
}

/// Maximum analysis grid width, and the width every distance parameter is tuned at.
///
/// Wider sources are averaged down to this width. Blur and closing distances scale by the
/// real grid width over this value. Area limits are fractions of the grid area instead.
const ANALYSIS_WIDTH: u32 = 450;

/// Minimum intersection over union between a fitted ellipse and its accent component.
const ELLIPSE_MIN_IOU: f64 = 0.74;
/// Minimum share of ellipse cells that must lie inside the shadow tier, so a fit never
/// spills far past the tier that contains it.
const ELLIPSE_MIN_CONTAINMENT: f64 = 0.98;
/// Maximum ratio of major to minor axis variance. Nine means a 3 to 1 axis ratio, so long
/// thin strokes keep their drawn shape.
const ELLIPSE_MAX_ASPECT_SQUARED: f64 = 9.0;

/// Fixed monochrome tones in background, base, shadow, accent order, from near white
/// paper to near black.
///
/// This is also the fallback of [`tones_from_palette`] for palettes with fewer than two
/// colors. The order never changes with [`MassOptions::invert`].
pub const MONO: [PaletteColor; 4] = [
    PaletteColor {
        rgb: [245, 245, 242],
    },
    PaletteColor {
        rgb: [184, 184, 181],
    },
    PaletteColor {
        rgb: [100, 100, 98],
    },
    PaletteColor { rgb: [17, 17, 17] },
];

/// Fixed warm tones in background, base, shadow, accent order, from white paper through
/// orange to dark umber.
///
/// Like [`MONO`], the order never changes with [`MassOptions::invert`]. Geometry does not
/// depend on the tones, so both presets show the same study.
///
/// # Examples
///
/// ```
/// use rthid_dither::mass::{MassOptions, WARM, mass_study};
///
/// let source = [0, 0, 0, 255, 255, 255, 255, 255];
/// let study = mass_study(&source, 2, 1, &MassOptions::default())?;
/// let rgba = study.rgba(2, 1, &WARM);
/// assert_eq!(&rgba[..3], &WARM[3].rgb);
/// assert_eq!(&rgba[4..7], &WARM[0].rgb);
/// # Ok::<(), rthid_dither::MassError>(())
/// ```
pub const WARM: [PaletteColor; 4] = [
    PaletteColor {
        rgb: [255, 255, 255],
    },
    PaletteColor {
        rgb: [223, 121, 82],
    },
    PaletteColor { rgb: [181, 71, 39] },
    PaletteColor { rgb: [101, 48, 30] },
];

/// Samples four background first tones from a palette as a gradient between its lightest
/// and darkest colors.
///
/// The palette is sorted by luma, so input order does not matter, and the input itself is
/// never changed. The sort is stable, which keeps the result deterministic for colors of
/// equal luma. Tones sit at 0, 1/3, 2/3 and 1 of the sorted gradient, so four colors keep
/// their exact values, sixteen colors yield entries 0, 5, 10 and 15, and two colors gain
/// two blended steps. Blends are per channel on the stored sRGB bytes, rounded to the
/// nearest value.
///
/// The background tone is the lightest color, which suits dark ink on light paper. Only
/// [`MassOptions::invert`] is read. It flips the order so the background takes the darkest
/// color for light ink on a dark background. Palettes from
/// [`make_palette`](crate::dither::make_palette) are darkest first, so passing one directly
/// to [`MassStudy::rgba`] would paint the background in ink. This function gives the
/// order the study expects.
///
/// Fewer than two colors fall back to [`MONO`] regardless of invert.
///
/// # Examples
///
/// ```
/// use rthid_dither::dither::PaletteColor;
/// use rthid_dither::mass::{MassOptions, tones_from_palette};
///
/// let palette = [PaletteColor { rgb: [255; 3] }, PaletteColor { rgb: [0; 3] }];
/// let tones = tones_from_palette(&palette, &MassOptions::default());
/// assert_eq!(tones.map(|tone| tone.rgb[0]), [255, 170, 85, 0]);
///
/// let inverted = MassOptions { invert: true, ..MassOptions::default() };
/// let tones = tones_from_palette(&palette, &inverted);
/// assert_eq!(tones.map(|tone| tone.rgb[0]), [0, 85, 170, 255]);
/// ```
pub fn tones_from_palette(palette: &[PaletteColor], options: &MassOptions) -> [PaletteColor; 4] {
    if palette.len() < 2 {
        return MONO;
    }
    let mut palette = palette.to_vec();
    palette.sort_by(|a, b| luma(&a.rgb).total_cmp(&luma(&b.rgb)));
    let mut tones = std::array::from_fn(|i| {
        let position = i as f64 * (palette.len() - 1) as f64 / 3.0;
        let lower = position.floor() as usize;
        let upper = (lower + 1).min(palette.len() - 1);
        let fraction = position - lower as f64;
        PaletteColor {
            rgb: std::array::from_fn(|channel| {
                let a = f64::from(palette[lower].rgb[channel]);
                let b = f64::from(palette[upper].rgb[channel]);
                (a + (b - a) * fraction).round() as u8
            }),
        }
    });
    if !options.invert {
        tones.reverse();
    }
    tones
}

/// Rec. 601 luma of stored sRGB bytes in [0, 1], shared by ink extraction and tone
/// ordering so both agree on which color is darker.
fn luma(rgb: &[u8; 3]) -> f32 {
    (0.299 * rgb[0] as f32 + 0.587 * rgb[1] as f32 + 0.114 * rgb[2] as f32) / 255.0
}

/// Settings for smoothing, ink polarity and the three cumulative ink masks of a
/// [`mass_study`].
///
/// Any value can be built. [`mass_study`] validates the fields and returns a
/// [`MassError`] for out of range values, so options edited live can be checked by
/// running a study. The defaults give broad, poster like masses with few islands.
///
/// # Examples
///
/// ```
/// use rthid_dither::mass::{MassOptions, mass_study};
///
/// // Light ink on a dark background, with sharper edges and no ellipse fit.
/// let options = MassOptions {
///     sigma: 2.0,
///     invert: true,
///     ellipses: false,
///     ..MassOptions::default()
/// };
/// let study = mass_study(&[0, 0, 0, 255, 255, 255, 255, 255], 2, 1, &options)?;
/// assert_eq!(study.indices(), [0, 3]);
/// # Ok::<(), rthid_dither::MassError>(())
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct MassOptions {
    /// Gaussian standard deviation in cells of a 450 column analysis grid. Defaults to
    /// 4.5.
    ///
    /// Narrower grids get a proportionally smaller sigma, so smoothing covers the same
    /// share of the image width. The kernel reaches four sigma on each side. Zero disables
    /// blur. Must be finite and nonnegative.
    pub sigma: f32,
    /// Base, shadow and accent thresholds on blurred ink density. Defaults to 0.25, 0.72
    /// and 0.90.
    ///
    /// A cell joins a tier when its density is at least the threshold, so higher tiers
    /// cover less area. A density of 1 means the strongest ink in the image at full
    /// opacity, the darkest visible pixel or the lightest one with
    /// [`invert`](Self::invert). Values must be finite, strictly increasing and in (0, 1].
    pub thresholds: [f32; 3],
    /// Treats light pixels as ink, for light on dark images. Defaults to off.
    ///
    /// [`tones_from_palette`] also reads this flag and then puts the darkest color in the
    /// background. Inverting the image colors and setting this flag gives the same study,
    /// up to float rounding.
    pub invert: bool,
    /// Replaces compact accent regions with their moment ellipse when the fit is close.
    /// Defaults to on.
    ///
    /// Only the accent tier changes. Long, thin, small or border touching regions keep
    /// their drawn cells, and every ellipse is clipped to the shadow tier.
    pub ellipses: bool,
}

impl Default for MassOptions {
    fn default() -> Self {
        Self {
            sigma: 4.5,
            thresholds: [0.25, 0.72, 0.90],
            invert: false,
            ellipses: true,
        }
    }
}

/// Grid of tone indices at analysis size, returned by [`mass_study`].
///
/// Index k means the cell lies inside the first k nested masks: 0 is background, 1 base,
/// 2 shadow and 3 accent. Indices describe ink density only, so they read the same with
/// or without [`MassOptions::invert`]. Colors are chosen at paint time by
/// [`rgba`](Self::rgba), which draws index i with entry i of a background first tone
/// array.
///
/// The grid is at most 450 columns wide and keeps the source aspect ratio. Fields are
/// private so the index grid always matches its dimensions.
#[derive(Debug, Clone, PartialEq)]
pub struct MassStudy {
    /// One tone index per cell in row major order.
    indices: Vec<u8>,
    /// Number of columns.
    w: usize,
    /// Number of rows.
    h: usize,
    /// Accepted accent fits in component acceptance order.
    ellipses: Vec<Ellipse>,
}

impl MassStudy {
    /// Returns the number of analysis columns.
    ///
    /// This is the source width, capped at 450. Narrower sources are never enlarged.
    pub fn w(&self) -> usize {
        self.w
    }

    /// Returns the number of analysis rows.
    ///
    /// The source height is scaled by the same factor as the width and rounded to the
    /// nearest row, with ties going to even so the result does not depend on float noise.
    /// It is never less than one, even for very wide sources.
    pub fn h(&self) -> usize {
        self.h
    }

    /// Returns the tone indices in row major order, one per cell.
    ///
    /// The slice holds [`w`](Self::w) times [`h`](Self::h) values, each from 0
    /// (background) to 3 (accent).
    pub fn indices(&self) -> &[u8] {
        &self.indices
    }

    /// Returns accepted accent ellipses in component acceptance order.
    pub fn ellipses(&self) -> &[Ellipse] {
        &self.ellipses
    }

    /// Paints the study as opaque RGBA at any output size.
    ///
    /// Each output pixel takes the cell under its top left corner, which is exact for
    /// integer scale factors and keeps flat edges without blending. Index i is drawn with
    /// `tones[i]` and full alpha. The result holds `out_w * out_h * 4` bytes in row major
    /// order. A zero width or height returns an empty buffer.
    ///
    /// # Panics
    ///
    /// Panics if `out_w * out_h * 4` overflows `usize`.
    ///
    /// # Examples
    ///
    /// ```
    /// use rthid_dither::mass::{MONO, MassOptions, mass_study};
    ///
    /// let source = [0, 0, 0, 255, 255, 255, 255, 255];
    /// let study = mass_study(&source, 2, 1, &MassOptions::default())?;
    /// // Doubling each axis turns every cell into a 2 x 2 block.
    /// let rgba = study.rgba(4, 2, &MONO);
    /// assert_eq!(rgba.len(), 4 * 2 * 4);
    /// assert_eq!(rgba[..4], [17, 17, 17, 255]);
    /// assert_eq!(rgba[8..12], [245, 245, 242, 255]);
    /// assert!(study.rgba(0, 2, &MONO).is_empty());
    /// # Ok::<(), rthid_dither::MassError>(())
    /// ```
    #[track_caller]
    pub fn rgba(&self, out_w: usize, out_h: usize, tones: &[PaletteColor; 4]) -> Vec<u8> {
        let len = out_w
            .checked_mul(out_h)
            .and_then(|n| n.checked_mul(4))
            .unwrap_or_else(|| panic!("output {out_w}x{out_h}x4 exceeds {} bytes", usize::MAX));
        let mut rgba = Vec::with_capacity(len);
        if len == 0 {
            return rgba;
        }
        for y in 0..out_h {
            let sy = (y as u128 * self.h as u128 / out_h as u128) as usize;
            for x in 0..out_w {
                let sx = (x as u128 * self.w as u128 / out_w as u128) as usize;
                rgba.extend_from_slice(&tones[self.indices[sy * self.w + sx] as usize].rgb);
                rgba.push(255);
            }
        }
        rgba
    }
}

/// The error type for [`mass_study`], covering an empty source and invalid
/// [`MassOptions`].
///
/// Each variant carries the rejected value, so a caller can show it next to the message
/// from [`Display`](fmt::Display). A buffer whose length does not match its dimensions is a
/// caller bug and panics instead.
///
/// # Examples
///
/// ```
/// use rthid_dither::MassError;
/// use rthid_dither::mass::{MassOptions, mass_study};
///
/// let options = MassOptions { sigma: -1.0, ..MassOptions::default() };
/// let error = mass_study(&[0; 4], 1, 1, &options).unwrap_err();
/// assert_eq!(error, MassError::InvalidSigma { sigma: -1.0 });
/// assert_eq!(error.to_string(), "sigma must be finite and nonnegative, got -1");
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MassError {
    /// [`MassOptions::sigma`] is NaN, infinite or negative.
    InvalidSigma {
        /// Rejected blur sigma.
        sigma: f32,
    },
    /// [`MassOptions::thresholds`] has a value that is not finite or lies outside
    /// (0, 1], or the values are not strictly increasing.
    InvalidThresholds {
        /// Rejected cumulative mask thresholds.
        thresholds: [f32; 3],
    },
    /// The source width or height is zero.
    EmptyImage,
}

impl fmt::Display for MassError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSigma { sigma } => {
                write!(f, "sigma must be finite and nonnegative, got {sigma}")
            }
            Self::InvalidThresholds { thresholds } => {
                write!(
                    f,
                    "thresholds must be finite, strictly increasing and in (0, 1], got {thresholds:?}"
                )
            }
            Self::EmptyImage => f.write_str("source image must not be empty"),
        }
    }
}

impl std::error::Error for MassError {}

/// Builds a mass study from a `w` by `h` image of row major RGBA bytes with straight
/// alpha.
///
/// Ink is relative to the image's own darkest and lightest pixels with nonzero alpha, so
/// two color images map to exactly 0 and 1 and continuous tone images use their full
/// range. Alpha then scales each pixel's ink, and fully transparent pixels add none.
/// Uniform or fully transparent images have no visible luma contrast and give an all
/// background study.
///
/// The ink field is averaged down to at most 450 columns, blurred and cut at the three
/// [`MassOptions::thresholds`]. Each tier is then closed to bridge small gaps, trimmed to
/// its few largest regions, cleared of small enclosed holes and clipped to the tier below.
/// With [`MassOptions::ellipses`], compact accent regions are finally replaced by fitted
/// ellipses. The returned grid keeps the source aspect ratio, and
/// [`MassStudy::rgba`] paints it at any output size.
///
/// # Errors
///
/// Checks run in this order and the first failure is returned.
///
/// - [`MassError::EmptyImage`] if `w` or `h` is zero, whatever the buffer holds.
/// - [`MassError::InvalidSigma`] if the sigma is NaN, infinite or negative.
/// - [`MassError::InvalidThresholds`] if a threshold is not finite, is outside (0, 1],
///   or is not greater than the one before it.
///
/// # Panics
///
/// For a nonzero size, panics if
///
/// - `w * h * 4` overflows `usize`,
/// - `rgba.len()` is not `w * h * 4`, checked before the options, or
/// - `w` or `h` exceeds `u32::MAX`.
///
/// # Examples
///
/// ```
/// use rthid_dither::MassError;
/// use rthid_dither::mass::{MassOptions, mass_study};
///
/// // Black, white and a fully transparent pixel, without blur.
/// let rgba = [0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 0];
/// let options = MassOptions { sigma: 0.0, ..MassOptions::default() };
/// let study = mass_study(&rgba, 3, 1, &options)?;
/// assert_eq!((study.w(), study.h()), (3, 1));
/// assert_eq!(study.indices(), [3, 0, 0]);
///
/// assert_eq!(mass_study(&[], 0, 1, &options), Err(MassError::EmptyImage));
/// # Ok::<(), MassError>(())
/// ```
#[track_caller]
pub fn mass_study(
    rgba: &[u8],
    w: usize,
    h: usize,
    options: &MassOptions,
) -> Result<MassStudy, MassError> {
    if w == 0 || h == 0 {
        return Err(MassError::EmptyImage);
    }
    let n = w
        .checked_mul(h)
        .unwrap_or_else(|| panic!("image {w}x{h} exceeds {} pixels", usize::MAX));
    let bytes = n
        .checked_mul(4)
        .unwrap_or_else(|| panic!("image {w}x{h}x4 exceeds {} bytes", usize::MAX));
    assert!(
        rgba.len() == bytes,
        "rgba has {} bytes, expected {w}x{h}x4 = {bytes}",
        rgba.len()
    );
    if !options.sigma.is_finite() || options.sigma < 0.0 {
        return Err(MassError::InvalidSigma {
            sigma: options.sigma,
        });
    }
    let mut previous = 0.0;
    for &threshold in &options.thresholds {
        if !threshold.is_finite() || threshold <= previous || threshold > 1.0 {
            return Err(MassError::InvalidThresholds {
                thresholds: options.thresholds,
            });
        }
        previous = threshold;
    }
    // Images are only shrunk, never enlarged. Ties go to even so results do not depend on
    // float noise, and very wide images still keep one row.
    let aw = w.min(ANALYSIS_WIDTH as usize);
    let ah = ((h as f64 * aw as f64 / w as f64).round_ties_even() as usize).max(1);
    // Ink is measured against the darkest and lightest visible pixels so every image uses
    // the full density range. Transparent pixels add no ink and do not affect the range.
    // Alpha then thins partly transparent ink.
    let mut ink = Vec::with_capacity(n);
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for pixel in rgba.as_chunks::<4>().0 {
        let luma = luma(&[pixel[0], pixel[1], pixel[2]]);
        if pixel[3] > 0 {
            lo = lo.min(luma);
            hi = hi.max(luma);
        }
        ink.push(luma);
    }
    for (value, pixel) in ink.iter_mut().zip(rgba.as_chunks::<4>().0) {
        *value = if hi > lo && pixel[3] > 0 {
            let contrast = if options.invert {
                *value - lo
            } else {
                hi - *value
            };
            contrast / (hi - lo) * (pixel[3] as f32 / 255.0)
        } else {
            0.0
        };
    }
    let source = ImageBuffer::<Luma<f32>, _>::from_raw(
        u32::try_from(w).unwrap_or_else(|_| panic!("image width {w} exceeds {} pixels", u32::MAX)),
        u32::try_from(h).unwrap_or_else(|_| panic!("image height {h} exceeds {} pixels", u32::MAX)),
        ink,
    )
    .expect("validated image dimensions");
    // Averaging on the way down turns thin dark lines into lower density instead of
    // dropping them. The triangle filter widens with the scale factor, so every source
    // pixel contributes.
    let mut density = image::imageops::resize(
        &source,
        aw as u32,
        u32::try_from(ah)
            .unwrap_or_else(|_| panic!("analysis height {ah} exceeds {} pixels", u32::MAX)),
        FilterType::Triangle,
    )
    .into_raw();
    // Scale the blur to the grid width so it covers the same share of the image at any size.
    blur(
        &mut density,
        aw,
        ah,
        options.sigma as f64 * aw as f64 / ANALYSIS_WIDTH as f64,
    );
    let (masks, ellipses) = masks(&density, aw, ah, &options.thresholds, options.ellipses);
    // The masks are nested, so counting the ones that contain a cell gives its tone index.
    let mut indices = vec![0; aw * ah];
    for mask in masks {
        for (index, foreground) in indices.iter_mut().zip(mask) {
            *index += u8::from(foreground);
        }
    }
    Ok(MassStudy {
        indices,
        w: aw,
        h: ah,
        ellipses,
    })
}

/// Mirrors an out of range position back inside `0..len` without repeating the edge cell.
///
/// Positions fold with a period of `2 * (len - 1)`, so any distance works, including
/// kernels wider than the image. A single cell axis always maps to that cell.
///
/// ```text
/// len 4   position  -3 -2 -1 | 0 1 2 3 | 4 5 6
///         reads      3  2  1 | 0 1 2 3 | 2 1 0
/// ```
fn reflect(position: i64, len: usize) -> usize {
    if len == 1 {
        return 0;
    }
    let period = 2 * (len as i64 - 1);
    let p = position.rem_euclid(period);
    p.min(period - p) as usize
}

/// Blurs a row major density grid in place with a separable Gaussian of the given sigma
/// in cells.
///
/// The kernel is cut off at four sigma and normalized, so a uniform field keeps its value.
/// Each pass reads past the edges through [`reflect`]. Results are clamped because
/// rounding can push density slightly past one. A zero sigma leaves the grid unchanged.
///
/// ```text
/// weight[k] = exp(-k^2 / (2 sigma^2)) / sum,  k in -r..=r,  r = ceil(4 sigma)
///
/// rows first        then columns over the row results
/// . . . . .         . . w . .
/// w w x w w   ->    . . x . .
/// . . . . .         . . w . .
/// x is the cell being computed and w its weighted neighbors
/// ```
fn blur(density: &mut [f32], w: usize, h: usize, sigma: f64) {
    if sigma == 0.0 {
        return;
    }
    let radius = (4.0 * sigma).ceil() as i64;
    let mut kernel = Vec::new();
    for offset in -radius..=radius {
        kernel.push((-0.5 * (offset as f64 / sigma).powi(2)).exp());
    }
    let sum: f64 = kernel.iter().sum();
    for weight in &mut kernel {
        *weight /= sum;
    }
    let mut horizontal = vec![0.0; density.len()];
    for y in 0..h {
        for x in 0..w {
            let mut value = 0.0;
            for (k, weight) in kernel.iter().enumerate() {
                value += *weight * density[y * w + reflect(x as i64 + k as i64 - radius, w)] as f64;
            }
            horizontal[y * w + x] = value;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let mut value = 0.0;
            for (k, weight) in kernel.iter().enumerate() {
                value += *weight * horizontal[reflect(y as i64 + k as i64 - radius, h) * w + x];
            }
            density[y * w + x] = value.clamp(0.0, 1.0) as f32;
        }
    }
}

/// Cuts the blurred density at each threshold and returns the cleaned base, shadow and
/// accent masks, each nested inside the one before it.
///
/// Every tier goes through [`clean_mask`] on its own raw threshold mask and is then
/// clipped to the tier below. Closing radius is three cells at 450 columns, so grids of
/// 75 columns or fewer skip closing. Regions smaller than 0.3 percent of the grid are
/// dropped, and base, shadow and accent keep at most 3, 6 and 5 regions. Accent allows
/// regions half as small so small dark accents survive. Holes up to 0.3 percent of the
/// grid are filled in every tier.
///
/// The accent cap applies before [`fit_ellipses`]. An ellipse can merge nearby blobs, so
/// the count usually stays within the cap. It is not a hard bound, since clipping an
/// ellipse to the shadow tier could split it.
///
/// ```text
/// density >= base     . . # # # # # . .
/// density >= shadow   . . . # # # . . .
/// density >= accent   . . . . # . . . .
///                     -----------------
/// tone index          0 0 1 2 3 2 1 0 0   (sum of masks per cell)
/// ```
fn masks(
    density: &[f32],
    w: usize,
    h: usize,
    thresholds: &[f32; 3],
    ellipses: bool,
) -> (Vec<Vec<bool>>, Vec<Ellipse>) {
    let radius = (3.0 * w as f64 / ANALYSIS_WIDTH as f64).round_ties_even() as isize;
    let mut result: Vec<Vec<bool>> = Vec::new();
    let mut accepted = Vec::new();
    for i in 0..3 {
        let mut raw = Vec::with_capacity(density.len());
        for &value in density {
            raw.push(value >= thresholds[i]);
        }
        // The minimum area rounds up and the hole limit rounds down, so both limits err
        // toward keeping the image as drawn.
        let mut mask = clean_mask(
            &raw,
            w,
            h,
            radius,
            ([0.003, 0.003, 0.0015][i] * (w * h) as f64).ceil() as usize,
            [3, 6, 5][i],
            (0.003 * (w * h) as f64).floor() as usize,
        );
        // Each tier is cleaned on its own threshold before it is clipped to the tier below.
        // The region cap counts regions of the tier itself. Clipping only removes cells,
        // which keeps the tiers nested.
        if let Some(parent) = result.last() {
            for (cell, &p) in mask.iter_mut().zip(parent) {
                *cell &= p;
            }
            if i == 2 && ellipses {
                (mask, accepted) = fit_ellipses(&mask, parent, w, h);
            }
        }
        result.push(mask);
    }
    (result, accepted)
}

/// Replaces each compact accent component with its moment ellipse when the ellipse is a
/// close fit, and returns the new accent mask.
///
/// The ellipse has the same centroid and covariance as the component's cell centers.
/// Components with fewer than five cells, touching the border, or with zero minor axis
/// variance are kept as drawn. Accepted ellipses are painted clipped to `shadow`, so the
/// masks stay nested. Rejected components keep their cells.
///
/// ```text
///   . # # # .       center = mean of cell centers
///   # # # # #       a = 2 sqrt(major variance), b = 2 sqrt(minor variance)
///   # # . # #  ->   major axis at theta = atan2(2 sxy, sxx - syy) / 2
///   . # # # .
///
/// accept if  a / b <= 3
///       and  |E and C| / |E or C|  >= 0.74   (E ellipse cells, C component)
///       and  |E and shadow| / |E|  >= 0.98
/// ```
fn fit_ellipses(accent: &[bool], shadow: &[bool], w: usize, h: usize) -> (Vec<bool>, Vec<Ellipse>) {
    let mut out = accent.to_vec();
    let mut accepted = Vec::new();
    for mut cells in components(accent, w, h, true, true) {
        if cells.len() < 5 {
            continue;
        }
        // Sorted cells allow a binary search for the intersection count below.
        cells.sort_unstable();
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (w, h, 0, 0);
        let (mut cx, mut cy) = (0.0, 0.0);
        for &p in &cells {
            let (x, y) = (p % w, p / w);
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            cx += x as f64 + 0.5;
            cy += y as f64 + 0.5;
        }
        // A component cut by the border is a crop, so its moments do not describe its shape.
        if min_x == 0 || min_y == 0 || max_x == w - 1 || max_y == h - 1 {
            continue;
        }
        let n = cells.len() as f64;
        cx /= n;
        cy /= n;
        let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
        for &p in &cells {
            let dx = (p % w) as f64 + 0.5 - cx;
            let dy = (p / w) as f64 + 0.5 - cy;
            sxx += dx * dx;
            sxy += dx * dy;
            syy += dy * dy;
        }
        sxx /= n;
        sxy /= n;
        syy /= n;
        // Eigenvalues of the covariance matrix are the variances along the major and minor
        // axes.
        let m = (sxx + syy) / 2.0;
        let d = (((sxx - syy) / 2.0).powi(2) + sxy * sxy).sqrt();
        let (major, minor) = (m + d, m - d);
        if minor <= 0.0 || major > ELLIPSE_MAX_ASPECT_SQUARED * minor {
            continue;
        }
        // A filled ellipse with semi axis s has variance s^2 / 4 along it, so twice the
        // standard deviation gives the semi axes of the uniform ellipse with these moments.
        let (a, b) = (2.0 * major.sqrt(), 2.0 * minor.sqrt());
        let (mut vx, mut vy) = if d < 1e-9 {
            (1.0, 0.0)
        } else {
            // atan2 avoids eigenvector cancellation to noise when the major axis is vertical.
            let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
            (theta.cos(), theta.sin())
        };
        // Pick one of the two opposite axis directions so the fit is deterministic.
        if vx < 0.0 || (vx == 0.0 && vy < 0.0) {
            vx = -vx;
            vy = -vy;
        }
        // The center lies inside the bounding box, so this window holds every cell within
        // the semi major axis of it. Cells past the grid edge are not counted.
        let padding = a.ceil() as usize + 1;
        let mut ellipse = Vec::new();
        let (mut intersection, mut contained) = (0, 0);
        for y in min_y.saturating_sub(padding)..=max_y.saturating_add(padding).min(h - 1) {
            for x in min_x.saturating_sub(padding)..=max_x.saturating_add(padding).min(w - 1) {
                let dx = x as f64 + 0.5 - cx;
                let dy = y as f64 + 0.5 - cy;
                let u = dx * vx + dy * vy;
                let t = -dx * vy + dy * vx;
                if (u / a).powi(2) + (t / b).powi(2) <= 1.0 {
                    let p = y * w + x;
                    ellipse.push(p);
                    intersection += usize::from(cells.binary_search(&p).is_ok());
                    contained += usize::from(shadow[p]);
                }
            }
        }
        if ellipse.is_empty() {
            continue;
        }
        let iou = intersection as f64 / (ellipse.len() + cells.len() - intersection) as f64;
        let containment = contained as f64 / ellipse.len() as f64;
        if iou >= ELLIPSE_MIN_IOU && containment >= ELLIPSE_MIN_CONTAINMENT {
            accepted.push((
                cells,
                ellipse,
                Ellipse {
                    center: [cx, cy],
                    radii: [a, b],
                    angle: vy.atan2(vx),
                },
            ));
        }
    }
    // Clear every accepted component before painting so later fits cannot erase earlier ellipses.
    for (cells, _, _) in &accepted {
        for &p in cells {
            out[p] = false;
        }
    }
    let mut fits = Vec::with_capacity(accepted.len());
    for (_, ellipse, fit) in accepted {
        for p in ellipse {
            out[p] = shadow[p];
        }
        fits.push(fit);
    }
    (out, fits)
}

/// Grows (`dilate`) or shrinks the mask by a disk of the given radius.
///
/// A cell turns on under dilation when any cell of the disk around it is on, and turns
/// off under erosion when any is off. Positions past the edge read the nearest edge cell,
/// so masses touching the border are not eaten away by shrinking. A zero radius returns
/// the mask unchanged.
///
/// ```text
/// radius 2 disk     edge clamp on a row of width 4
/// . . # . .         offset  -2 -1 | 0 1 2 3 | 4 5
/// # # # # #         reads    0  0 | 0 1 2 3 | 3 3
/// # # # # #
/// # # # # #         row half width = round(sqrt(r^2 - dy^2)), ties to even
/// . . # . .
/// ```
fn morphology(mask: &[bool], w: usize, h: usize, radius: isize, dilate: bool) -> Vec<bool> {
    let mut out = vec![!dilate; mask.len()];
    for y in 0..h {
        for x in 0..w {
            'disk: for dy in -radius..=radius {
                // Half width of the disk on this row, so the shape is round and not square.
                let extent = ((radius * radius - dy * dy) as f64)
                    .sqrt()
                    .round_ties_even() as isize;
                for dx in -extent..=extent {
                    let nx = (x as isize + dx).clamp(0, w as isize - 1) as usize;
                    let ny = (y as isize + dy).clamp(0, h as isize - 1) as usize;
                    if mask[ny * w + nx] == dilate {
                        out[y * w + x] = dilate;
                        break 'disk;
                    }
                }
            }
        }
    }
    out
}

/// Returns the connected regions of cells equal to `foreground`, as cell index lists in
/// scan order.
///
/// `diagonal` picks eight neighbor links instead of four. Each region's first cell is its
/// topmost, then leftmost cell, which gives a stable key for ordering. The other cells
/// follow in no particular order.
///
/// ```text
/// eight neighbor   four neighbor   # .   one region with eight links,
///   n n n            . n .         . #   two with four
///   n p n            n p n
///   n n n            . n .
/// ```
fn components(
    mask: &[bool],
    w: usize,
    h: usize,
    foreground: bool,
    diagonal: bool,
) -> Vec<Vec<usize>> {
    let mut seen = vec![false; mask.len()];
    let mut result = Vec::new();
    let mut stack = Vec::new();
    for first in 0..mask.len() {
        if seen[first] || mask[first] != foreground {
            continue;
        }
        seen[first] = true;
        stack.push(first);
        let mut cells = Vec::new();
        while let Some(p) = stack.pop() {
            cells.push(p);
            let (x, y) = (p % w, p / w);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    if !diagonal && nx != x && ny != y {
                        continue;
                    }
                    let q = ny * w + nx;
                    if !seen[q] && mask[q] == foreground {
                        seen[q] = true;
                        stack.push(q);
                    }
                }
            }
        }
        result.push(cells);
    }
    result
}

/// Whether any of `cells` lies on the grid border.
fn touches_border(cells: &[usize], w: usize, h: usize) -> bool {
    cells
        .iter()
        .any(|&p| p % w == 0 || p % w == w - 1 || p / w == 0 || p / w == h - 1)
}

/// Simplifies one threshold mask into a few solid regions.
///
/// Closing (dilate, then erode by the same disk) bridges gaps narrower than the disk.
/// Then the largest eight connected regions of at least `min_area` cells are kept, up to
/// `cap` of them. Last, four connected background holes of at most `max_hole` cells that
/// do not touch the border are filled. Dropped islands inside a kept region become part
/// of its hole and can be filled.
fn clean_mask(
    mask: &[bool],
    w: usize,
    h: usize,
    radius: isize,
    min_area: usize,
    cap: usize,
    max_hole: usize,
) -> Vec<bool> {
    let closed = morphology(&morphology(mask, w, h, radius, true), w, h, radius, false);
    let mut regions = components(&closed, w, h, true, true);
    // Larger regions win. Equal areas go to the region that starts first in row major
    // order, so the result is the same on every run.
    regions.sort_by_key(|cells| (std::cmp::Reverse(cells.len()), cells[0]));
    let mut out = vec![false; mask.len()];
    for cells in regions
        .iter()
        .filter(|cells| cells.len() >= min_area)
        .take(cap)
    {
        for &p in cells {
            out[p] = true;
        }
    }
    // Holes use four neighbor links while regions use eight. A diagonal gap then never counts
    // as both a region link and a way out of a hole.
    for hole in components(&out, w, h, false, false) {
        // Background that reaches the border is outside space, not a hole.
        if hole.len() <= max_hole && !touches_border(&hole, w, h) {
            for p in hole {
                out[p] = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fitted(accent: &[bool], shadow: &[bool], w: usize, h: usize) -> Vec<bool> {
        fit_ellipses(accent, shadow, w, h).0
    }

    fn tone_counts(study: &MassStudy) -> [usize; 4] {
        let mut counts = [0; 4];
        for &index in study.indices() {
            counts[index as usize] += 1;
        }
        counts
    }

    fn study(rgba: &[u8], w: usize, h: usize, options: &MassOptions) -> MassStudy {
        let result = mass_study(rgba, w, h, options).unwrap();
        assert_eq!(result.indices().len(), result.w() * result.h());
        result
    }

    fn grid(rows: &[&str]) -> Vec<bool> {
        rows.iter()
            .flat_map(|row| row.bytes().map(|c| c == b'#'))
            .collect()
    }

    fn drawn_ellipse(
        w: usize,
        h: usize,
        cx: f64,
        cy: f64,
        a: f64,
        b: f64,
        angle: f64,
    ) -> Vec<bool> {
        let mut mask = vec![false; w * h];
        let (vy, vx) = angle.sin_cos();
        for y in 0..h {
            for x in 0..w {
                let dx = x as f64 + 0.5 - cx;
                let dy = y as f64 + 0.5 - cy;
                let u = dx * vx + dy * vy;
                let t = -dx * vy + dy * vx;
                mask[y * w + x] = (u / a).powi(2) + (t / b).powi(2) <= 1.0;
            }
        }
        mask
    }

    #[test]
    fn fits_drawn_ellipses() {
        for (name, w, h, a, b, angle) in [
            ("rotated ellipse", 60, 40, 20.0, 12.5, 30.0_f64.to_radians()),
            ("circle", 40, 40, 12.0, 12.0, 0.0),
            ("upright ellipse", 40, 40, 8.0, 16.0, 0.0),
        ] {
            let accent = drawn_ellipse(w, h, w as f64 / 2.0, h as f64 / 2.0, a, b, angle);
            let shadow = morphology(&accent, w, h, 2, true);
            assert_eq!(fitted(&accent, &shadow, w, h), accent, "{name}");
            // A missing interior cell makes acceptance observable without changing the outline.
            let mut chipped = accent.clone();
            let chip = (h / 2) * w + w / 2;
            assert!(accent[chip], "{name}: chip must remove a drawn cell");
            chipped[chip] = false;
            let fitted = fitted(&chipped, &shadow, w, h);
            assert!(fitted[chip], "{name}: fit must restore the chipped cell");
            assert_eq!(fitted, accent, "{name}");
        }
    }

    #[test]
    fn fits_an_upright_ellipse_off_the_pixel_grid() {
        let accent = drawn_ellipse(40, 40, 20.0, 20.2, 8.0, 16.0, 0.0);
        let shadow = morphology(&accent, 40, 40, 2, true);
        let mut chipped = accent.clone();
        chipped[12 * 40 + 19] = false;
        chipped[12 * 40 + 20] = false;
        let fitted = fitted(&chipped, &shadow, 40, 40);
        assert!(fitted[12 * 40 + 19] && fitted[12 * 40 + 20]);
        let changed = fitted.iter().zip(&accent).filter(|(a, b)| a != b).count();
        assert!(
            changed <= 8,
            "{changed} cells differ from the drawn ellipse"
        );
    }

    #[test]
    fn keeps_an_earlier_ellipse_when_a_later_fit_clears_cells() {
        let (w, h) = (80, 60);
        let mut first = drawn_ellipse(w, h, 40.0, 25.0, 25.0, 16.0, 0.0);
        let mut second = drawn_ellipse(w, h, 38.0, 33.0, 3.0, 3.0, 0.0);
        second[28 * w + 38] = true;
        second[29 * w + 38] = true;
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                if second[y * w + x] {
                    for ny in y - 1..=y + 1 {
                        for nx in x - 1..=x + 1 {
                            first[ny * w + nx] = false;
                        }
                    }
                }
            }
        }
        let shadow = vec![true; w * h];
        let fitted_first = fitted(&first, &shadow, w, h);
        let fitted_second = fitted(&second, &shadow, w, h);
        assert_ne!(fitted_first, first);
        assert_ne!(fitted_second, second);
        let mut accent = vec![false; w * h];
        let mut expected = vec![false; w * h];
        let mut overlap = 0;
        for p in 0..accent.len() {
            accent[p] = first[p] || second[p];
            expected[p] = fitted_first[p] || fitted_second[p];
            overlap += usize::from(fitted_first[p] && second[p] && !fitted_second[p]);
        }
        assert!(overlap > 0);
        assert_eq!(components(&accent, w, h, true, true).len(), 2);
        assert_eq!(fitted(&accent, &shadow, w, h), expected);
    }

    #[test]
    fn leaves_thin_border_and_small_components_unchanged() {
        let mut bar = vec![false; 60 * 40];
        for y in 18..22 {
            for x in 10..50 {
                bar[y * 60 + x] = true;
            }
        }
        let mut edge = drawn_ellipse(60, 40, 30.0, 20.0, 20.0, 12.0, 30.0_f64.to_radians());
        let mut left = 60;
        for (p, &cell) in edge.iter().enumerate() {
            if cell {
                left = left.min(p % 60);
            }
        }
        let mut translated = vec![false; edge.len()];
        for p in 0..edge.len() {
            if edge[p] {
                translated[p - left] = true;
            }
        }
        edge = translated;
        assert!(edge.iter().step_by(60).any(|&cell| cell));
        for accent in [bar, edge] {
            assert_eq!(fitted(&accent, &[true; 60 * 40], 60, 40), accent);
        }
        let small = grid(&["....", ".##.", ".##.", "...."]);
        assert_eq!(fitted(&small, &[true; 16], 4, 4), small);
    }

    #[test]
    fn rejects_an_ellipse_outside_shadow() {
        let accent = drawn_ellipse(40, 40, 20.0, 20.0, 12.0, 12.0, 0.0);
        let mut shadow = accent.clone();
        let mut area = 0;
        for &cell in &accent {
            area += usize::from(cell);
        }
        let mut removed = 0;
        for cell in &mut shadow {
            if *cell && removed < area / 10 {
                *cell = false;
                removed += 1;
            }
        }
        let mut chipped = accent.clone();
        chipped[20 * 40 + 20] = false;
        assert_ne!(fitted(&chipped, &accent, 40, 40), chipped);
        assert_eq!(fitted(&chipped, &shadow, 40, 40), chipped);
    }

    #[test]
    fn ellipse_option_preserves_old_indices_and_ancestor_masks() {
        let (w, h) = (60, 40);
        let mut accent = drawn_ellipse(w, h, 30.0, 20.0, 20.0, 12.0, 30.0_f64.to_radians());
        for y in 18..22 {
            for x in 28..32 {
                accent[y * w + x] = false;
            }
        }
        let shadow = morphology(&accent, w, h, 2, true);
        let mut rgba = Vec::new();
        let mut expected = Vec::new();
        for p in 0..accent.len() {
            let (v, index) = if accent[p] {
                (0, 3)
            } else if shadow[p] {
                (50, 2)
            } else {
                (255, 0)
            };
            rgba.extend_from_slice(&[v, v, v, 255]);
            expected.push(index);
        }
        let options = MassOptions {
            sigma: 0.0,
            ellipses: false,
            ..MassOptions::default()
        };
        let old = study(&rgba, w, h, &options);
        assert_eq!(old.indices(), expected);
        let fitted = study(
            &rgba,
            w,
            h,
            &MassOptions {
                ellipses: true,
                ..options
            },
        );
        assert_ne!(old.indices(), fitted.indices());
        for (&a, &b) in old.indices().iter().zip(fitted.indices()) {
            assert!(a == b || ((a == 2 || a == 3) && (b == 2 || b == 3)));
        }
    }

    #[test]
    fn palette_tones_match_expected_gradients() {
        let mut reversed = WARM;
        reversed.reverse();
        let shuffled = [WARM[2], WARM[0], WARM[3], WARM[1]];
        let two = [
            PaletteColor { rgb: [0, 10, 20] },
            PaletteColor {
                rgb: [100, 110, 120],
            },
        ];
        let blended = [[100, 110, 120], [67, 77, 87], [33, 43, 53], [0, 10, 20]]
            .map(|rgb| PaletteColor { rgb });
        let mut inverted = blended;
        inverted.reverse();
        let sixteen: [_; 16] = std::array::from_fn(|i| PaletteColor {
            rgb: [i as u8 * 16; 3],
        });
        for (name, palette, invert, expected) in [
            ("reversed four colors", reversed.as_slice(), false, WARM),
            ("shuffled", shuffled.as_slice(), false, WARM),
            ("two colors", two.as_slice(), false, blended),
            (
                "sixteen colors",
                sixteen.as_slice(),
                false,
                [sixteen[15], sixteen[10], sixteen[5], sixteen[0]],
            ),
            ("inverted", two.as_slice(), true, inverted),
            ("empty", &[], false, MONO),
            ("empty inverted", &[], true, MONO),
            ("single color", &WARM[..1], false, MONO),
            ("single color inverted", &WARM[..1], true, MONO),
        ] {
            assert_eq!(
                tones_from_palette(
                    palette,
                    &MassOptions {
                        invert,
                        ..MassOptions::default()
                    }
                ),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn validates_input() {
        let options = MassOptions::default();
        assert_eq!(mass_study(&[], 0, 2, &options), Err(MassError::EmptyImage));
        assert_eq!(mass_study(&[], 2, 0, &options), Err(MassError::EmptyImage));
        assert_eq!(
            mass_study(&[0; 4], 0, 1, &options),
            Err(MassError::EmptyImage)
        );
        assert_eq!(
            mass_study(&[0; 4], 1, 0, &options),
            Err(MassError::EmptyImage)
        );
        for sigma in [-1.0, f32::NAN, f32::INFINITY] {
            let error = mass_study(
                &[0; 4],
                1,
                1,
                &MassOptions {
                    sigma,
                    ..options.clone()
                },
            )
            .unwrap_err();
            let MassError::InvalidSigma { sigma: actual } = error else {
                panic!("{error}")
            };
            assert_eq!(actual.to_bits(), sigma.to_bits());
            assert_eq!(
                error.to_string(),
                format!("sigma must be finite and nonnegative, got {sigma}")
            );
        }
        for thresholds in [
            [0.0, 0.5, 1.0],
            [0.5, 0.5, 1.0],
            [0.6, 0.5, 1.0],
            [0.2, 0.5, 1.1],
            [0.2, f32::NAN, 1.0],
            [0.2, 0.5, f32::INFINITY],
        ] {
            let error = mass_study(
                &[0; 4],
                1,
                1,
                &MassOptions {
                    thresholds,
                    ..options.clone()
                },
            )
            .unwrap_err();
            let MassError::InvalidThresholds { thresholds: actual } = error else {
                panic!("{error}")
            };
            assert_eq!(actual.map(f32::to_bits), thresholds.map(f32::to_bits));
            assert_eq!(
                error.to_string(),
                format!(
                    "thresholds must be finite, strictly increasing and in (0, 1], got {thresholds:?}"
                )
            );
        }
    }

    #[test]
    #[should_panic(expected = "rgba has 3 bytes, expected 1x1x4 = 4")]
    fn rejects_length_mismatch() {
        let _ = mass_study(&[0; 3], 1, 1, &MassOptions::default());
    }

    #[test]
    fn uniform_images_and_threshold_equality() {
        let options = MassOptions {
            sigma: 0.0,
            thresholds: [0.25, 0.5, 1.0],
            ..MassOptions::default()
        };
        assert_eq!(
            study(&[0, 0, 0, 255].repeat(12), 4, 3, &options).indices(),
            vec![0; 12]
        );
        assert_eq!(study(&[255; 48], 4, 3, &options).indices(), vec![0; 12]);
        assert_eq!(study(&[0; 48], 4, 3, &options).indices(), vec![0; 12]);
        let mut rgba = [17, 17, 17, 255].repeat(6);
        rgba.extend_from_slice(&[245, 245, 245, 255].repeat(6));
        assert_eq!(
            study(&rgba, 4, 3, &options).indices(),
            [vec![3; 6], vec![0; 6]].concat()
        );
        assert_eq!(
            study(
                &[0, 0, 0, 255, 64, 64, 64, 255, 128, 128, 128, 255],
                3,
                1,
                &options
            )
            .indices(),
            [3, 2, 0]
        );
    }

    #[test]
    fn stretches_visible_luma_before_scaling_alpha() {
        let rgba = [
            0, 0, 0, 128, 64, 64, 64, 255, 128, 128, 128, 255, 255, 255, 255, 0,
        ];
        for invert in [false, true] {
            let options = MassOptions {
                sigma: 0.0,
                thresholds: [0.25, 0.5, 1.0],
                invert,
                ..MassOptions::default()
            };
            assert_eq!(
                study(&rgba, 4, 1, &options).indices(),
                if invert { [0, 2, 3, 0] } else { [2, 2, 0, 0] }
            );
            assert_eq!(
                study(&[17, 17, 17, 255].repeat(4), 4, 1, &options).indices(),
                [0; 4]
            );
            assert_eq!(
                study(&[0, 0, 0, 0, 255, 255, 255, 0], 2, 1, &options).indices(),
                [0; 2]
            );
        }
    }

    #[test]
    fn cleans_masks() {
        for (name, rows, w, h, (radius, min_area, cap, max_hole), expected) in [
            (
                "removes small components and breaks ties by first cell",
                &["##...##", ".......", "...#..."][..],
                7,
                3,
                (0, 2, 1, 0),
                &["##.....", ".......", "......."][..],
            ),
            (
                "removes all components below minimum area",
                &["##...##", ".......", "...#..."],
                7,
                3,
                (0, 3, 3, 0),
                &[".......", ".......", "......."],
            ),
            (
                "fills only small enclosed holes",
                &["#.######", "###.####", "########", "##..####", "########"],
                8,
                5,
                (0, 1, 1, 1),
                &["#.######", "########", "########", "##..####", "########"],
            ),
            (
                "keeps diagonally connected foreground",
                &["#..", ".#.", "..#"],
                3,
                3,
                (0, 3, 1, 0),
                &["#..", ".#.", "..#"],
            ),
            (
                "fills diagonally separated background holes",
                &[".####", "#.###", "##.##", "#####", "#####"],
                5,
                5,
                (0, 1, 1, 1),
                &[".####", "#####", "#####", "#####", "#####"],
            ),
            (
                "closing replicates edges and joins a gap",
                &["##.##", "##.##", "##.##"],
                5,
                3,
                (1, 1, 1, 0),
                &["#####", "#####", "#####"],
            ),
        ] {
            assert_eq!(
                clean_mask(&grid(rows), w, h, radius, min_area, cap, max_hole),
                grid(expected),
                "{name}"
            );
        }
    }

    #[test]
    fn intersects_masks_after_component_caps() {
        let mut density = vec![0.0; 100 * 100];
        for region in 0..4 {
            for y in 10..16 {
                for x in (10 + region * 20)..(16 + region * 20) {
                    density[y * 100 + x] = if region == 3 { 1.0 } else { 0.8 };
                }
            }
        }
        let (masks, _) = masks(&density, 100, 100, &[0.25, 0.72, 0.90], false);
        assert_eq!(components(&masks[0], 100, 100, true, true).len(), 3);
        assert_eq!(masks[0], masks[1]);
        assert!(masks[2].iter().all(|&cell| !cell));
    }

    #[test]
    fn blur_reflects_without_repeating_edges() {
        assert_eq!(
            (0..7).map(|i| reflect(i - 2, 3)).collect::<Vec<_>>(),
            [2, 1, 0, 1, 2, 1, 0]
        );
        let mut field = vec![0.0, 1.0, 0.0];
        blur(&mut field, 3, 1, 0.5);
        assert!((field[0] - field[2]).abs() < 1e-6);
        assert!(field[0] > 0.2 && field[0] < 0.22);
        let mut singleton = vec![0.75];
        blur(&mut singleton, 1, 1, 4.5);
        assert_eq!(singleton, [0.75]);
    }

    #[test]
    fn invert_preserves_geometry() {
        let mut rgba = Vec::new();
        for y in 0..20 {
            for x in 0..30 {
                let v = ((x * 19 + y * 31) % 256) as u8;
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let options = MassOptions {
            sigma: 0.0,
            ..MassOptions::default()
        };
        let first = study(&rgba, 30, 20, &options);
        for pixel in rgba.as_chunks_mut::<4>().0 {
            for channel in &mut pixel[..3] {
                *channel = 255 - *channel;
            }
        }
        assert_eq!(
            first,
            study(
                &rgba,
                30,
                20,
                &MassOptions {
                    invert: true,
                    ..options
                }
            )
        );
    }

    #[test]
    fn rendering_and_counts_accept_any_consistent_grid() {
        let result = study(
            &[
                255, 255, 255, 255, 170, 170, 170, 255, 85, 85, 85, 255, 0, 0, 0, 255,
            ],
            2,
            2,
            &MassOptions {
                sigma: 0.0,
                thresholds: [0.25, 0.5, 0.9],
                invert: false,
                ..MassOptions::default()
            },
        );
        assert_eq!(tone_counts(&result), [1; 4]);
        let rgba = result.rgba(4, 4, &WARM);
        for y in 0..4 {
            for x in 0..4 {
                let offset = (y * 4 + x) * 4;
                assert_eq!(&rgba[offset..offset + 3], &WARM[(y / 2) * 2 + x / 2].rgb);
                assert_eq!(rgba[offset + 3], 255);
            }
        }
        assert!(result.rgba(0, 2, &MONO).is_empty());
    }

    #[test]
    fn analysis_height_rounds_ties_to_even() {
        let result = study(&[255; 4].repeat(900 * 5), 900, 5, &MassOptions::default());
        assert_eq!((result.w(), result.h()), (450, 2));
    }
}
