//! Editable settings and palette presets.

use image::{RgbaImage, imageops};
use rthid::dither::{self, Mode, Options};

/// A named palette that the Palette row can select.
pub struct Preset {
    /// Label shown in the Palette row.
    pub name: &'static str,
    /// Hex colors in any order, sorted by `make_palette`.
    pub colors: &'static [&'static str],
}

/// Presets in the order the Palette row cycles through them. The first is the default.
pub const PALETTES: [Preset; 3] = [
    Preset {
        name: "Black/white",
        colors: &["#000", "#fff"],
    },
    Preset {
        name: "4-gray",
        colors: &["#000", "#555", "#aaa", "#fff"],
    },
    Preset {
        name: "Warm inks",
        colors: &["#ffffff", "#ee6a45", "#a3300f", "#1c0d07"],
    },
];

/// Clockwise rotation applied to the grid after scaling. Not a panel row, the r
/// and R keys change it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rotation {
    /// Original orientation.
    #[default]
    R0,
    /// One clockwise quarter turn.
    R90,
    /// Half turn.
    R180,
    /// Three clockwise quarter turns.
    R270,
}

impl Rotation {
    /// Turns clockwise by ninety degrees.
    pub fn cw(self) -> Self {
        match self {
            Self::R0 => Self::R90,
            Self::R90 => Self::R180,
            Self::R180 => Self::R270,
            Self::R270 => Self::R0,
        }
    }

    /// Turns counterclockwise by ninety degrees.
    pub fn ccw(self) -> Self {
        match self {
            Self::R0 => Self::R270,
            Self::R90 => Self::R0,
            Self::R180 => Self::R90,
            Self::R270 => Self::R180,
        }
    }

    /// Whether rotation exchanges width and height (quarter and three quarter turns).
    pub fn swaps_axes(self) -> bool {
        matches!(self, Self::R90 | Self::R270)
    }

    /// Clockwise angle for display.
    pub fn degrees(self) -> u16 {
        match self {
            Self::R0 => 0,
            Self::R90 => 90,
            Self::R180 => 180,
            Self::R270 => 270,
        }
    }

    /// Rotates an upright image into this orientation.
    pub fn apply(self, image: RgbaImage) -> RgbaImage {
        match self {
            Self::R0 => image,
            Self::R90 => imageops::rotate90(&image),
            Self::R180 => imageops::rotate180(&image),
            Self::R270 => imageops::rotate270(&image),
        }
    }
}

/// Editable rows in the parameter panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Param {
    /// Grid size as a percentage of the source.
    Scale,
    /// Dither color model.
    Mode,
    /// Palette preset.
    Palette,
    /// Fraction of dither error diffused.
    Strength,
    /// Whether odd rows dither right to left.
    Serpentine,
    /// Tone brightness offset.
    Brightness,
    /// Tone contrast adjustment.
    Contrast,
}

impl Param {
    /// Parameter panel order.
    pub const ALL: [Self; 7] = [
        Self::Scale,
        Self::Mode,
        Self::Palette,
        Self::Strength,
        Self::Serpentine,
        Self::Brightness,
        Self::Contrast,
    ];

    /// Label including the current value.
    pub fn label(self, params: &Params) -> String {
        let o = &params.options;
        match self {
            Param::Scale => format!("Scale: {}%", params.scale),
            Param::Mode => format!("Mode: {:?}", o.mode),
            Param::Palette => format!("Palette: {}", params.palette_name()),
            Param::Strength => format!("Strength: {:.1}", o.strength),
            Param::Serpentine => format!("Serpentine: {}", if o.serpentine { "on" } else { "off" }),
            Param::Brightness => format!("Brightness: {:+.2}", o.tone.brightness),
            Param::Contrast => format!("Contrast: {:+.2}", o.tone.contrast),
        }
    }

    /// `(steps per unit, min, max)` for numeric parameters, with inclusive bounds.
    /// Values snap to multiples of `1 / steps`, except Scale, which moves 10
    /// percentage points per step.
    pub fn range(self) -> Option<(f64, f64, f64)> {
        match self {
            Self::Scale => Some((0.1, 10.0, 200.0)),
            Self::Strength => Some((10.0, 0.0, 1.0)),
            Self::Brightness => Some((20.0, -0.5, 0.5)),
            Self::Contrast => Some((20.0, -0.5, 1.0)),
            Self::Mode | Self::Palette | Self::Serpentine => None,
        }
    }

