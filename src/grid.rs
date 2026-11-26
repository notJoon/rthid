//! Source image, print grid and dithered indices caches.

use crate::{memo::Memo, print::Placement, settings::Settings};
use image::{RgbaImage, imageops};
use rthid_dither::dither::{self, Options};

type GridKey = Placement;

/// Grid build version and the settings the indices were dithered with.
///
/// The options come from [`Settings::options`], which holds only the values the
/// current algorithm uses. Remembered inactive values, such as a diffusion
/// strength while Bayer is selected, therefore never trigger a new dither.
/// Comparing whole Settings here, or keeping a second copy of the algorithm,
/// would lose that property.
#[derive(PartialEq)]
pub(crate) struct DitherKey {
    grid: u64,
    options: Options,
}

/// Source image, card grid and dithered indices. Each stage
/// rebuilds only when its key changes.
pub struct GridCache {
    /// Loaded image before scaling and rotation.
    pub(crate) source: Option<RgbaImage>,
    /// Composed card image cache.
    pub(crate) grid: Memo<GridKey, RgbaImage>,
    /// Palette indices from the last requested dither.
    pub(crate) dithered: Memo<DitherKey, Vec<u8>>,
}

impl GridCache {
    /// Whether an image is loaded.
    pub fn has_source(&self) -> bool {
        self.source.is_some()
    }

    /// Replaces the image and invalidates both caches without resetting versions.
    pub fn set_source(&mut self, image: RgbaImage) {
        self.source = Some(image);
        self.grid.clear();
        self.dithered.clear();
    }

    /// Current card placement when a source is loaded.
    pub(crate) fn placement(&self, settings: &Settings) -> Option<Placement> {
        Placement::new(self.source.as_ref()?.dimensions(), settings).ok()
    }

    /// Rebuilds the grid when the card placement changed and, if
    /// `need_dither` is set, the indices when the grid or dither options changed.
    /// Returns true when new indices were built.
    pub fn refresh(&mut self, settings: &Settings, need_dither: bool) -> bool {
        let Some(source) = &self.source else {
            return false;
        };
        let Some(placement) = self.placement(settings) else {
            return false;
        };
        self.grid.update(placement, || {
            let mut grid =
                RgbaImage::from_pixel(placement.width, placement.height, image::Rgba([255; 4]));
            if placement.aw > 0 {
                let mut rotated = placement.rotation.apply(source.clone());
                if placement.flip {
                    imageops::flip_horizontal_in_place(&mut rotated);
                }
                let left = (-placement.x).max(0) as u32;
                let top = (-placement.y).max(0) as u32;
                let right =
                    (i64::from(placement.aw) - placement.x).min(i64::from(placement.sw)) as u32;
                let bottom =
                    (i64::from(placement.ah) - placement.y).min(i64::from(placement.sh)) as u32;
                let resized = resize_region(
                    &rotated,
                    (placement.sw, placement.sh),
                    (left, top, right - left, bottom - top),
                );
                for y in 0..placement.ah {
                    for x in 0..placement.aw {
                        let sx = i64::from(x) - placement.x;
                        let sy = i64::from(y) - placement.y;
                        if sx >= 0
                            && sx < i64::from(placement.sw)
                            && sy >= 0
                            && sy < i64::from(placement.sh)
                        {
                            grid.put_pixel(
                                x + placement.inset,
                                y + placement.inset,
                                *resized.get_pixel(sx as u32 - left, sy as u32 - top),
                            );
                        }
                    }
                }
            }
            grid
        });
        if !need_dither {
            return false;
        }
        let grid = self.grid.get().expect("processed grid");
        let version = self.grid_version();
        let options = settings.options();
        self.dithered.update_by(
            |key| key.grid == version && key.options == options,
            || DitherKey {
                grid: version,
                options: options.clone(),
            },
            || {
                let paper = (options.palette.len() - 1) as u8;
                let mut indices = vec![paper; (placement.width * placement.height) as usize];
                if placement.aw > 0 {
                    let area = imageops::crop_imm(
                        grid,
                        placement.inset,
                        placement.inset,
                        placement.aw,
                        placement.ah,
                    )
                    .to_image();
                    let pixels = dither::dither(
                        area.as_raw(),
                        placement.aw as usize,
                        placement.ah as usize,
                        &options,
                    );
                    for y in 0..placement.ah {
                        for x in 0..placement.aw {
                            let sx = i64::from(x) - placement.x;
                            let sy = i64::from(y) - placement.y;
                            if sx >= 0
                                && sx < i64::from(placement.sw)
                                && sy >= 0
                                && sy < i64::from(placement.sh)
                            {
                                indices[((y + placement.inset) * placement.width
                                    + x
                                    + placement.inset)
                                    as usize] = pixels[(y * placement.aw + x) as usize];
                            }
                        }
                    }
                }
                indices
            },
        )
    }

