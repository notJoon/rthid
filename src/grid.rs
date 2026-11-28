//! Source image, print grid and processed preview caches.

use crate::{document::History, memo::Memo, print::Placement, settings::Settings};
use image::{Rgba, RgbaImage, imageops};
use rthid_dither::{
    dither::{self, Options, PaletteColor},
    mass::{self, MassError, MassOptions},
};

/// Placement and source composition used to build the card grid.
#[derive(PartialEq)]
pub(crate) struct GridKey {
    placement: Placement,
    /// Composed generation, `None` when the document leaves the source unchanged.
    composed: Option<u64>,
}

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

/// Grid build and the geometry and colors of the mass study.
#[derive(PartialEq)]
pub(crate) struct MassKey {
    grid: u64,
    options: MassOptions,
    tones: [PaletteColor; 4],
}

/// Mass image and the diagnostic from the same build.
pub(crate) struct MassBuild {
    image: RgbaImage,
    study: Option<mass::MassStudy>,
    error: Option<(MassError, String)>,
}

/// Source image, card grid, dithered indices and mass image. Each stage
/// rebuilds only when its key changes.
#[derive(Default)]
pub struct GridCache {
    /// Loaded image before scaling and rotation.
    pub(crate) source: Option<RgbaImage>,
    /// Source with visible exposure layers applied, keyed by history revision.
    composed: Option<RgbaImage>,
    composed_revision: Option<u64>,
    composed_generation: u64,
    /// Composed card image cache.
    pub(crate) grid: Memo<GridKey, RgbaImage>,
    /// Palette indices from the last requested dither.
    pub(crate) dithered: Memo<DitherKey, Vec<u8>>,
    /// Four tone colors from the last requested mass study.
    pub(crate) mass: Memo<MassKey, MassBuild>,
}

impl GridCache {
    /// Whether an image is loaded.
    pub fn has_source(&self) -> bool {
        self.source.is_some()
    }

    /// Replaces the image and invalidates all caches without resetting versions.
    pub fn set_source(&mut self, image: RgbaImage) {
        self.source = Some(image);
        self.composed = None;
        self.composed_revision = None;
        self.grid.clear();
        self.dithered.clear();
        self.mass.clear();
    }

    /// Current card placement when a source is loaded.
    pub(crate) fn placement(&self, settings: &Settings) -> Option<Placement> {
        Placement::new(self.source.as_ref()?.dimensions(), settings).ok()
    }

