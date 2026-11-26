//! Editable settings and palette presets.

use image::{RgbaImage, imageops};
use rthid_dither::dither::{self, Mode, Options, PaletteColor, Tone};

/// Maximum number of characters in a preset or template name.
pub const MAX_NAME_CHARS: usize = 32;
/// Maximum number of colors in an editable palette.
pub const MAX_COLORS: usize = 16;

/// Validates editable hex colors and sorts the palette by brightness.
pub fn build_palette(hexes: &[impl AsRef<str>]) -> Result<Vec<PaletteColor>, String> {
    if hexes.len() > MAX_COLORS {
        return Err(format!("palette must have at most {MAX_COLORS} colors"));
    }
    if hexes.iter().any(|hex| hex.as_ref().chars().count() > 7) {
        return Err("hex colors must have at most 7 characters".into());
    }
    dither::make_palette(&hexes.iter().map(AsRef::as_ref).collect::<Vec<_>>())
}

/// Compact color model label for saved settings and palette editing.
pub fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Tone => "tone",
        Mode::Color => "color",
    }
}

/// Rejects blank names and names beyond the input limit.
pub(crate) fn validate_name(name: &str, kind: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err(format!("{kind} name must not be empty"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "{kind} name must have at most {MAX_NAME_CHARS} characters"
        ));
    }
    Ok(())
}

// TODO: Derive built-in status from the position in Palettes instead of storing a flag that
// every upsert and test literal must keep false.
/// A named palette that the Palette row can select.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    /// Label shown in the Palette row.
    pub name: String,
    /// Hex colors preserved in the user's order.
    pub colors: Vec<String>,
    /// Optional color model applied when selected.
    pub mode: Option<Mode>,
    /// Whether this preset is protected from edits and removal.
    pub builtin: bool,
}

/// Built-in palettes followed by validated user presets.
#[derive(Debug, PartialEq)]
pub struct Palettes {
    presets: Vec<Preset>,
}

impl Palettes {
    /// Original presets in selection order, with the default first.
    pub fn builtin() -> Self {
        Self {
            presets: [
                ("Black/white", vec!["#000", "#fff"], None),
                ("4-gray", vec!["#000", "#555", "#aaa", "#fff"], None),
                (
                    "Warm inks",
                    vec!["#ffffff", "#ee6a45", "#a3300f", "#1c0d07"],
                    None,
                ),
                // Hues are not evenly spaced in brightness, so Tone mode would ignore them.
                (
                    "Pratt",
                    vec!["#ffffff", "#f3c25a", "#ee6a45", "#7a4527", "#1c0d07"],
                    Some(Mode::Color),
                ),
            ]
            .into_iter()
            .map(|(name, colors, mode)| Preset {
                name: name.into(),
                colors: colors.into_iter().map(String::from).collect(),
                mode,
                builtin: true,
            })
            .collect(),
        }
    }

    /// Presets in selection order.
    pub fn iter(&self) -> impl Iterator<Item = &Preset> {
        self.presets.iter()
    }

    /// Number of selectable presets.
    pub fn len(&self) -> usize {
        self.presets.len()
    }

    /// Preset at a selection index, if it exists.
    pub fn get(&self, i: usize) -> Option<&Preset> {
        self.presets.get(i)
    }

    /// Selection index for an exact name match.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.iter().position(|preset| preset.name == name)
    }

    /// Rejects invalid names and clashes with protected presets.
    pub fn check_name(&self, name: &str) -> Result<(), String> {
        validate_name(name, "preset")?;
        if self.find(name).is_some_and(|i| self.presets[i].builtin) {
            return Err("built-in presets cannot be replaced".into());
        }
        Ok(())
    }

    /// Validates and replaces a user preset by name, or appends it.
    pub fn upsert(&mut self, mut preset: Preset) -> Result<usize, String> {
        self.check_name(&preset.name)?;
        let existing = self.find(&preset.name);
        build_palette(&preset.colors)?;
        preset.builtin = false;
        if let Some(i) = existing {
            self.presets[i] = preset;
            Ok(i)
        } else {
            self.presets.push(preset);
            Ok(self.len() - 1)
        }
    }

    /// Removes a user preset, rejecting built-ins and missing indices.
    pub fn remove(&mut self, i: usize) -> Result<(), String> {
        match self.get(i) {
            None => Err("preset does not exist".into()),
            Some(preset) if preset.builtin => Err("built-in presets cannot be removed".into()),
            Some(_) => {
                self.presets.remove(i);
                Ok(())
            }
        }
    }

    /// User presets in their saved order.
    #[cfg(test)]
    pub fn user(&self) -> impl Iterator<Item = &Preset> {
        self.iter().filter(|preset| !preset.builtin)
    }
}