    /// Whether changing this parameter switches the preview to Dithered. Scale
    /// leaves the current choice alone.
    pub fn shows_dithered(self) -> bool {
        self != Self::Scale
    }
}

/// Everything that affects the grid or the dither. d resets the selected row and
/// D resets everything, including rotation and tone values without a row.
#[derive(Clone, PartialEq)]
pub struct Params {
    /// Grid size as a percentage of the source, 10 to 200.
    pub scale: i32,
    /// Rotation applied after scaling.
    pub rotation: Rotation,
    /// Index of the selected palette preset.
    pub(crate) palette: usize,
    /// Dither and tone settings.
    pub options: Options,
}

impl Default for Params {
    fn default() -> Self {
        let mut params = Self {
            scale: 100,
            rotation: Rotation::R0,
            palette: 0,
            options: Options::default(),
        };
        params.set_palette(0);
        params
    }
}

impl Params {
    /// Name of the selected palette preset.
    pub fn palette_name(&self) -> &'static str {
        PALETTES[self.palette].name
    }

    fn set_palette(&mut self, i: usize) {
        self.palette = i;
        self.options.palette = dither::make_palette(PALETTES[i].colors).expect("valid preset");
    }

    /// Returns settings with one parameter restored to its default.
    pub fn with_default(&self, param: Param) -> Self {
        let mut next = self.clone();
        let defaults = Self::default();
        match param {
            Param::Scale => next.scale = defaults.scale,
            Param::Mode => next.options.mode = defaults.options.mode,
            Param::Palette => next.set_palette(defaults.palette),
            Param::Strength => next.options.strength = defaults.options.strength,
            Param::Serpentine => next.options.serpentine = defaults.options.serpentine,
            Param::Brightness => next.options.tone.brightness = defaults.options.tone.brightness,
            Param::Contrast => next.options.tone.contrast = defaults.options.tone.contrast,
        }
        next
    }

    /// Returns settings moved `dir` steps and clamped to the range. Mode and
    /// Serpentine toggle, Palette wraps around the presets.
    pub fn adjusted(&self, param: Param, dir: i32) -> Self {
        let mut next = self.clone();
        let step = |value: f64| {
            let (steps, low, high) = param.range().expect("numeric parameter");
            (((value * steps).round() + f64::from(dir)) / steps).clamp(low, high)
        };
        match param {
            Param::Scale => {
                let (steps, low, high) = param.range().expect("numeric parameter");
                next.scale =
                    (f64::from(self.scale) + f64::from(dir) / steps).clamp(low, high) as i32;
            }
            Param::Mode => {
                next.options.mode = match self.options.mode {
                    Mode::Tone => Mode::Color,
                    Mode::Color => Mode::Tone,
                }
            }
            Param::Palette => next.set_palette(
                (self.palette as i32 + dir).rem_euclid(PALETTES.len() as i32) as usize,
            ),
            Param::Strength => next.options.strength = step(self.options.strength),
            Param::Serpentine => next.options.serpentine = !self.options.serpentine,
            Param::Brightness => next.options.tone.brightness = step(self.options.tone.brightness),
            Param::Contrast => next.options.tone.contrast = step(self.options.tone.contrast),
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_cycles_and_reports_axes() {
        for (rotation, degrees, swaps_axes) in [
            (Rotation::R0, 0, false),
            (Rotation::R90, 90, true),
            (Rotation::R180, 180, false),
            (Rotation::R270, 270, true),
        ] {
            assert_eq!(rotation.cw().ccw(), rotation);
            assert_eq!(rotation.ccw().cw(), rotation);
            assert_eq!(rotation.cw().cw().cw().cw(), rotation);
            assert_eq!(rotation.degrees(), degrees);
            assert_eq!(rotation.swaps_axes(), swaps_axes);
        }
    }

    #[test]
    fn adjustments_use_unsnapped_steps_and_restore_each_default() {
        let params = Params {
            scale: 105,
            ..Params::default()
        };
        let next = params.adjusted(Param::Scale, 1);
        assert_eq!(next.scale, 115);
        assert_eq!(next.with_default(Param::Scale).scale, 100);
        for param in Param::ALL {
            let next = params.adjusted(param, -1);
            let reset = next.with_default(param);
            assert_eq!(param.label(&reset), param.label(&Params::default()));
        }
    }
}