    /// Rebuilds the grid and requested preview pixels when their inputs change.
    /// Returns true when new dithered indices were built.
    pub fn refresh(
        &mut self,
        edits: &History,
        settings: &Settings,
        need_dither: bool,
        need_mass: bool,
        mass_options: &MassOptions,
        tones: &[PaletteColor; 4],
    ) -> bool {
        let Some(source) = &self.source else {
            return false;
        };
        let Some(placement) = self.placement(settings) else {
            return false;
        };
        if self.composed_revision != Some(edits.revision()) {
            let composed = edits.current().compose(source);
            if composed != self.composed {
                self.composed_generation += 1;
                self.composed = composed;
            }
            self.composed_revision = Some(edits.revision());
        }
        let source = self.composed.as_ref().unwrap_or(source);
        self.grid.update(
            GridKey {
                placement,
                composed: self.composed.as_ref().map(|_| self.composed_generation),
            },
            || {
                let mut grid =
                    RgbaImage::from_pixel(placement.width, placement.height, image::Rgba([255; 4]));
                if placement.aw > 0 {
                    let mut rotated = placement.rotation.apply(source.clone());
                    if placement.flip {
                        imageops::flip_horizontal_in_place(&mut rotated);
                    }
                    let (x0, y0, x1, y1) = placement.covered();
                    let left = (i64::from(x0) - placement.x) as u32;
                    let top = (i64::from(y0) - placement.y) as u32;
                    let resized = resize_region(
                        &rotated,
                        (placement.sw, placement.sh),
                        (left, top, x1 - x0, y1 - y0),
                    );
                    imageops::replace(
                        &mut grid,
                        &resized,
                        i64::from(placement.inset + x0),
                        i64::from(placement.inset + y0),
                    );
                }
                grid
            },
        );
        let grid = self.grid.get().expect("processed grid");
        let (w, h) = grid.dimensions();
        let version = self.grid_version();
        if need_mass {
            self.mass.update(
                MassKey {
                    grid: version,
                    options: mass_options.clone(),
                    tones: *tones,
                },
                || match mass::mass_study(grid.as_raw(), w as usize, h as usize, mass_options) {
                    Ok(study) => MassBuild {
                        image: RgbaImage::from_raw(w, h, study.rgba(w as usize, h as usize, tones))
                            .expect("mass grid dimensions"),
                        study: Some(study),
                        error: None,
                    },
                    Err(error) => MassBuild {
                        image: RgbaImage::from_pixel(
                            w,
                            h,
                            Rgba([tones[0].rgb[0], tones[0].rgb[1], tones[0].rgb[2], 255]),
                        ),
                        study: None,
                        error: Some((error, format!("mass study: {error}"))),
                    },
                },
            );
        }
        if !need_dither {
            return false;
        }
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
                    let (x0, y0, x1, y1) = placement.covered();
                    let (x0, x1) = (x0 as usize, x1 as usize);
                    for y in y0..y1 {
                        let src = (y * placement.aw) as usize;
                        let dst =
                            ((y + placement.inset) * placement.width + placement.inset) as usize;
                        indices[dst + x0..dst + x1].copy_from_slice(&pixels[src + x0..src + x1]);
                    }
                }
                indices
            },
        )
    }

    /// Last mass image, current after a refresh that requested it.
    pub fn mass(&self) -> Option<&RgbaImage> {
        self.mass.get().map(|build| &build.image)
    }

    /// Last successful mass study, current after a refresh that requested it.
    pub fn mass_study(&self) -> Option<&mass::MassStudy> {
        self.mass.get()?.study.as_ref()
    }

    /// Error and status text from the last mass build, cleared on success.
    pub fn mass_error(&self) -> Option<(&MassError, &str)> {
        self.mass
            .get()?
            .error
            .as_ref()
            .map(|(error, text)| (error, text.as_str()))
    }

    /// Number of mass builds for preview cache invalidation.
    pub fn mass_version(&self) -> u64 {
        self.mass.version()
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
    use crate::{document::Document, settings::Rotation};

    fn cache(source: RgbaImage) -> GridCache {
        GridCache {
            source: Some(source),
            ..GridCache::default()
        }
    }

    fn empty_edits(cache: &GridCache) -> History {
        let source = cache.source.as_ref().unwrap();
        History::new(Document::new(source.width(), source.height()))
    }

    #[test]
    fn edits_rebuild_grid_and_indices_only_when_revision_changes() {
        use crate::document::STOP;

        let source = RgbaImage::from_pixel(8, 8, Rgba([100, 100, 100, 255]));
        let mut cache = cache(source.clone());
        let mut edits = empty_edits(&cache);
        let settings = Settings::for_pixels(8, 8);
        let options = MassOptions::default();
        assert!(cache.refresh(&edits, &settings, true, false, &options, &mass::MONO));
        assert_eq!(cache.grid(), Some(&source));
        let before = cache.grid_version();
        let indices = cache.dithered().unwrap().clone();
        edits.edit(|doc| {
            doc.add_layer("Exposure".into());
        });
        assert!(!cache.refresh(&edits, &settings, true, false, &options, &mass::MONO));
        assert_eq!(cache.grid_version(), before);
        edits.amend(|doc| doc.target_mut().unwrap().exposure.add(0, 0, STOP));
        assert!(cache.refresh(&edits, &settings, true, false, &options, &mass::MONO));
        assert!(cache.grid_version() > before);
        assert_ne!(cache.dithered().unwrap(), &indices);
        assert_eq!(
            cache.grid().unwrap().get_pixel(0, 0).0,
            [200, 200, 200, 255]
        );
        let versions = (
            cache.composed_generation,
            cache.grid_version(),
            cache.dithered.version(),
        );
        assert!(!cache.refresh(&edits, &settings, true, false, &options, &mass::MONO));
        assert_eq!(
            versions,
            (
                cache.composed_generation,
                cache.grid_version(),
                cache.dithered.version()
            )
        );
        assert!(edits.undo());
        assert!(cache.refresh(&edits, &settings, true, false, &options, &mass::MONO));
        assert_eq!(cache.grid(), Some(&source));
        assert_eq!(cache.dithered(), Some(&indices));
        cache.set_source(RgbaImage::from_pixel(8, 8, Rgba([50, 50, 50, 255])));
        assert!(cache.composed.is_none());
        assert!(cache.refresh(&edits, &settings, true, false, &options, &mass::MONO));
        assert_eq!(cache.grid().unwrap().get_pixel(0, 0).0, [50, 50, 50, 255]);
    }

    #[test]
    fn empty_layers_above_edits_keep_grid_and_dither_versions() {
        use crate::document::STOP;

        let mut cache = cache(RgbaImage::from_pixel(8, 8, Rgba([100, 100, 100, 255])));
        let mut edits = empty_edits(&cache);
        let settings = Settings::for_pixels(8, 8);
        let options = MassOptions::default();
        edits.edit(|doc| {
            doc.add_layer("Exposure".into());
            doc.target_mut().unwrap().exposure.add(0, 0, STOP);
        });
        assert!(cache.refresh(&edits, &settings, true, false, &options, &mass::MONO));
        let grid_version = cache.grid_version();
        let dither_version = cache.dithered.version();
        let composed_generation = cache.composed_generation;

        edits.edit(|doc| {
            doc.add_layer("Empty".into());
        });
        assert!(!cache.refresh(&edits, &settings, true, false, &options, &mass::MONO));
        assert_eq!(cache.grid_version(), grid_version);
        assert_eq!(cache.dithered.version(), dither_version);
        assert_eq!(cache.composed_generation, composed_generation);

        edits.edit(|doc| doc.set_visible(1, false));
        assert!(!cache.refresh(&edits, &settings, true, false, &options, &mass::MONO));
        assert_eq!(cache.grid_version(), grid_version);
        assert_eq!(cache.dithered.version(), dither_version);
        assert_eq!(cache.composed_generation, composed_generation);

        edits.edit(|doc| {
            doc.select(Some(0));
            doc.target_mut().unwrap().exposure.add(1, 0, STOP);
        });
        assert!(cache.refresh(&edits, &settings, true, false, &options, &mass::MONO));
        assert!(cache.grid_version() > grid_version);
        assert!(cache.dithered.version() > dither_version);
        assert!(cache.composed_generation > composed_generation);
        assert_eq!(
            cache.grid().unwrap().get_pixel(1, 0).0,
            [200, 200, 200, 255]
        );
    }

    #[test]
    fn inverse_placement_matches_coordinate_pixels_for_every_orientation_and_crop() {
        let source = RgbaImage::from_fn(30, 20, |x, y| Rgba([x as u8, y as u8, 0, 255]));
        for rotation in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
            for flip in [false, true] {
                for (zoom, pan_x, pan_y) in [(100, 0, 0), (175, 65, -40), (60, -30, 55)] {
                    let settings = Settings {
                        rotation,
                        flip,
                        zoom,
                        pan_x,
                        pan_y,
                        margin: 4,
                        ..Settings::default()
                    };
                    let mut cache = cache(source.clone());
                    cache.refresh(
                        &empty_edits(&cache),
                        &settings,
                        false,
                        false,
                        &MassOptions::default(),
                        &mass::MONO,
                    );
                    let p = cache.placement(&settings).unwrap();
                    let (x0, y0, x1, y1) = p.covered();
                    for (x, y, pixel) in cache.grid().unwrap().enumerate_pixels() {
                        let mapped =
                            p.source_point((f64::from(x), f64::from(y)), source.dimensions());
                        let covered = x >= p.inset + x0
                            && x < p.inset + x1
                            && y >= p.inset + y0
                            && y < p.inset + y1;
                        assert_eq!(mapped.is_some(), covered);
                        if let Some((sx, sy)) = mapped {
                            assert!(
                                (sx - f64::from(pixel[0])).abs() <= 1.0,
                                "{rotation:?} {flip} {x},{y}: {sx} {pixel:?}"
                            );
                            assert!((sy - f64::from(pixel[1])).abs() <= 1.0);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn image_card_preserves_every_source_pixel_at_each_rotation() {
        for (w, h) in [(30, 20), (1, 2)] {
            let source = RgbaImage::from_fn(w, h, |x, y| Rgba([x as u8, y as u8, 0, 255]));
            for rotation in [Rotation::R0, Rotation::R90, Rotation::R180, Rotation::R270] {
                let mut settings = Settings {
                    rotation,
                    ..Settings::default()
                };
                let mut cache = cache(source.clone());
                cache.refresh(
                    &empty_edits(&cache),
                    &settings,
                    false,
                    false,
                    &MassOptions::default(),
                    &mass::MONO,
                );
                assert_eq!(cache.grid().unwrap(), &rotation.apply(source.clone()));
                settings.zoom = 200;
                settings.pan_x = 100;
                settings.pan_y = -100;
                cache.refresh(
                    &empty_edits(&cache),
                    &settings,
                    false,
                    false,
                    &MassOptions::default(),
                    &mass::MONO,
                );
                let p = cache.placement(&settings).unwrap();
                assert_eq!((p.sw, p.sh), (p.aw * 2, p.ah * 2));
                assert_eq!((p.x, p.y), (-i64::from(p.aw), 0));
            }
        }
    }

    #[test]
    fn flip_mirrors_rotated_pixels_before_cover_and_pan() {
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
                cache.refresh(
                    &empty_edits(&cache),
                    &settings,
                    false,
                    false,
                    &MassOptions::default(),
                    &mass::MONO,
                );
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
        settings.palette = crate::settings::Palette::custom(&["#123", "#fed"]);
        settings.tone.brightness = -0.5;
        settings.tone.contrast = 1.0;
        let mut cache = cache(RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255])));
        assert!(cache.refresh(
            &empty_edits(&cache),
            &settings,
            true,
            false,
            &MassOptions::default(),
            &mass::MONO
        ));
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
        assert!(cache.refresh(
            &empty_edits(&cache),
            &settings,
            true,
            false,
            &MassOptions::default(),
            &mass::MONO
        ));
        assert!(cache.grid().unwrap().pixels().all(|p| p.0 == [255; 4]));
        assert!(cache.dithered().unwrap().iter().all(|&i| i == 1));
        settings.palette = crate::settings::Palette::custom(&["#000", "#777", "#fff"]);
        let version = cache.grid_version();
        assert!(cache.refresh(
            &empty_edits(&cache),
            &settings,
            true,
            false,
            &MassOptions::default(),
            &mass::MONO
        ));
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
            card: crate::settings::Card::from_id("business-portrait"),
            ..Settings::default()
        };
        for source in [
            RgbaImage::from_pixel(1024, 1, Rgba([0, 0, 0, 255])),
            RgbaImage::from_pixel(1, 1024, Rgba([0, 0, 0, 255])),
        ] {
            let mut cache = cache(source);
            settings.pan_x = 100;
            settings.pan_y = -100;
            assert!(cache.refresh(
                &empty_edits(&cache),
                &settings,
                true,
                false,
                &MassOptions::default(),
                &mass::MONO
            ));
            assert_eq!(cache.grid().unwrap().dimensions(), (250, 450));
            assert!(cache.dithered().unwrap().iter().all(|&i| i == 0));
        }
    }

    #[test]
    fn mass_cache_tracks_grid_options_and_tones() {
        let mut cache = GridCache::default();
        let mut settings = Settings::for_pixels(8, 12);
        let mut options = MassOptions::default();
        cache.set_source(RgbaImage::new(8, 4));
        cache.refresh(
            &empty_edits(&cache),
            &settings,
            false,
            false,
            &options,
            &mass::MONO,
        );
        assert!(cache.mass().is_none());
        for (tones, expected) in [(mass::MONO, 1), (mass::MONO, 1), (mass::WARM, 2)] {
            assert!(!cache.refresh(
                &empty_edits(&cache),
                &settings,
                false,
                true,
                &options,
                &tones
            ));
            assert_eq!(cache.mass_version(), expected);
            assert_eq!(cache.mass().unwrap().dimensions(), (8, 12));
            assert_eq!(cache.mass().unwrap().get_pixel(0, 0).0[..3], tones[0].rgb);
        }
        options.invert = true;
        cache.refresh(
            &empty_edits(&cache),
            &settings,
            false,
            true,
            &options,
            &mass::WARM,
        );
        assert_eq!(cache.mass_version(), 3);
        settings.rotation = Rotation::R90;
        cache.refresh(
            &empty_edits(&cache),
            &settings,
            false,
            true,
            &options,
            &mass::WARM,
        );
        assert_eq!(cache.mass_version(), 4);
        assert_eq!(cache.mass().unwrap().dimensions(), (8, 12));
        assert!(cache.dithered().is_none());
        cache.set_source(RgbaImage::new(2, 3));
        assert!(cache.mass().is_none());
        cache.refresh(
            &empty_edits(&cache),
            &settings,
            false,
            true,
            &options,
            &mass::WARM,
        );
        assert_eq!(cache.mass_version(), 5);
    }

    #[test]
    fn mass_study_uses_the_composed_card_including_paper() {
        let settings = Settings {
            margin: 10,
            zoom: 50,
            ..Settings::for_pixels(16, 12)
        };
        let mut cache = cache(RgbaImage::from_pixel(16, 12, Rgba([0, 0, 0, 255])));
        let options = MassOptions::default();
        cache.refresh(
            &empty_edits(&cache),
            &settings,
            false,
            true,
            &options,
            &mass::WARM,
        );
        let grid = cache.grid().unwrap();
        assert_eq!(grid.dimensions(), (16, 12));
        assert_eq!(grid.get_pixel(0, 0).0, [255; 4]);
        assert_eq!(grid.get_pixel(8, 6).0, [0, 0, 0, 255]);
        let study = mass::mass_study(grid.as_raw(), 16, 12, &options).unwrap();
        assert_eq!(
            cache.mass().unwrap().as_raw(),
            &study.rgba(16, 12, &mass::WARM)
        );
    }

    #[test]
    fn invalid_mass_options_use_the_background_tone_and_clear_error_on_success() {
        let mut cache = GridCache {
            source: Some(RgbaImage::new(3, 2)),
            ..GridCache::default()
        };
        let options = MassOptions {
            sigma: -1.0,
            ..MassOptions::default()
        };
        cache.refresh(
            &empty_edits(&cache),
            &Settings::for_pixels(8, 8),
            false,
            true,
            &options,
            &mass::WARM,
        );
        let image = cache.mass().unwrap();
        assert_eq!(image.dimensions(), (8, 8));
        assert!(image.pixels().all(|p| p.0 == [255; 4]));
        assert_eq!(
            cache.mass_error().map(|(error, _)| error),
            Some(&MassError::InvalidSigma { sigma: -1.0 })
        );
        assert_eq!(cache.mass_version(), 1);
        cache.refresh(
            &empty_edits(&cache),
            &Settings::for_pixels(8, 8),
            false,
            true,
            &options,
            &mass::WARM,
        );
        assert_eq!(
            cache.mass_error().map(|(error, _)| error),
            Some(&MassError::InvalidSigma { sigma: -1.0 })
        );
        assert_eq!(cache.mass_version(), 1);
        cache.refresh(
            &empty_edits(&cache),
            &Settings::for_pixels(8, 8),
            false,
            true,
            &MassOptions::default(),
            &mass::WARM,
        );
        assert!(cache.mass_error().is_none());
        assert_eq!(cache.mass_version(), 2);
    }
}
