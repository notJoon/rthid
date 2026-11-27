//! Flat graphic masses from a blurred ink field with nested foreground masks.
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

/// Maximum analysis width all distance parameters are specified at.
const ANALYSIS_WIDTH: u32 = 450;

/// Fixed monochrome tones: background, base, shadow, accent, independent of invert.
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

/// Fixed warm tones for style comparison, independent of invert.
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

/// Four tones sampled from a palette as a gradient from its lightest to its darkest color.
///
/// A local copy is stably sorted by luma ascending. Tones sit at 0, 1/3, 2/3 and 1 of the
/// gradient, so four colors keep their values and two colors get two blended steps.
/// `options.invert` reverses the direction for light ink on dark backgrounds. Fewer than
/// two colors fall back to the fixed [`MONO`] tones regardless of invert.
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

/// Rec. 601 luma in [0, 1], shared by ink extraction and tone ordering.
fn luma(rgb: &[u8; 3]) -> f32 {
    (0.299 * rgb[0] as f32 + 0.587 * rgb[1] as f32 + 0.114 * rgb[2] as f32) / 255.0
}

/// Controls smoothing and the three cumulative ink masks.
#[derive(Debug, Clone, PartialEq)]
pub struct MassOptions {
    /// Gaussian sigma at 450 pixels, scaled by the actual analysis width. Zero disables blur.
    pub sigma: f32,
    /// Strictly increasing base, shadow and accent thresholds in (0, 1]. Higher covers less area.
    pub thresholds: [f32; 3],
    /// Treat light pixels as ink for light on dark images.
    pub invert: bool,
}

impl Default for MassOptions {
    fn default() -> Self {
        Self {
            sigma: 4.5,
            thresholds: [0.25, 0.72, 0.90],
            invert: false,
        }
    }
}

/// Four tone grid at analysis size, from the lightest background (0) to accent (3).
///
/// Tone order is opposite to dither palette order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MassStudy {
    /// One tone index per cell in row major order.
    indices: Vec<u8>,
    /// Number of columns.
    w: usize,
    /// Number of rows.
    h: usize,
}

impl MassStudy {
    /// Number of analysis columns, at most 450.
    pub fn w(&self) -> usize {
        self.w
    }

    /// Number of analysis rows, preserving the source aspect ratio.
    pub fn h(&self) -> usize {
        self.h
    }

    /// Tone indices in row major order, from background (0) to accent (3).
    pub fn indices(&self) -> &[u8] {
        &self.indices
    }

    /// Opaque RGBA at any output size with nearest neighbor sampling.
    ///
    /// Panics on output size overflow.
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

/// Invalid analysis options or an empty source image.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MassError {
    /// Sigma must be finite and nonnegative.
    InvalidSigma {
        /// Rejected blur sigma.
        sigma: f32,
    },
    /// Thresholds must be finite, strictly increasing and in (0, 1].
    InvalidThresholds {
        /// Rejected cumulative mask thresholds.
        thresholds: [f32; 3],
    },
    /// Both source dimensions must be positive.
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

/// Computes a mass study from the undithered RGBA source image.
///
/// The returned grid is at most 450 columns wide with the source aspect ratio preserved.
/// [`MassStudy::rgba`] resamples it to any output size.
///
/// Ink is relative to the image's own darkest and lightest pixels with nonzero
/// alpha, so two color images map to 0 and 1 and continuous tone images use
/// their full range before alpha scales coverage. Uniform or fully transparent
/// images have zero ink because they contain no visible luma contrast.
///
/// Panics on mismatched RGBA dimensions or image size overflow.
#[track_caller]
pub fn mass_study(
    rgba: &[u8],
    w: usize,
    h: usize,
    options: &MassOptions,
) -> Result<MassStudy, MassError> {
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
    if w == 0 || h == 0 {
        return Err(MassError::EmptyImage);
    }
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
    let aw = w.min(ANALYSIS_WIDTH as usize);
    let ah = ((h as f64 * aw as f64 / w as f64).round_ties_even() as usize).max(1);
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
    let mut density = image::imageops::resize(
        &source,
        aw as u32,
        u32::try_from(ah)
            .unwrap_or_else(|_| panic!("analysis height {ah} exceeds {} pixels", u32::MAX)),
        FilterType::Triangle,
    )
    .into_raw();
    blur(
        &mut density,
        aw,
        ah,
        options.sigma as f64 * aw as f64 / ANALYSIS_WIDTH as f64,
    );
    let masks = masks(&density, aw, ah, &options.thresholds);
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
    })
}