/// Saved settings with palette colors independent of the preset list.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// Label shown in the template manager.
    pub name: String,
    /// Palette label retained even if the preset is removed.
    pub palette_name: String,
    /// Authoritative colors in their original order.
    pub palette_hex: Vec<String>,
    /// Color model used for diffusion.
    pub mode: Mode,
    /// Fraction of error diffused.
    pub strength: f64,
    /// Whether alternate rows run in reverse.
    pub serpentine: bool,
    /// Source size percentage.
    pub scale: i32,
    /// Adjustments before quantization.
    pub tone: Tone,
}

// TODO: Make rotation optional in templates because it depends on the image.
impl Template {
    /// Copies settings so later preset edits cannot alter this template.
    pub fn capture(name: String, settings: &Settings) -> Self {
        Self {
            name,
            palette_name: settings.palette_name.clone(),
            palette_hex: settings.palette_hex.clone(),
            mode: settings.options.mode,
            strength: settings.options.strength,
            serpentine: settings.options.serpentine,
            scale: settings.scale,
            tone: settings.options.tone,
        }
    }

    /// Restores saved settings while retaining the current image rotation.
    pub fn apply(&self, settings: &Settings) -> Result<Settings, String> {
        let palette = build_palette(&self.palette_hex)?;
        Ok(Settings {
            scale: self.scale,
            rotation: settings.rotation,
            palette_name: self.palette_name.clone(),
            palette_hex: self.palette_hex.clone(),
            options: rthid_dither::dither::Options {
                palette,
                mode: self.mode,
                strength: self.strength,
                serpentine: self.serpentine,
                tone: self.tone,
            },
        })
    }
}

// TODO: Share one named collection type with Palettes if a third list appears. They repeat
// iter, len, get, find, upsert and remove today.
/// Validated settings snapshots in saved order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Templates {
    entries: Vec<Template>,
}

impl Templates {
    /// Templates in saved order.
    pub fn iter(&self) -> impl Iterator<Item = &Template> {
        self.entries.iter()
    }

    /// Number of saved templates.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the collection contains no templates.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Template at a selection index, if it exists.
    pub fn get(&self, i: usize) -> Option<&Template> {
        self.entries.get(i)
    }

    /// Selection index for an exact name match.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.iter().position(|template| template.name == name)
    }

    /// Validates and replaces a template by name, or appends it.
    pub fn upsert(&mut self, mut template: Template) -> Result<usize, String> {
        validate_name(&template.name, "template")?;
        // TODO: Templates and presets are separate lists, so decide whether this clash rule is
        // needed at all. It also builds the built-in list on every call.
        if Palettes::builtin().find(&template.name).is_some() {
            return Err("template name must not match a built-in preset".into());
        }
        build_palette(&template.palette_hex)?;
        template.scale = Setting::Scale.clamp(f64::from(template.scale)) as i32;
        template.strength = Setting::Strength.clamp(template.strength);
        template.tone.brightness = Setting::Brightness.clamp(template.tone.brightness);
        template.tone.contrast = Setting::Contrast.clamp(template.tone.contrast);
        template.tone.gamma = Setting::Gamma.clamp(template.tone.gamma);
        template.tone.black = Setting::Black.clamp(template.tone.black);
        template.tone.white = Setting::White.clamp(template.tone.white);
        template.tone.sharpen = Setting::Sharpen.clamp(template.tone.sharpen);
        if let Some(i) = self.find(&template.name) {
            self.entries[i] = template;
            Ok(i)
        } else {
            self.entries.push(template);
            Ok(self.len() - 1)
        }
    }

    /// Removes a template if the selection index exists.
    pub fn remove(&mut self, i: usize) {
        if i < self.len() {
            self.entries.remove(i);
        }
    }
}

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
pub enum Setting {
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
    /// Tone gamma correction.
    Gamma,
    /// Tone black point.
    Black,
    /// Tone white point.
    White,
    /// Tone sharpening strength.
    Sharpen,
}

