//! Source pixel exposure dabs and evenly spaced strokes independent of the viewport.

use crate::document::{ExposureMap, STOP};

const RADII: [u32; 16] = [1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64, 96, 128, 192, 256];

/// Direction of exposure adjustment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum BrushMode {
    /// Lightens the source.
    #[default]
    Dodge,
    /// Darkens the source.
    Burn,
}

impl BrushMode {
    /// Label used by the panel and status line.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Dodge => "Dodge",
            Self::Burn => "Burn",
        }
    }
}

/// Brush controls retained outside document history and saved settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Brush {
    /// Exposure direction.
    pub mode: BrushMode,
    /// Radius in source pixels.
    pub radius: u32,
    /// Maximum dab strength as a percentage of a quarter stop.
    pub strength: u8,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            mode: BrushMode::Dodge,
            radius: 16,
            strength: 20,
        }
    }
}

/// Controls in their panel order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BrushRow {
    /// Exposure direction.
    Mode,
    /// Source pixel radius.
    Radius,
    /// Percentage of a quarter stop.
    Strength,
}

impl BrushRow {
    /// Flat order shared by rendering and input.
    pub(crate) const ALL: [Self; 3] = [Self::Mode, Self::Radius, Self::Strength];

    /// Panel label with its current value.
    pub(crate) fn label(self, brush: Brush) -> String {
        match self {
            Self::Mode => format!("Mode  {}", brush.mode.name()),
            Self::Radius => format!("Radius  {} px", brush.radius),
            Self::Strength => format!("Strength  {}%", brush.strength),
        }
    }

    /// Whether this control differs from its default.
    pub(crate) fn changed(self, brush: Brush) -> bool {
        let default = Brush::default();
        match self {
            Self::Mode => brush.mode != default.mode,
            Self::Radius => brush.radius != default.radius,
            Self::Strength => brush.strength != default.strength,
        }
    }

    /// Changes one control by a step, or resets it when the direction is zero.
    pub(crate) fn adjust(self, brush: &mut Brush, direction: i32) {
        let default = Brush::default();
        match self {
            Self::Mode => {
                brush.mode = if direction == 0 {
                    default.mode
                } else if brush.mode == BrushMode::Dodge {
                    BrushMode::Burn
                } else {
                    BrushMode::Dodge
                }
            }
            Self::Radius => {
                let index = RADII
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, r)| r.abs_diff(brush.radius))
                    .map(|(index, _)| index)
                    .unwrap();
                brush.radius = if direction == 0 {
                    default.radius
                } else {
                    RADII[(index as i32 + direction).clamp(0, RADII.len() as i32 - 1) as usize]
                };
            }
            Self::Strength => {
                brush.strength = if direction == 0 {
                    default.strength
                } else {
                    (i32::from(brush.strength) + direction * 5).clamp(5, 100) as u8
                }
            }
        }
    }
}

impl Brush {
    /// Distance between dabs in source pixels.
    pub(crate) fn spacing(self) -> u32 {
        (self.radius / 4).max(1)
    }

    /// Adds a smooth dab clipped to the source, with saturation in the exposure map.
    pub(crate) fn dab(self, map: &mut ExposureMap, center: (f64, f64)) {
        let radius = f64::from(self.radius);
        let sign = if self.mode == BrushMode::Dodge {
            1.0
        } else {
            -1.0
        };
        let peak = sign * f64::from(self.strength) / 100.0 * f64::from(STOP) / 4.0;
        let left = (center.0 - radius).floor().max(0.0) as u32;
        let top = (center.1 - radius).floor().max(0.0) as u32;
        let right = ((center.0 + radius).ceil() as u32).min(map.width());
        let bottom = ((center.1 + radius).ceil() as u32).min(map.height());
        for y in top..bottom {
            for x in left..right {
                let d2 = (f64::from(x) - center.0).powi(2) + (f64::from(y) - center.1).powi(2);
                if d2 < radius * radius {
                    let weight = (1.0 - d2 / (radius * radius)).powi(2);
                    let delta = (peak * weight).round() as i16;
                    if delta != 0 {
                        map.add(x, y, delta);
                    }
                }
            }
        }
    }
}

/// Segment endpoint and distance to the next dab in an active stroke.
pub(crate) struct Stroke {
    point: Option<(f64, f64)>,
    remaining: f64,
}

impl Stroke {
    /// Starts after the initial press dab.
    pub(crate) fn new(point: (f64, f64), brush: Brush) -> Self {
        Self {
            point: Some(point),
            remaining: f64::from(brush.spacing()),
        }
    }