    /// The composed card image that is shown as Original and dithered.
    pub fn grid(&self) -> Option<&RgbaImage> {
        self.grid.get()
    }

    /// Number of grid builds, so preview caches can tell when the grid changed.
    pub fn grid_version(&self) -> u64 {
        self.grid.version()
    }

    /// Indices from the last dither, `None` before the first. They match the
    /// current grid only after a refresh that requested dithering.
    pub fn dithered(&self) -> Option<&Vec<u8>> {
        self.dithered.get()
    }
}

// Only sample visible output pixels to keep extreme source aspect ratios affordable.
fn resize_region(source: &RgbaImage, size: (u32, u32), region: (u32, u32, u32, u32)) -> RgbaImage {
    let (left, top, width, height) = region;
    if u64::from(size.0) * u64::from(size.1) <= crate::settings::MAX_DOTS {
        let resized = imageops::resize(source, size.0, size.1, imageops::FilterType::Triangle);
        return imageops::crop_imm(&resized, left, top, width, height).to_image();
    }
    triangle_region(source, size, region)
}

// Keep the same separable Triangle weights and float precision as the image filter.
fn triangle_region(
    source: &RgbaImage,
    size: (u32, u32),
    region: (u32, u32, u32, u32),
) -> RgbaImage {
    let (left, top, width, height) = region;
    let mut intermediate = image::Rgba32FImage::new(source.width(), height);
    for y in 0..height {
        let (start, weights) = triangle_weights(source.height(), size.1, y + top);
        for x in 0..source.width() {
            let mut pixel = [0.0; 4];
            for (i, weight) in weights.iter().enumerate() {
                let input = source.get_pixel(x, start + i as u32);
                for c in 0..4 {
                    pixel[c] += f32::from(input[c]) * weight;
                }
            }
            intermediate.put_pixel(x, y, image::Rgba(pixel));
        }
    }
    let mut output = RgbaImage::new(width, height);
    for x in 0..width {
        let (start, weights) = triangle_weights(source.width(), size.0, x + left);
        for y in 0..height {
            let mut pixel = [0.0; 4];
            for (i, weight) in weights.iter().enumerate() {
                let input = intermediate.get_pixel(start + i as u32, y);
                for c in 0..4 {
                    pixel[c] += input[c] * weight;
                }
            }
            output.put_pixel(
                x,
                y,
                image::Rgba(pixel.map(|v| v.clamp(0.0, 255.0).round() as u8)),
            );
        }
    }
    output
}

