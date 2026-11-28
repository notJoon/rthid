//! Physical PNG sizing and integer placement inside a physical card.

use crate::settings::{Rotation, Settings, round};

/// Rounds each resolved physical axis including bleed to PNG pixels.
/// Returns `InvalidInput` when a dimension is zero or overflows `u32`.
pub(crate) fn png_size(
    settings: &Settings,
    source: (u32, u32),
    dpi: u32,
) -> std::io::Result<(u32, u32)> {
    let dimensions = settings
        .dimensions(source)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let axis = |mm: f64| {
        let pixels = (mm / 25.4 * f64::from(dpi)).round();
        if !(1.0..=f64::from(u32::MAX)).contains(&pixels) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "PNG dimension must be positive and fit in u32",
            ));
        }
        Ok(pixels as u32)
    };
    Ok((axis(dimensions.mm.0)?, axis(dimensions.mm.1)?))
}

/// Placement also serves as the complete geometry cache key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Placement {
    /// Dot grid width.
    pub width: u32,
    /// Dot grid height.
    pub height: u32,
    /// Paper border in dots.
    pub inset: u32,
    /// Image area width, zero when the margin consumes the card.
    pub aw: u32,
    /// Image area height, zero when the margin consumes the card.
    pub ah: u32,
    /// Resized source width.
    pub sw: u32,
    /// Resized source height.
    pub sh: u32,
    /// Source left edge relative to the image area.
    pub x: i64,
    /// Source top edge relative to the image area.
    pub y: i64,
    /// Cover magnification retained when rounding leaves the placement unchanged.
    pub zoom: i32,
    /// Horizontal composition control.
    pub pan_x: i32,
    /// Vertical composition control.
    pub pan_y: i32,
    /// Source orientation inside the card.
    pub rotation: Rotation,
    /// Mirrors the rotated source along the card width.
    pub flip: bool,
}