fn reflect(position: i64, len: usize) -> usize {
    if len == 1 {
        return 0;
    }
    let period = 2 * (len as i64 - 1);
    let p = position.rem_euclid(period);
    p.min(period - p) as usize
}

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

fn masks(density: &[f32], w: usize, h: usize, thresholds: &[f32; 3]) -> Vec<Vec<bool>> {
    let radius = (3.0 * w as f64 / ANALYSIS_WIDTH as f64).round_ties_even() as isize;
    let mut result: Vec<Vec<bool>> = Vec::new();
    for i in 0..3 {
        let mut raw = Vec::with_capacity(density.len());
        for &value in density {
            raw.push(value >= thresholds[i]);
        }
        let mut mask = clean_mask(
            &raw,
            w,
            h,
            radius,
            ([0.003, 0.003, 0.0015][i] * (w * h) as f64).ceil() as usize,
            [3, 6, 5][i],
            (0.003 * (w * h) as f64).floor() as usize,
        );
        if let Some(parent) = result.last() {
            for (cell, &p) in mask.iter_mut().zip(parent) {
                *cell &= p;
            }
        }
        result.push(mask);
    }
    result
}

fn morphology(mask: &[bool], w: usize, h: usize, radius: isize, dilate: bool) -> Vec<bool> {
    let mut out = vec![!dilate; mask.len()];
    for y in 0..h {
        for x in 0..w {
            'disk: for dy in -radius..=radius {
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
    for hole in components(&out, w, h, false, false) {
        if hole.len() <= max_hole
            && !hole
                .iter()
                .any(|&p| p % w == 0 || p % w == w - 1 || p / w == 0 || p / w == h - 1)
        {
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

    fn check_masks(masks: &[Vec<bool>]) {
        for pair in masks.windows(2) {
            assert!(
                pair[1]
                    .iter()
                    .zip(&pair[0])
                    .all(|(&child, &parent)| !child || parent)
            );
        }
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
        let cumulative: Vec<Vec<bool>> = (1..=3)
            .map(|i| result.indices().iter().map(|&v| v >= i).collect())
            .collect();
        check_masks(&cumulative);
        assert_eq!(
            tone_counts(&result).iter().sum::<usize>(),
            result.w() * result.h()
        );
        result
    }

    fn grid(rows: &[&str]) -> Vec<bool> {
        rows.iter()
            .flat_map(|row| row.bytes().map(|c| c == b'#'))
            .collect()
    }

    #[test]
    fn palette_tones_preserve_four_colors_in_background_first_order() {
        let mut palette = WARM;
        palette.reverse();
        assert_eq!(tones_from_palette(&palette, &MassOptions::default()), WARM);
    }

    #[test]
    fn palette_tones_sort_without_changing_the_input() {
        let palette = [WARM[2], WARM[0], WARM[3], WARM[1]];
        assert_eq!(tones_from_palette(&palette, &MassOptions::default()), WARM);
        assert_eq!(palette, [WARM[2], WARM[0], WARM[3], WARM[1]]);
    }

    #[test]
    fn palette_tones_blend_two_colors_with_rounding() {
        let palette = [
            PaletteColor { rgb: [0, 10, 20] },
            PaletteColor {
                rgb: [100, 110, 120],
            },
        ];
        assert_eq!(
            tones_from_palette(&palette, &MassOptions::default()).map(|tone| tone.rgb),
            [[100, 110, 120], [67, 77, 87], [33, 43, 53], [0, 10, 20]]
        );
    }

    #[test]
    fn palette_tones_sample_sixteen_colors() {
        let palette: [_; 16] = std::array::from_fn(|i| PaletteColor {
            rgb: [i as u8 * 16; 3],
        });
        assert_eq!(
            tones_from_palette(&palette, &MassOptions::default()),
            [palette[15], palette[10], palette[5], palette[0]]
        );
    }

    #[test]
    fn inverted_palette_tones_reverse_the_gradient() {
        let palette = [
            PaletteColor { rgb: [0, 10, 20] },
            PaletteColor {
                rgb: [100, 110, 120],
            },
        ];
        let mut expected = tones_from_palette(&palette, &MassOptions::default());
        expected.reverse();
        assert_eq!(
            tones_from_palette(
                &palette,
                &MassOptions {
                    invert: true,
                    ..MassOptions::default()
                }
            ),
            expected
        );
    }

    #[test]
    fn palette_tones_fall_back_to_mono_for_fewer_than_two_colors() {
        for invert in [false, true] {
            assert_eq!(
                tones_from_palette(
                    &[],
                    &MassOptions {
                        invert,
                        ..MassOptions::default()
                    }
                ),
                MONO
            );
            assert_eq!(
                tones_from_palette(
                    &WARM[..1],
                    &MassOptions {
                        invert,
                        ..MassOptions::default()
                    }
                ),
                MONO
            );
        }
    }

    #[test]
    fn validates_input() {
        let options = MassOptions::default();
        assert_eq!(mass_study(&[], 0, 2, &options), Err(MassError::EmptyImage));
        assert_eq!(mass_study(&[], 2, 0, &options), Err(MassError::EmptyImage));
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
    fn removes_small_components_and_breaks_ties_by_first_cell() {
        let input = grid(&["##...##", ".......", "...#..."]);
        let cleaned = clean_mask(&input, 7, 3, 0, 2, 1, 0);
        assert_eq!(cleaned, grid(&["##.....", ".......", "......."]));
        assert_eq!(clean_mask(&input, 7, 3, 0, 3, 3, 0), vec![false; 21]);
    }

    #[test]
    fn fills_only_small_enclosed_holes() {
        let input = grid(&["#.######", "###.####", "########", "##..####", "########"]);
        let expected = grid(&["#.######", "########", "########", "##..####", "########"]);
        assert_eq!(clean_mask(&input, 8, 5, 0, 1, 1, 1), expected);
    }

    #[test]
    fn foreground_is_eight_connected_and_background_is_four_connected() {
        let diagonal = grid(&["#..", ".#.", "..#"]);
        assert_eq!(components(&diagonal, 3, 3, true, true).len(), 1);
        assert_eq!(clean_mask(&diagonal, 3, 3, 0, 3, 1, 0), diagonal);
        let holes = grid(&[".####", "#.###", "##.##", "#####", "#####"]);
        assert_eq!(
            clean_mask(&holes, 5, 5, 0, 1, 1, 1),
            grid(&[".####", "#####", "#####", "#####", "#####"])
        );
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
        let masks = masks(&density, 100, 100, &[0.25, 0.72, 0.90]);
        check_masks(&masks);
        assert_eq!(components(&masks[0], 100, 100, true, true).len(), 3);
        assert_eq!(masks[0], masks[1]);
        assert!(masks[2].iter().all(|&cell| !cell));
    }

    #[test]
    fn closing_replicates_edges_and_joins_a_gap() {
        let input = grid(&["##.##", "##.##", "##.##"]);
        assert_eq!(clean_mask(&input, 5, 3, 1, 1, 1, 0), vec![true; 15]);
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
    fn deterministic_and_invert_preserves_geometry() {
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
        assert_eq!(first, study(&rgba, 30, 20, &options));
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