fn triangle_weights(input: u32, output: u32, position: u32) -> (u32, Vec<f32>) {
    let ratio = input as f32 / output as f32;
    let support = ratio.max(1.0);
    let center = (position as f32 + 0.5) * ratio;
    let start = ((center - support).floor() as i64).clamp(0, i64::from(input) - 1) as u32;
    let end =
        ((center + support).ceil() as i64).clamp(i64::from(start) + 1, i64::from(input)) as u32;
    let center = center - 0.5;
    let mut weights: Vec<_> = (start..end)
        .map(|i| (1.0 - ((i as f32 - center) / support).abs()).max(0.0))
        .collect();
    let sum: f32 = weights.iter().sum();
    for w in &mut weights {
        *w /= sum;
    }
    (start, weights)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn cache(source: RgbaImage) -> GridCache {
        GridCache {
            source: Some(source),
            grid: Memo::default(),
            dithered: Memo::default(),
        }
    }

    #[test]
    fn flip_mirrors_rotated_pixels_before_cover_and_pan() {
        use crate::settings::Rotation;

        let source = RgbaImage::from_fn(16, 12, |x, y| Rgba([x as u8, y as u8, 0, 255]));
        for rotation in [Rotation::R0, Rotation::R90] {
            for cropped in [false, true] {
                let (width, height) = match (rotation, cropped) {
                    (_, true) => (8, 8),
                    (Rotation::R0, false) => (16, 12),
                    _ => (12, 16),
                };
                let settings = Settings {
                    rotation,
                    flip: true,
                    zoom: if cropped { 150 } else { 100 },
                    pan_x: if cropped { 100 } else { 0 },
                    pan_y: if cropped { -100 } else { 0 },
                    ..Settings::for_pixels(width, height)
                };
                let mut cache = cache(source.clone());
                cache.refresh(&settings, false);
                let grid = cache.grid().unwrap();
                for y in 0..height {
                    for x in 0..width {
                        let (sx, sy) = match rotation {
                            Rotation::R0 => (width - 1 - x, y),
                            _ => (y, x + if cropped { 4 } else { 0 }),
                        };
                        assert_eq!(grid.get_pixel(x, y), source.get_pixel(sx, sy));
                    }
                }
            }
        }
    }

    #[test]
    fn margins_and_uncovered_pixels_stay_paper_despite_tone_and_palette() {
        let mut settings = Settings {
            margin: 10,
            zoom: 50,
            ..Settings::for_pixels(12, 12)
        };
        settings.palette = crate::settings::build_palette(&["#123", "#fed"]).unwrap();
        settings.tone.brightness = -0.5;
        settings.tone.contrast = 1.0;
        let mut cache = cache(RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255])));
        assert!(cache.refresh(&settings, true));
        let p = cache.placement(&settings).unwrap();
        assert_eq!(
            (p.inset, p.aw, p.ah, p.sw, p.sh, p.x, p.y),
            (1, 10, 10, 5, 5, 3, 3)
        );
        let grid = cache.grid().unwrap();
        let area = imageops::crop_imm(grid, p.inset, p.inset, p.aw, p.ah).to_image();
        let dithered_area = dither::dither(area.as_raw(), 10, 10, &settings.options());
        for y in 0..12 {
            for x in 0..12 {
                let covered = (4..9).contains(&x) && (4..9).contains(&y);
                assert_eq!(
                    grid.get_pixel(x, y).0,
                    if covered { [0, 0, 0, 255] } else { [255; 4] }
                );
                let expected = if covered {
                    dithered_area[((y - 1) * 10 + x - 1) as usize]
                } else {
                    1
                };
                assert_eq!(cache.dithered().unwrap()[(y * 12 + x) as usize], expected);
            }
        }
        settings.margin = 100;
        assert!(cache.refresh(&settings, true));
        assert!(cache.grid().unwrap().pixels().all(|p| p.0 == [255; 4]));
        assert!(cache.dithered().unwrap().iter().all(|&i| i == 1));
        settings.palette = crate::settings::build_palette(&["#000", "#777", "#fff"]).unwrap();
        let version = cache.grid_version();
        assert!(cache.refresh(&settings, true));
        assert_eq!(cache.grid_version(), version);
        assert!(cache.dithered().unwrap().iter().all(|&i| i == 2));
    }

    #[test]
    fn clipped_triangle_matches_full_resize_exactly() {
        for (iw, ih) in [(1, 9), (9, 1), (17, 13), (8, 8)] {
            let source = RgbaImage::from_fn(iw, ih, |x, y| {
                Rgba([
                    (x * 19 + y * 7) as u8,
                    (x * 31 + y * 23) as u8,
                    (x * 11 + y * 37) as u8,
                    (x * 43 + y * 17) as u8,
                ])
            });
            for size in [(3, 5), (25, 27), (8, 8), (101, 2)] {
                let expected =
                    imageops::resize(&source, size.0, size.1, imageops::FilterType::Triangle);
                for (x, y, w, h) in [(0, 0, size.0, size.1), (1, 1, size.0 - 2, size.1 - 1)] {
                    assert_eq!(
                        triangle_region(&source, size, (x, y, w, h)),
                        imageops::crop_imm(&expected, x, y, w, h).to_image(),
                        "{iw}x{ih} -> {size:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn extreme_source_aspect_uses_bounded_resize() {
        let mut settings = Settings {
            zoom: 400,
            ..Settings::default()
        };
        for source in [
            RgbaImage::from_pixel(1024, 1, Rgba([0, 0, 0, 255])),
            RgbaImage::from_pixel(1, 1024, Rgba([0, 0, 0, 255])),
        ] {
            let mut cache = cache(source);
            settings.pan_x = 100;
            settings.pan_y = -100;
            assert!(cache.refresh(&settings, true));
            assert_eq!(cache.grid().unwrap().dimensions(), (250, 450));
            assert!(cache.dithered().unwrap().iter().all(|&i| i == 0));
        }
    }
}