impl Setting {
    /// Parameter panel order.
    pub const ALL: [Self; 11] = [
        Self::Scale,
        Self::Mode,
        Self::Palette,
        Self::Strength,
        Self::Serpentine,
        Self::Brightness,
        Self::Contrast,
        Self::Gamma,
        Self::Black,
        Self::White,
        Self::Sharpen,
    ];

    /// Label including the current value.
    pub fn label(self, settings: &Settings, palettes: &Palettes) -> String {
        let o = &settings.options;
        match self {
            Setting::Scale => format!("Scale: {}%", settings.scale),
            Setting::Mode => format!("Mode: {:?}", o.mode),
            Setting::Palette => {
                let matches = palettes.iter().any(|preset| settings.uses(preset));
                format!(
                    "Palette: {}{}",
                    settings.palette_name,
                    if matches { "" } else { "*" }
                )
            }
            Setting::Strength => format!("Strength: {:.1}", o.strength),
            Setting::Serpentine => {
                format!("Serpentine: {}", if o.serpentine { "on" } else { "off" })
            }
            Setting::Brightness => format!("Brightness: {:+.2}", o.tone.brightness),
            Setting::Contrast => format!("Contrast: {:+.2}", o.tone.contrast),
            Setting::Gamma => format!("Gamma: {:.2}", o.tone.gamma),
            Setting::Black => format!("Black: {:.2}", o.tone.black),
            Setting::White => format!("White: {:.2}", o.tone.white),
            Setting::Sharpen => format!("Sharpen: {:.2}", o.tone.sharpen),
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
            Self::Gamma => Some((20.0, 0.4, 2.5)),
            Self::Black => Some((100.0, 0.0, 0.5)),
            Self::White => Some((100.0, 0.5, 1.0)),
            Self::Sharpen => Some((20.0, 0.0, 3.0)),
            Self::Mode | Self::Palette | Self::Serpentine => None,
        }
    }

    /// Keeps a numeric parameter within its panel bounds, mapping NaN to the minimum.
    pub fn clamp(self, value: f64) -> f64 {
        let (_, low, high) = self.range().expect("numeric parameter");
        value.max(low).min(high)
    }

    /// Whether changing this parameter switches the preview to Dithered. Scale
    /// leaves the current choice alone.
    pub fn shows_dithered(self) -> bool {
        self != Self::Scale
    }
}

/// Everything that affects the grid or the dither. d resets the selected row and
/// D resets everything, including rotation.
// TODO: Hold the palette as one Preset and derive options.palette from it. Name, hex list and
// built palette are three copies that every setter must keep in sync.
#[derive(Clone, PartialEq)]
pub struct Settings {
    /// Grid size as a percentage of the source, 10 to 200.
    pub scale: i32,
    /// Rotation applied after scaling.
    pub rotation: Rotation,
    /// Selected preset name, also retained for palettes stored in templates.
    pub palette_name: String,
    /// Hex colors preserved in the user's order.
    pub palette_hex: Vec<String>,
    /// Dither and tone settings.
    pub options: Options,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            scale: 100,
            rotation: Rotation::R0,
            palette_name: String::new(),
            palette_hex: Vec::new(),
            options: Options::default(),
        }
        .with_preset(&Palettes::builtin().presets[0])
    }
}

