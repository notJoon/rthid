//! Physical PNG sizing and integer placement inside a physical card.

use crate::settings::{Rotation, Settings};

/// Rounds each card axis including bleed to pixels, independently of the dot grid.
/// Returns `InvalidInput` when a dimension is zero or overflows `u32`.
pub(crate) fn png_size(settings: &Settings, dpi: u32) -> std::io::Result<(u32, u32)> {
    let axis = |mm: u32| {
        let mm = f64::from(mm) + 2.0 * f64::from(settings.bleed);
        let pixels = (mm / 25.4 * f64::from(dpi)).round();
        if !(1.0..=f64::from(u32::MAX)).contains(&pixels) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "PNG dimension must be positive and fit in u32",
            ));
        }
        Ok(pixels as u32)
    };
    Ok((axis(settings.card.w_mm)?, axis(settings.card.h_mm)?))
}

/// Placement also serves as the complete geometry cache key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Placement {
    /// Fixed dot grid width.
    pub width: u32,
    /// Fixed dot grid height.
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

// Ties go toward positive infinity, including negative image offsets.
fn round(value: f64) -> f64 {
    (value + 0.5).floor()
}

impl Placement {
    /// Rejects impossible source sizes and placements before allocating pixels.
    pub fn new(source: (u32, u32), settings: &Settings) -> Result<Self, &'static str> {
        let (width, height) = settings.grid_dimensions()?;
        if source.0 == 0 || source.1 == 0 {
            return Err("empty source");
        }
        let inset = if settings.margin > 0 {
            round(
                (i64::from(settings.bleed) * 100 + i64::from(settings.margin) * 10) as f64
                    / f64::from(settings.dot),
            ) as u32
        } else {
            0
        };
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
        if aw <= 2 || ah <= 2 {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{CARDS, Card};

    #[test]
    fn png_presets_round_each_axis_at_print_resolutions() {
        for (card, expected) in CARDS.into_iter().zip([
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
                assert_eq!(png_size(&settings, dpi).unwrap(), expected);
                settings.dot = 8;
                settings.rotation = Rotation::R90;
                assert_eq!(png_size(&settings, dpi).unwrap(), expected);
            }
        }
        let mut settings = Settings {
            bleed: 3,
            ..Settings::default()
        };
        assert_eq!(png_size(&settings, 600).unwrap(), (1323, 2268));
        settings.card.w_mm = 127;
        settings.bleed = 0;
        assert_eq!(png_size(&settings, 1).unwrap().0, 5);
        assert_eq!(png_size(&settings, 3).unwrap().0, 15);
    }

    #[test]
    fn png_size_rejects_zero_and_overflow_before_casting() {
        let mut settings = Settings::default();
        assert!(png_size(&settings, 0).is_err());
        settings.card.w_mm = 0;
        assert!(png_size(&settings, 600).is_err());
        settings.card.w_mm = u32::MAX;
        settings.bleed = i32::MAX;
        assert!(png_size(&settings, 4800).is_err());
        settings.card = CARDS[0];
        settings.bleed = i32::MIN;
        assert!(png_size(&settings, 600).is_err());
    }

    #[test]
    fn presets_and_axes_round_independently() {
        for (card, expected) in CARDS.into_iter().zip([
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
            assert_eq!(settings.grid_dimensions(), Ok(expected));
        }
        let settings = Settings {
            card: CARDS[2],
            dot: 8,
            ..Settings::default()
        };
        assert_eq!(settings.grid_dimensions(), Ok((688, 1063)));
        let settings = Settings::for_pixels(1, 2);
        assert_eq!(settings.grid_dimensions(), Ok((8, 8)));
    }

    #[test]
    fn margin_zero_covers_bleed_and_positive_margin_insets_it() {
        let mut settings = Settings {
            bleed: 3,
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
        let mut settings = Settings::default();
        assert!(Placement::new((0, 1), &settings).is_err());
        assert!(Placement::new((u32::MAX, 1), &settings).is_err());
        settings.card = Card {
            w_mm: u32::MAX,
            h_mm: u32::MAX,
            ..CARDS[0]
        };
        assert!(settings.grid_dimensions().is_err());
        settings.card = CARDS[0];
        settings.bleed = i32::MAX;
        assert!(settings.grid_dimensions().is_err());
        settings.bleed = 0;
        settings.dot = 0;
        assert!(settings.grid_dimensions().is_err());
        for card in CARDS {
            let settings = Settings {
                card,
                dot: 8,
                bleed: 3,
                ..Settings::default()
            };
            assert!(settings.grid_dimensions().is_ok());
        }
    }
}