impl Placement {
    /// Rejects impossible source sizes and placements before allocating pixels.
    pub fn new(source: (u32, u32), settings: &Settings) -> Result<Self, &'static str> {
        let dimensions = settings.dimensions(source)?;
        let (width, height) = dimensions.grid;
        let inset = dimensions.inset;
        let aw = width.saturating_sub(inset.saturating_mul(2));
        let ah = height.saturating_sub(inset.saturating_mul(2));
        let mut result = Self {
            width,
            height,
            inset,
            aw,
            ah,
            sw: 0,
            sh: 0,
            x: 0,
            y: 0,
            rotation: settings.rotation,
            flip: settings.flip,
            zoom: settings.zoom,
            pan_x: settings.pan_x,
            pan_y: settings.pan_y,
        };
        if settings.card.size_mm.is_some() && (aw <= 2 || ah <= 2) {
            result.aw = 0;
            result.ah = 0;
            return Ok(result);
        }
        let (ew, eh) = if settings.rotation.swaps_axes() {
            (source.1, source.0)
        } else {
            source
        };
        let scale = (f64::from(aw) / f64::from(ew)).max(f64::from(ah) / f64::from(eh))
            * f64::from(settings.zoom)
            / 100.0;
        let sw = round(f64::from(ew) * scale).max(1.0);
        let sh = round(f64::from(eh) * scale).max(1.0);
        if !sw.is_finite()
            || !sh.is_finite()
            || sw > f64::from(u32::MAX)
            || sh > f64::from(u32::MAX)
        {
            return Err("resized source overflows");
        }
        result.sw = sw as u32;
        result.sh = sh as u32;
        result.x =
            round(-(sw - f64::from(aw)) * (100.0 + f64::from(settings.pan_x)) / 200.0) as i64;
        result.y =
            round(-(sh - f64::from(ah)) * (100.0 + f64::from(settings.pan_y)) / 200.0) as i64;
        Ok(result)
    }

    /// Maps a dot center back to the original source, excluding paper and uncovered space.
    /// Integer results are source pixel centers. `source` must match the size passed to `new`.
    pub(crate) fn source_point(&self, dot: (f64, f64), source: (u32, u32)) -> Option<(f64, f64)> {
        debug_assert!(source.0 > 0 && source.1 > 0);
        let x = dot.0 - f64::from(self.inset);
        let y = dot.1 - f64::from(self.inset);
        let (x0, y0, x1, y1) = self.covered();
        let ix = (x + 0.5).floor();
        let iy = (y + 0.5).floor();
        if !(ix >= f64::from(x0) && ix < f64::from(x1) && iy >= f64::from(y0) && iy < f64::from(y1))
        {
            return None;
        }
        let (ew, eh) = if self.rotation.swaps_axes() {
            (source.1, source.0)
        } else {
            source
        };
        let mut x = ((x - self.x as f64 + 0.5) * f64::from(ew) / f64::from(self.sw) - 0.5)
            .clamp(0.0, f64::from(ew - 1));
        let y = ((y - self.y as f64 + 0.5) * f64::from(eh) / f64::from(self.sh) - 0.5)
            .clamp(0.0, f64::from(eh - 1));
        if self.flip {
            x = f64::from(ew - 1) - x;
        }
        Some(match self.rotation {
            Rotation::R0 => (x, y),
            Rotation::R90 => (y, f64::from(source.1 - 1) - x),
            Rotation::R180 => (f64::from(source.0 - 1) - x, f64::from(source.1 - 1) - y),
            Rotation::R270 => (f64::from(source.0 - 1) - y, x),
        })
    }

    /// Image area cells the resized source covers, as `(x0, y0, x1, y1)` with exclusive ends.
    pub fn covered(&self) -> (u32, u32, u32, u32) {
        let clip = |offset: i64, size: u32, area: u32| {
            let area = i64::from(area);
            (
                offset.clamp(0, area) as u32,
                (offset + i64::from(size)).clamp(0, area) as u32,
            )
        };
        let (x0, x1) = clip(self.x, self.sw, self.aw);
        let (y0, y1) = clip(self.y, self.sh, self.ah);
        (x0, y0, x1, y1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{CARDS, Card};

    fn grid_dimensions(settings: &Settings) -> Result<(u32, u32), &'static str> {
        Ok(settings.dimensions((1, 1))?.grid)
    }

    #[test]
    fn source_point_uses_dot_footprints_at_paper_edges() {
        let settings = Settings {
            margin: 2,
            ..Settings::default()
        };
        let source = (8, 8);
        let placement = Placement::new(source, &settings).unwrap();
        assert_eq!(placement.inset, 1);
        assert_eq!(
            placement.source_point((0.75, 1.0), source),
            Some((0.0, 0.0))
        );
        assert_eq!(placement.source_point((8.75, 1.0), source), None);
        assert_eq!(
            placement.source_point((1.0, 0.75), source),
            Some((0.0, 0.0))
        );
        assert_eq!(placement.source_point((1.0, 8.75), source), None);
        for y in 0..10 {
            for x in 0..10 {
                let expected = if (1..9).contains(&x) && (1..9).contains(&y) {
                    Some((f64::from(x - 1), f64::from(y - 1)))
                } else {
                    None
                };
                assert_eq!(
                    placement.source_point((f64::from(x), f64::from(y)), source),
                    expected
                );
            }
        }
    }

    #[test]
    fn image_card_follows_source_and_adds_paper_around_it() {
        let mut settings = Settings::default();
        assert_eq!(settings.card.id, "image");
        assert_eq!(settings.card.label, "Image");
        assert_eq!(Card::from_id("unknown"), settings.card);
        assert_eq!(grid_dimensions(&settings), Ok((1, 1)));
        for (rotation, size) in [
            (Rotation::R0, (300, 200)),
            (Rotation::R90, (200, 300)),
            (Rotation::R180, (300, 200)),
            (Rotation::R270, (200, 300)),
        ] {
            settings.rotation = rotation;
            for (margin, bleed, inset) in [(0, 0, 0), (20, 0, 10), (0, 3, 15), (20, 3, 25)] {
                settings.margin = margin;
                settings.bleed = bleed;
                let p = Placement::new((300, 200), &settings).unwrap();
                assert_eq!(
                    (p.width, p.height),
                    (size.0 + 2 * inset, size.1 + 2 * inset)
                );
                assert_eq!((p.inset, p.aw, p.ah), (inset, size.0, size.1));
                assert_eq!((p.sw, p.sh, p.x, p.y), (size.0, size.1, 0, 0));
                assert_eq!(p.covered(), (0, 0, size.0, size.1));
            }
        }
    }

    #[test]
    fn image_physical_size_and_png_follow_dot_size() {
        let mut settings = Settings::default();
        for (dot, mm, png) in [
            (20, (60.0, 40.0), (1417, 945)),
            (40, (120.0, 80.0), (2835, 1890)),
        ] {
            settings.dot = dot;
            let dimensions = settings.dimensions((300, 200)).unwrap();
            assert_eq!(dimensions.grid, (300, 200));
            assert_eq!(dimensions.mm, mm);
            assert_eq!(png_size(&settings, (300, 200), 600).unwrap(), png);
            settings.rotation = Rotation::R90;
            assert_eq!(
                png_size(&settings, (300, 200), 600).unwrap(),
                (png.1, png.0)
            );
            settings.rotation = Rotation::R0;
        }
        settings.dot = 25;
        settings.margin = 20;
        settings.bleed = 3;
        let dimensions = settings.dimensions((300, 200)).unwrap();
        assert_eq!(dimensions.grid, (340, 240));
        assert_eq!(dimensions.mm, (85.0, 60.0));
        assert_eq!(png_size(&settings, (300, 200), 600).unwrap(), (2008, 1417));
        settings.dot = 20;
        settings.margin = 15;
        assert_eq!(settings.dimensions((300, 200)).unwrap().inset, 23);
    }

    #[test]
    fn png_presets_round_each_axis_at_print_resolutions() {
        for (card, expected) in CARDS.into_iter().skip(1).zip([
            [(591, 1063), (1181, 2126), (2362, 4252)],
            [(1063, 591), (2126, 1181), (4252, 2362)],
            [(650, 1004), (1299, 2008), (2598, 4016)],
            [(744, 1039), (1488, 2079), (2976, 4157)],
            [(768, 768), (1535, 1535), (3071, 3071)],
            [(1181, 1748), (2362, 3496), (4724, 6992)],
        ]) {
            let mut settings = Settings {
                card,
                ..Settings::default()
            };
            for (dpi, expected) in [300, 600, 1200].into_iter().zip(expected) {
                assert_eq!(png_size(&settings, (300, 200), dpi).unwrap(), expected);
                settings.dot = 8;
                settings.rotation = Rotation::R90;
                assert_eq!(png_size(&settings, (300, 200), dpi).unwrap(), expected);
            }
        }
        let mut settings = Settings {
            bleed: 3,
            card: Card::from_id("business-portrait"),
            ..Settings::default()
        };
        assert_eq!(png_size(&settings, (300, 200), 600).unwrap(), (1323, 2268));
        settings.card.size_mm = Some((127, 90));
        settings.bleed = 0;
        assert_eq!(png_size(&settings, (300, 200), 1).unwrap().0, 5);
        assert_eq!(png_size(&settings, (300, 200), 3).unwrap().0, 15);
    }

    #[test]
    fn png_size_rejects_zero_and_overflow_before_casting() {
        let mut settings = Settings {
            card: Card::from_id("business-portrait"),
            ..Settings::default()
        };
        assert!(png_size(&settings, (300, 200), 0).is_err());
        settings.card.size_mm = Some((0, 90));
        assert!(png_size(&settings, (300, 200), 600).is_err());
        settings.card.size_mm = Some((u32::MAX, 90));
        settings.bleed = i32::MAX;
        assert!(png_size(&settings, (300, 200), 4800).is_err());
        settings.card = Card::from_id("business-portrait");
        settings.bleed = i32::MIN;
        assert!(png_size(&settings, (300, 200), 600).is_err());
    }

    #[test]
    fn presets_and_axes_round_independently() {
        for (card, expected) in CARDS.into_iter().skip(1).zip([
            (250, 450),
            (450, 250),
            (275, 425),
            (315, 440),
            (325, 325),
            (500, 740),
        ]) {
            let settings = Settings {
                card,
                ..Settings::default()
            };
            assert_eq!(grid_dimensions(&settings), Ok(expected));
        }
        let settings = Settings {
            card: Card::from_id("euro-business"),
            dot: 8,
            ..Settings::default()
        };
        assert_eq!(grid_dimensions(&settings), Ok((688, 1063)));
        let settings = Settings::for_pixels(1, 2);
        assert_eq!(grid_dimensions(&settings), Ok((8, 8)));
    }

    #[test]
    fn margin_zero_covers_bleed_and_positive_margin_insets_it() {
        let mut settings = Settings {
            bleed: 3,
            card: Card::from_id("business-portrait"),
            ..Settings::default()
        };
        let full = Placement::new((100, 100), &settings).unwrap();
        assert_eq!((full.width, full.height, full.inset), (280, 480, 0));
        settings.margin = 1;
        let bordered = Placement::new((100, 100), &settings).unwrap();
        assert_eq!((bordered.inset, bordered.aw, bordered.ah), (16, 248, 448));
    }

    #[test]
    fn consumed_area_is_empty_including_two_dots() {
        for margin in [30, 40, 100, i32::MAX] {
            let settings = Settings {
                margin,
                ..Settings::for_pixels(8, 10)
            };
            let placement = Placement::new((20, 10), &settings).unwrap();
            assert_eq!(
                (placement.aw, placement.ah, placement.sw, placement.sh),
                (0, 0, 0, 0)
            );
        }
    }

    #[test]
    fn cover_and_rotation_turn_the_source_inside_a_fixed_card() {
        let mut settings = Settings::for_pixels(100, 80);
        for (rotation, scaled) in [
            (Rotation::R0, (160, 80)),
            (Rotation::R90, (100, 200)),
            (Rotation::R180, (160, 80)),
            (Rotation::R270, (100, 200)),
        ] {
            settings.rotation = rotation;
            let p = Placement::new((40, 20), &settings).unwrap();
            assert_eq!((p.width, p.height), (100, 80));
            assert_eq!((p.sw, p.sh), scaled);
            assert!(p.sw == p.aw || p.sh == p.ah);
        }
    }

    #[test]
    fn zoom_and_pan_align_both_overflow_and_uncovered_space() {
        let mut settings = Settings::for_pixels(100, 80);
        for (zoom, pan, expected) in [
            (100, -100, (0, 0)),
            (100, 0, (-30, 0)),
            (100, 100, (-60, 0)),
            (30, -100, (0, 0)),
            (30, 0, (26, 28)),
            (30, 100, (52, 56)),
        ] {
            settings.zoom = zoom;
            settings.pan_x = pan;
            settings.pan_y = pan;
            let p = Placement::new((40, 20), &settings).unwrap();
            assert_eq!((p.x, p.y), expected);
        }
    }

    #[test]
    fn ties_round_up_including_negative_offsets() {
        for (input, expected) in [(0.5, 1.0), (1.5, 2.0), (-0.5, 0.0), (-1.5, -1.0)] {
            assert_eq!(round(input), expected);
        }
        let settings = Settings {
            zoom: 101,
            ..Settings::for_pixels(100, 100)
        };
        let p = Placement::new((100, 100), &settings).unwrap();
        assert_eq!((p.sw, p.sh, p.x, p.y), (101, 101, 0, 0));
    }

    #[test]
    fn overflow_and_dot_cap_are_rejected() {
        let mut settings = Settings {
            card: Card::from_id("business-portrait"),
            ..Settings::default()
        };
        for source in [(0, 1), (1, 0), (0, 0)] {
            assert_eq!(settings.dimensions(source).err(), Some("empty source"));
            assert_eq!(
                Placement::new(source, &settings).unwrap_err(),
                "empty source"
            );
        }
        assert!(Placement::new((u32::MAX, 1), &settings).is_err());
        for (name, settings, source, expected) in [
            (
                "image dot cap",
                Settings::default(),
                (2001, 2000),
                "grid exceeds four million dots",
            ),
            (
                "image bleed overflow",
                Settings {
                    bleed: i32::MAX,
                    ..Settings::default()
                },
                (300, 200),
                "grid overflows",
            ),
            (
                "image margin dot cap",
                Settings {
                    dot: 8,
                    margin: 400,
                    ..Settings::default()
                },
                (1024, 1024),
                "grid exceeds four million dots",
            ),
        ] {
            assert_eq!(settings.dimensions(source).err(), Some(expected), "{name}");
            assert!(Placement::new(source, &settings).is_err(), "{name}");
        }
        settings.card = Card {
            size_mm: Some((u32::MAX, u32::MAX)),
            ..Card::from_id("business-portrait")
        };
        assert!(grid_dimensions(&settings).is_err());
        settings.card = Card::from_id("business-portrait");
        settings.bleed = i32::MAX;
        assert!(grid_dimensions(&settings).is_err());
        settings.bleed = 0;
        settings.dot = 0;
        assert!(grid_dimensions(&settings).is_err());
        for card in CARDS {
            let settings = Settings {
                card,
                dot: 8,
                bleed: 3,
                ..Settings::default()
            };
            assert!(grid_dimensions(&settings).is_ok());
        }
    }
}