impl Settings {
    /// Whether the selected name and colors still match this preset.
    pub fn uses(&self, preset: &Preset) -> bool {
        self.palette_name == preset.name && self.palette_hex == preset.colors
    }

    fn set_palette(&mut self, preset: &Preset) -> Result<(), String> {
        let palette = build_palette(&preset.colors)?;
        self.palette_name.clone_from(&preset.name);
        self.palette_hex.clone_from(&preset.colors);
        self.options.palette = palette;
        if let Some(mode) = preset.mode {
            self.options.mode = mode;
        }
        Ok(())
    }

    /// Applies palette colors and an optional mode, leaving settings unchanged for invalid colors.
    pub fn with_preset(&self, preset: &Preset) -> Self {
        let mut next = self.clone();
        let _ = next.set_palette(preset);
        next
    }

    /// Returns settings with one parameter restored to its default.
    pub fn with_default(&self, setting: Setting, palettes: &Palettes) -> Self {
        let mut next = self.clone();
        let defaults = Self::default();
        match setting {
            Setting::Scale => next.scale = defaults.scale,
            Setting::Mode => next.options.mode = defaults.options.mode,
            Setting::Palette => next = next.with_preset(&palettes.presets[0]),
            Setting::Strength => next.options.strength = defaults.options.strength,
            Setting::Serpentine => next.options.serpentine = defaults.options.serpentine,
            Setting::Brightness => next.options.tone.brightness = defaults.options.tone.brightness,
            Setting::Contrast => next.options.tone.contrast = defaults.options.tone.contrast,
            Setting::Gamma => next.options.tone.gamma = defaults.options.tone.gamma,
            Setting::Black => next.options.tone.black = defaults.options.tone.black,
            Setting::White => next.options.tone.white = defaults.options.tone.white,
            Setting::Sharpen => next.options.tone.sharpen = defaults.options.tone.sharpen,
        }
        next
    }