    /// Emits spaced dabs, lifting outside the image and restarting on entry.
    pub(crate) fn advance(
        &mut self,
        point: Option<(f64, f64)>,
        brush: Brush,
        mut dab: impl FnMut((f64, f64)),
    ) {
        let previous = self.point;
        self.point = point;
        let Some(point) = point else {
            return;
        };
        let Some(previous) = previous else {
            dab(point);
            self.remaining = f64::from(brush.spacing());
            return;
        };
        let dx = point.0 - previous.0;
        let dy = point.1 - previous.1;
        let length = dx.hypot(dy);
        let mut distance = self.remaining;
        while distance <= length {
            dab((
                previous.0 + dx * distance / length,
                previous.1 + dy * distance / length,
            ));
            distance += f64::from(brush.spacing());
        }
        self.remaining = distance - length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_after_lifting_resets_spacing() {
        let brush = Brush::default();
        let mut stroke = Stroke::new((0.0, 0.0), brush);
        let mut centers = Vec::new();
        for point in [
            Some((1.0, 0.0)),
            None,
            Some((100.0, 0.0)),
            Some((103.0, 0.0)),
        ] {
            stroke.advance(point, brush, |point| centers.push(point));
        }
        assert_eq!(centers, [(100.0, 0.0)]);
        stroke.advance(Some((104.0, 0.0)), brush, |point| centers.push(point));
        assert_eq!(centers, [(100.0, 0.0), (104.0, 0.0)]);
    }

    #[test]
    fn radius_adjustment_snaps_arbitrary_values_before_stepping() {
        for (radius, direction, expected) in [
            (0, -1, 1),
            (5, 1, 6),
            (11, -1, 8),
            (17, 1, 24),
            (u32::MAX, 1, 256),
            (u32::MAX, 0, 16),
        ] {
            let mut brush = Brush {
                radius,
                ..Brush::default()
            };
            BrushRow::Radius.adjust(&mut brush, direction);
            assert_eq!(brush.radius, expected);
        }
    }

    #[test]
    fn controls_follow_steps_and_clamp_at_limits() {
        let mut brush = Brush::default();
        for _ in 0..32 {
            BrushRow::Radius.adjust(&mut brush, -1);
        }
        assert_eq!(brush.radius, 1);
        for radius in RADII.into_iter().skip(1) {
            BrushRow::Radius.adjust(&mut brush, 1);
            assert_eq!(brush.radius, radius);
        }
        BrushRow::Radius.adjust(&mut brush, 1);
        assert_eq!(brush.radius, 256);
        for _ in 0..32 {
            BrushRow::Strength.adjust(&mut brush, -1);
        }
        assert_eq!(brush.strength, 5);
        for strength in (10..=100).step_by(5) {
            BrushRow::Strength.adjust(&mut brush, 1);
            assert_eq!(brush.strength, strength);
        }
        BrushRow::Strength.adjust(&mut brush, 1);
        assert_eq!(brush.strength, 100);
    }

    #[test]
    fn dabs_fall_off_clip_negate_and_saturate() {
        let brush = Brush {
            radius: 4,
            ..Brush::default()
        };
        let mut dodge = ExposureMap::new(12, 12);
        let mut burn = dodge.clone();
        brush.dab(&mut dodge, (4.0, 4.0));
        Brush {
            mode: BrushMode::Burn,
            ..brush
        }
        .dab(&mut burn, (4.0, 4.0));
        assert_eq!(dodge.get(4, 4), 13);
        for y in 0..12 {
            for x in 0..12 {
                assert_eq!(dodge.get(x, y), -burn.get(x, y));
                if (f64::from(x) - 4.0).hypot(f64::from(y) - 4.0) >= 4.0 {
                    assert_eq!(dodge.get(x, y), 0);
                }
            }
        }
        for center in [(0.0, 0.0), (11.0, 11.0)] {
            brush.dab(&mut dodge, center);
            assert_eq!(dodge.get(center.0 as u32, center.1 as u32), 13);
        }
        dodge.add(4, 4, i16::MAX);
        brush.dab(&mut dodge, (4.0, 4.0));
        assert_eq!(dodge.get(4, 4), i16::MAX);
        burn.add(4, 4, i16::MIN);
        Brush {
            mode: BrushMode::Burn,
            ..brush
        }
        .dab(&mut burn, (4.0, 4.0));
        assert_eq!(burn.get(4, 4), i16::MIN);
    }

    #[test]
    fn spacing_survives_short_events_and_turns() {
        for radius in [1, 3, 16, 256] {
            let brush = Brush {
                radius,
                ..Brush::default()
            };
            let spacing = f64::from(brush.spacing());
            let mut long = Stroke::new((0.0, 0.0), brush);
            let mut expected = Vec::new();
            long.advance(Some((512.0, 0.0)), brush, |p| expected.push(p));
            assert_eq!(expected.len(), (512.0 / spacing) as usize);
            for (i, p) in expected.iter().enumerate() {
                assert_eq!(*p, ((i + 1) as f64 * spacing, 0.0));
            }
            let mut short = Stroke::new((0.0, 0.0), brush);
            let mut actual = Vec::new();
            for x in 0..=512 {
                short.advance(Some((f64::from(x), 0.0)), brush, |p| actual.push(p));
            }
            assert_eq!(actual, expected);
            short.advance(Some((512.0, spacing)), brush, |p| actual.push(p));
            assert_eq!(actual.last(), Some(&(512.0, spacing)));
        }
    }
}