    /// Returns settings moved `dir` steps and clamped to the range. Mode and
    /// Serpentine toggle, Palette wraps around the presets.
    pub fn adjusted(&self, setting: Setting, dir: i32, palettes: &Palettes) -> Self {
        let mut next = self.clone();
        let step = |value: f64| {
            let (steps, _, _) = setting.range().expect("numeric parameter");
            setting.clamp(((value * steps).round() + f64::from(dir)) / steps)
        };
        match setting {
            Setting::Scale => {
                let (steps, _, _) = setting.range().expect("numeric parameter");
                next.scale = setting.clamp(f64::from(self.scale) + f64::from(dir) / steps) as i32;
            }
            Setting::Mode => {
                next.options.mode = match self.options.mode {
                    Mode::Tone => Mode::Color,
                    Mode::Color => Mode::Tone,
                }
            }
            Setting::Palette => {
                let current = palettes.find(&self.palette_name).unwrap_or(0);
                let i =
                    (current as i64 + i64::from(dir)).rem_euclid(palettes.len() as i64) as usize;
                next = next.with_preset(&palettes.presets[i]);
            }
            Setting::Strength => next.options.strength = step(self.options.strength),
            Setting::Serpentine => next.options.serpentine = !self.options.serpentine,
            Setting::Brightness => {
                next.options.tone.brightness = step(self.options.tone.brightness)
            }
            Setting::Contrast => next.options.tone.contrast = step(self.options.tone.contrast),
            Setting::Gamma => next.options.tone.gamma = step(self.options.tone.gamma),
            Setting::Black => next.options.tone.black = step(self.options.tone.black),
            Setting::White => next.options.tone.white = step(self.options.tone.white),
            Setting::Sharpen => next.options.tone.sharpen = step(self.options.tone.sharpen),
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_limits_preserve_valid_entries_on_failure() {
        let mut palettes = Palettes::builtin();
        let preset = Preset {
            name: "猫".repeat(MAX_NAME_CHARS),
            colors: vec!["#123456".into(); MAX_COLORS],
            mode: None,
            builtin: false,
        };
        let index = palettes.upsert(preset.clone()).unwrap();
        let settings = Settings::default().with_preset(&preset);
        let template = Template::capture(preset.name.clone(), &settings);
        let mut templates = Templates::default();
        assert_eq!(templates.upsert(template.clone()), Ok(0));
        for name in [
            String::new(),
            " ".into(),
            "猫".repeat(MAX_NAME_CHARS + 1),
            "Pratt".into(),
        ] {
            assert!(
                palettes
                    .upsert(Preset {
                        name: name.clone(),
                        ..preset.clone()
                    })
                    .is_err()
            );
            assert!(
                templates
                    .upsert(Template {
                        name,
                        ..template.clone()
                    })
                    .is_err()
            );
        }
        for colors in [
            vec![],
            vec!["invalid".into()],
            vec!["#1234567".into()],
            vec!["#000".into(); MAX_COLORS + 1],
        ] {
            let invalid = Preset {
                colors: colors.clone(),
                ..preset.clone()
            };
            assert!(palettes.upsert(invalid.clone()).is_err());
            assert!(settings.with_preset(&invalid) == settings);
            let invalid = Template {
                palette_hex: colors,
                ..template.clone()
            };
            assert!(invalid.apply(&settings).is_err());
            assert!(templates.upsert(invalid).is_err());
        }
        assert_eq!(palettes.get(index), Some(&preset));
        assert_eq!(templates.get(0), Some(&template));
        assert_eq!(templates.find(&template.name), Some(0));
        let replacement = Template {
            strength: 0.5,
            ..template
        };
        assert_eq!(templates.upsert(replacement.clone()), Ok(0));
        assert_eq!(templates.len(), 1);
        assert_eq!(templates.get(0), Some(&replacement));
        templates.remove(0);
        assert!(templates.is_empty());
    }

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
    fn user_presets_validate_replace_and_preserve_builtins() {
        let mut palettes = Palettes::builtin();
        let builtins: Vec<_> = palettes.iter().cloned().collect();
        let mut preset = Preset {
            name: "Paper".into(),
            colors: vec!["#fff".into(), "#123".into()],
            mode: Some(Mode::Color),
            builtin: true,
        };
        for name in ["", "   ", "Black/white"] {
            assert!(
                palettes
                    .upsert(Preset {
                        name: name.into(),
                        ..preset.clone()
                    })
                    .is_err()
            );
        }
        for colors in [vec![], vec!["bad hex".into()], vec!["#000".into(); 257]] {
            assert!(
                palettes
                    .upsert(Preset {
                        colors,
                        ..preset.clone()
                    })
                    .is_err()
            );
        }
        assert_eq!(palettes.len(), builtins.len());
        let i = palettes.upsert(preset.clone()).unwrap();
        assert_eq!(i, builtins.len());
        assert_eq!(palettes.get(i).unwrap().colors, preset.colors);
        assert!(!palettes.get(i).unwrap().builtin);
        preset.colors.reverse();
        assert_eq!(palettes.upsert(preset.clone()).unwrap(), i);
        assert_eq!(palettes.len(), builtins.len() + 1);
        assert_eq!(palettes.get(i).unwrap().colors, preset.colors);
        assert_eq!(palettes.find("Paper"), Some(i));
        assert_eq!(palettes.user().count(), 1);
        let second = palettes
            .upsert(Preset {
                name: "Ink".into(),
                ..preset
            })
            .unwrap();
        assert_eq!(second, i + 1);
        for j in 0..builtins.len() {
            assert!(palettes.remove(j).is_err());
        }
        assert!(palettes.remove(palettes.len()).is_err());
        palettes.remove(i).unwrap();
        assert_eq!(palettes.find("Ink"), Some(i));
        palettes.remove(i).unwrap();
        assert_eq!(palettes.user().count(), 0);
        assert_eq!(palettes.iter().cloned().collect::<Vec<_>>(), builtins);
    }

    #[test]
    fn applying_presets_preserves_color_order_and_labels_custom_colors() {
        let mut palettes = Palettes::builtin();
        let mut preset = Preset {
            name: "Paper".into(),
            colors: vec!["#fff".into(), "#000".into()],
            mode: Some(Mode::Color),
            builtin: false,
        };
        let settings = Settings::default().with_preset(&preset);
        assert_eq!(settings.options.mode, Mode::Color);
        assert_eq!(settings.palette_hex, preset.colors);
        assert_eq!(settings.options.palette[0].rgb, [0; 3]);
        assert_eq!(
            Setting::Palette.label(&settings, &palettes),
            "Palette: Paper*"
        );
        palettes.upsert(preset.clone()).unwrap();
        assert_eq!(
            Setting::Palette.label(&settings, &palettes),
            "Palette: Paper"
        );
        preset.colors.reverse();
        assert_eq!(
            Setting::Palette.label(&settings.with_preset(&preset), &palettes),
            "Palette: Paper*"
        );
        preset.colors = vec!["invalid".into()];
        assert!(settings.clone().set_palette(&preset).is_err());
        assert!(settings.with_preset(&preset) == settings);
        let builtin = settings.with_preset(palettes.get(0).unwrap());
        assert_eq!(builtin.options.mode, Mode::Color);
        let unknown = Settings {
            palette_name: "Template".into(),
            ..settings
        };
        assert_eq!(
            unknown
                .adjusted(Setting::Palette, 1, &palettes)
                .palette_name,
            "4-gray"
        );
        assert_eq!(
            unknown
                .adjusted(Setting::Palette, -1, &palettes)
                .palette_name,
            "Paper"
        );
    }

    #[test]
    fn adjustments_use_unsnapped_steps_and_restore_each_default() {
        let palettes = Palettes::builtin();
        let settings = Settings {
            scale: 105,
            ..Settings::default()
        };
        let next = settings.adjusted(Setting::Scale, 1, &palettes);
        assert_eq!(next.scale, 115);
        assert_eq!(next.with_default(Setting::Scale, &palettes).scale, 100);
        for setting in Setting::ALL {
            let next = settings.adjusted(setting, -1, &palettes);
            let reset = next.with_default(setting, &palettes);
            assert_eq!(
                setting.label(&reset, &palettes),
                setting.label(&Settings::default(), &palettes)
            );
            assert!(setting.label(&reset, &palettes).len() <= 25);
            for dir in [i32::MIN, i32::MAX] {
                assert!(
                    setting
                        .label(&settings.adjusted(setting, dir, &palettes), &palettes)
                        .len()
                        <= 25
                );
            }
        }
    }
    #[test]
    fn templates_restore_snapshots_while_retaining_rotation() {
        let mut settings = Settings {
            scale: 60,
            rotation: Rotation::R90,
            ..Settings::default().with_preset(&Preset {
                name: "Paper".into(),
                colors: vec!["#fff".into(), "#123".into()],
                mode: Some(Mode::Color),
                builtin: false,
            })
        };
        settings.options.strength = 0.85;
        settings.options.serpentine = false;
        settings.options.tone = Tone {
            brightness: 0.1,
            contrast: 0.15,
            gamma: 1.2,
            black: 0.04,
            white: 0.95,
            sharpen: 0.6,
        };
        let mut template = Template::capture("Zine".into(), &settings);
        let current = Settings {
            rotation: Rotation::R270,
            ..Settings::default()
        };
        let restored = template.apply(&current).unwrap();
        assert!(
            restored
                == Settings {
                    rotation: current.rotation,
                    ..settings
                }
        );
        template.scale = 100_000;
        template.strength = f64::NAN;
        template.tone.gamma = 9.0;
        let mut templates = Templates::default();
        templates.upsert(template.clone()).unwrap();
        let clamped = templates.get(0).unwrap();
        assert_eq!(clamped.scale, 200);
        assert_eq!(clamped.strength, 0.0);
        assert_eq!(clamped.tone.gamma, 2.5);
        template.palette_hex = vec!["invalid".into()];
        assert!(template.apply(&current).is_err());
    }
}
