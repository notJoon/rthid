//! Editable settings and palette presets.

use image::{RgbaImage, imageops};
use rthid_dither::dither::{
    self, Algorithm, BayerSize, Diffusion, Kernel, Mode, Options, PaletteColor, Tone,
};

/// Maximum dot count accepted before image allocation.
pub const MAX_DOTS: u64 = 4_000_000;

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
        .map_err(|error| error.to_string())
}

/// A named palette with source colors and their validated dither colors.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    name: String,
    hex: Vec<String>,
    colors: Vec<PaletteColor>,
}

impl Palette {
    /// Validates colors and keeps their original and dither-ready forms together.
    pub fn new(name: String, hex: Vec<String>) -> Result<Self, String> {
        let colors = build_palette(&hex)?;
        Ok(Self { name, hex, colors })
    }

    /// Palette label retained for saved templates and preset selection.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Hex colors in their original order.
    pub fn hex(&self) -> &[String] {
        &self.hex
    }

    /// Colors sorted by brightness for quantization.
    pub fn colors(&self) -> &[PaletteColor] {
        &self.colors
    }

    /// Builds unnamed colors for tests that do not need a preset label.
    #[cfg(test)]
    pub(crate) fn custom(hex: &[&str]) -> Self {
        Self::new(
            String::new(),
            hex.iter().map(|color| (*color).into()).collect(),
        )
        .unwrap()
    }
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

/// Dither algorithm choices in panel order.
///
/// The app keeps its own enum for the nine choices and their short labels so the
/// dither library stays free of UI and serde concerns. Five choices diffuse error
/// and four use thresholds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dither {
    /// Diffuses error to six nearby pixels.
    #[default]
    Atkinson,
    /// Diffuses error to four nearby pixels.
    FloydSteinberg,
    /// Spreads error across two following rows.
    Jarvis,
    /// Spreads error with Stucki weights.
    Stucki,
    /// Diffuses error to three nearby pixels.
    SierraLite,
    /// Uses a two by two threshold matrix.
    Bayer2,
    /// Uses a four by four threshold matrix.
    Bayer4,
    /// Uses an eight by eight threshold matrix.
    Bayer8,
    /// Uses thresholds from a repeatable seed.
    Noise,
}

impl Dither {
    /// Fixed cycle order for the panel.
    pub const ALL: [Self; 9] = [
        Self::Atkinson,
        Self::FloydSteinberg,
        Self::Jarvis,
        Self::Stucki,
        Self::SierraLite,
        Self::Bayer2,
        Self::Bayer4,
        Self::Bayer8,
        Self::Noise,
    ];

    /// Short label for panels and templates.
    pub fn label(self) -> &'static str {
        match self {
            Self::Atkinson => "Atkinson",
            Self::FloydSteinberg => "Floyd-Steinberg",
            Self::Jarvis => "Jarvis",
            Self::Stucki => "Stucki",
            Self::SierraLite => "Sierra Lite",
            Self::Bayer2 => "Bayer 2",
            Self::Bayer4 => "Bayer 4",
            Self::Bayer8 => "Bayer 8",
            Self::Noise => "Noise",
        }
    }

    /// Diffusion kernel used by this choice, if any. This is the only mapping from
    /// app choices to library kernels, and `None` marks a threshold algorithm.
    pub fn kernel(self) -> Option<Kernel> {
        match self {
            Self::Atkinson => Some(Kernel::Atkinson),
            Self::FloydSteinberg => Some(Kernel::FloydSteinberg),
            Self::Jarvis => Some(Kernel::JarvisJudiceNinke),
            Self::Stucki => Some(Kernel::Stucki),
            Self::SierraLite => Some(Kernel::SierraLite),
            Self::Bayer2 | Self::Bayer4 | Self::Bayer8 | Self::Noise => None,
        }
    }

    /// Uses only the parameters relevant to the selected algorithm.
    ///
    /// Diffusion choices take strength and serpentine, Noise takes the seed and
    /// Bayer takes neither. The other values are dropped here, so the engine never
    /// sees settings that do not apply. The app seed widens to the library u64.
    pub fn algorithm(self, strength: f64, serpentine: bool, seed: u32) -> Algorithm {
        if let Some(kernel) = self.kernel() {
            return Algorithm::Diffusion(Diffusion {
                kernel,
                strength,
                serpentine,
            });
        }
        match self {
            Self::Bayer2 => Algorithm::Bayer(BayerSize::Two),
            Self::Bayer4 => Algorithm::Bayer(BayerSize::Four),
            Self::Bayer8 => Algorithm::Bayer(BayerSize::Eight),
            Self::Noise => Algorithm::Noise {
                seed: u64::from(seed),
            },
            _ => unreachable!("diffusion kernels handled above"),
        }
    }
}

/// Fixed physical dimensions or a card that follows the source image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Card {
    /// Stable template identifier.
    pub id: &'static str,
    /// Panel label.
    pub label: &'static str,
    /// Fixed size in whole millimetres, or None for Image.
    pub size_mm: Option<(u32, u32)>,
}

/// Resolved card dimensions including bleed.
pub struct Dimensions {
    /// Dot grid width and height.
    pub grid: (u32, u32),
    /// Physical width and height in millimetres.
    pub mm: (f64, f64),
    /// Physical card size used by the status line.
    pub card_mm: (f64, f64),
    /// Paper border on each side in dots.
    pub inset: u32,
}

/// Card choices in panel order, with the default first.
pub const CARDS: [Card; 7] = [
    Card {
        id: "image",
        label: "Image",
        size_mm: None,
    },
    Card {
        id: "business-portrait",
        label: "Business 50x90",
        size_mm: Some((50, 90)),
    },
    Card {
        id: "business-landscape",
        label: "Business 90x50",
        size_mm: Some((90, 50)),
    },
    Card {
        id: "euro-business",
        label: "Euro business 55x85",
        size_mm: Some((55, 85)),
    },
    Card {
        id: "trading",
        label: "Trading 63x88",
        size_mm: Some((63, 88)),
    },
    Card {
        id: "square",
        label: "Square 65x65",
        size_mm: Some((65, 65)),
    },
    Card {
        id: "postcard",
        label: "Postcard 100x148",
        size_mm: Some((100, 148)),
    },
];

/// Business portrait card shared by tests.
#[cfg(test)]
pub(crate) const BUSINESS_CARD: Card = CARDS[1];

/// Ties go toward positive infinity, including negative image offsets.
pub(crate) fn round(value: f64) -> f64 {
    (value + 0.5).floor()
}

impl Card {
    /// Unknown template identifiers select the default card.
    pub fn from_id(id: &str) -> Self {
        CARDS
            .iter()
            .find(|card| card.id == id)
            .copied()
            .unwrap_or(CARDS[0])
    }
}

/// Saved settings with palette colors independent of the preset list.
///
/// The algorithm, seed and remembered diffusion values are all saved, even when
/// the saved algorithm does not use some of them. Palette colors are copied rather
/// than referenced by name, so later preset edits leave the snapshot intact.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    /// Label shown in the template manager.
    pub name: String,
    /// Palette label retained even if the preset is removed.
    pub palette_name: String,
    /// Authoritative colors in their original order.
    pub palette_hex: Vec<String>,
    /// Color model used for quantization.
    pub mode: Mode,
    /// Selected quantization algorithm.
    pub dither: Dither,
    /// Seed retained for noise patterns, saved even when another algorithm is selected.
    pub seed: u32,
    /// Fraction of error diffused, saved even when a threshold algorithm is selected.
    pub strength: f64,
    /// Whether alternate rows run in reverse, saved like strength.
    pub serpentine: bool,
    /// Card preset, stored by id in configuration files.
    pub card: Card,
    /// Dot size in hundredths of a millimetre.
    pub dot: i32,
    /// Margin in tenths of a millimetre.
    pub margin: i32,
    /// Bleed in whole millimetres.
    pub bleed: i32,
    /// Adjustments before quantization.
    pub tone: Tone,
}

impl Template {
    /// Copies settings so later preset edits cannot alter this template.
    pub fn capture(name: String, settings: &Settings) -> Self {
        Self {
            name,
            palette_name: settings.palette.name().into(),
            palette_hex: settings.palette.hex().to_vec(),
            mode: settings.mode,
            dither: settings.dither,
            seed: settings.seed,
            strength: settings.strength,
            serpentine: settings.serpentine,
            card: settings.card,
            dot: settings.dot,
            margin: settings.margin,
            bleed: settings.bleed,
            tone: settings.tone,
        }
    }

    /// Restores saved settings while retaining the current rotation, flip, zoom and pan.
    pub fn apply(&self, settings: &Settings) -> Result<Settings, String> {
        Ok(Settings {
            dither: self.dither,
            strength: self.strength,
            serpentine: self.serpentine,
            seed: self.seed,
            card: self.card,
            dot: self.dot,
            margin: self.margin,
            bleed: self.bleed,
            palette: Palette::new(self.palette_name.clone(), self.palette_hex.clone())?,
            mode: self.mode,
            tone: self.tone,
            ..settings.clone()
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
        for (setting, value) in [
            (Setting::Dot, &mut template.dot),
            (Setting::Margin, &mut template.margin),
            (Setting::Bleed, &mut template.bleed),
        ] {
            *value = setting.snap(f64::from(*value)).round() as i32;
        }
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

/// Clockwise source rotation inside the fixed card, changed by r and R.
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
    /// Physical card preset.
    Card,
    /// Physical size of one dot.
    Dot,
    /// Paper border inside the card.
    Margin,
    /// Extra print area on every side.
    Bleed,
    /// Image magnification relative to cover.
    Zoom,
    /// Horizontal image alignment.
    PanX,
    /// Vertical image alignment.
    PanY,
    /// Dither color model.
    Mode,
    /// Palette preset.
    Palette,
    /// Quantization algorithm.
    Dither,
    /// Fraction of dither error diffused.
    Strength,
    /// Whether odd rows dither right to left.
    Serpentine,
    /// Noise pattern seed.
    Seed,
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

/// Parameter groups in panel order, with their item counts.
pub const GROUPS: [(&str, usize); 3] = [("Print", 7), ("Dither", 6), ("Tone", 6)];

/// Item counts of the three settings groups.
pub const GROUP_LENS: [usize; 3] = [GROUPS[0].1, GROUPS[1].1, GROUPS[2].1];

const _: () = assert!(GROUP_LENS[0] + GROUP_LENS[1] + GROUP_LENS[2] == Setting::ALL.len());

impl Setting {
    /// Parameter panel order.
    pub const ALL: [Self; 19] = [
        Self::Card,
        Self::Dot,
        Self::Margin,
        Self::Bleed,
        Self::Zoom,
        Self::PanX,
        Self::PanY,
        Self::Mode,
        Self::Palette,
        Self::Dither,
        Self::Strength,
        Self::Serpentine,
        Self::Seed,
        Self::Brightness,
        Self::Contrast,
        Self::Gamma,
        Self::Black,
        Self::White,
        Self::Sharpen,
    ];

    /// Label including the current value.
    pub fn label(self, settings: &Settings, palettes: &Palettes) -> String {
        let o = settings;
        match self {
            Setting::Card => format!("Card: {}", o.card.label),
            Setting::Dot => format!("Dot: {:.2} mm", f64::from(o.dot) / 100.0),
            Setting::Margin => format!("Margin: {:.1} mm", f64::from(o.margin) / 10.0),
            Setting::Bleed => format!("Bleed: {} mm", o.bleed),
            Setting::Zoom => format!("Zoom: {:.2}x", f64::from(o.zoom) / 100.0),
            Setting::PanX => format!("Pan X: {:+.2}", f64::from(o.pan_x) / 100.0),
            Setting::PanY => format!("Pan Y: {:+.2}", f64::from(o.pan_y) / 100.0),
            Setting::Mode => format!("Mode: {:?}", o.mode),
            Setting::Palette => {
                let matches = palettes.iter().any(|preset| settings.uses(preset));
                format!(
                    "Palette: {}{}",
                    settings.palette.name(),
                    if matches { "" } else { "*" }
                )
            }
            Setting::Dither => format!("Dither: {}", settings.dither.label()),
            Setting::Seed => format!("Seed: {}", settings.seed),
            Setting::Strength => format!("Strength: {:.1}", settings.strength),
            Setting::Serpentine => {
                format!(
                    "Serpentine: {}",
                    if settings.serpentine { "on" } else { "off" }
                )
            }
            Setting::Brightness => format!("Brightness: {:+.2}", o.tone.brightness),
            Setting::Contrast => format!("Contrast: {:+.2}", o.tone.contrast),
            Setting::Gamma => format!("Gamma: {:.2}", o.tone.gamma),
            Setting::Black => format!("Black: {:.2}", o.tone.black),
            Setting::White => format!("White: {:.2}", o.tone.white),
            Setting::Sharpen => format!("Sharpen: {:.2}", o.tone.sharpen),
        }
    }

    /// Whether the current algorithm uses this parameter. Strength and Serpentine
    /// apply to diffusion and Seed applies to Noise.
    ///
    /// Inactive rows stay visible in place, shown dimmed. Adjusting, resetting and
    /// reseeding leave them unchanged, so they keep their value for a later switch
    /// back and never switch the preview.
    pub fn is_active(self, settings: &Settings) -> bool {
        match self {
            Self::Strength | Self::Serpentine => settings.dither.kernel().is_some(),
            Self::Seed => settings.dither == Dither::Noise,
            _ => true,
        }
    }

    /// Mutable numeric value for rows with float steps. It ignores whether the row
    /// is active, so callers check [`Setting::is_active`] first.
    pub fn value_mut(self, settings: &mut Settings) -> Option<&mut f64> {
        match self {
            Self::Strength => Some(&mut settings.strength),
            Self::Brightness => Some(&mut settings.tone.brightness),
            Self::Contrast => Some(&mut settings.tone.contrast),
            Self::Gamma => Some(&mut settings.tone.gamma),
            Self::Black => Some(&mut settings.tone.black),
            Self::White => Some(&mut settings.tone.white),
            Self::Sharpen => Some(&mut settings.tone.sharpen),
            _ => None,
        }
    }

    fn integer_mut(self, settings: &mut Settings) -> Option<&mut i32> {
        match self {
            Self::Dot => Some(&mut settings.dot),
            Self::Margin => Some(&mut settings.margin),
            Self::Bleed => Some(&mut settings.bleed),
            Self::Zoom => Some(&mut settings.zoom),
            Self::PanX => Some(&mut settings.pan_x),
            Self::PanY => Some(&mut settings.pan_y),
            _ => None,
        }
    }

    /// `(steps per unit, min, max)` for numeric parameters, with inclusive bounds.
    /// Print controls use their stored integer units.
    pub fn range(self) -> Option<(f64, f64, f64)> {
        match self {
            Self::Dot => Some((1.0, 8.0, 60.0)),
            Self::Margin => Some((0.2, 0.0, 100.0)),
            Self::Bleed => Some((1.0, 0.0, 3.0)),
            Self::Zoom => Some((0.2, 30.0, 400.0)),
            Self::PanX | Self::PanY => Some((0.2, -100.0, 100.0)),
            Self::Strength => Some((10.0, 0.0, 1.0)),
            Self::Brightness => Some((20.0, -0.5, 0.5)),
            Self::Contrast => Some((20.0, -0.5, 1.0)),
            Self::Gamma => Some((20.0, 0.4, 2.5)),
            Self::Black => Some((100.0, 0.0, 0.5)),
            Self::White => Some((100.0, 0.5, 1.0)),
            Self::Sharpen => Some((20.0, 0.0, 3.0)),
            Self::Card
            | Self::Mode
            | Self::Palette
            | Self::Dither
            | Self::Serpentine
            | Self::Seed => None,
        }
    }

    /// Keeps a numeric parameter within its panel bounds, mapping NaN to the minimum.
    pub fn clamp(self, value: f64) -> f64 {
        let (_, low, high) = self.range().expect("numeric parameter");
        value.max(low).min(high)
    }

    /// Rounds to the nearest panel step, then clamps, so loaded values match keyboard steps.
    pub fn snap(self, value: f64) -> f64 {
        let (steps, _, _) = self.range().expect("numeric parameter");
        self.clamp((value * steps).round() / steps)
    }

    /// Geometry controls retain the selected view.
    pub fn shows_dithered(self) -> bool {
        !matches!(
            self,
            Self::Card
                | Self::Dot
                | Self::Margin
                | Self::Bleed
                | Self::Zoom
                | Self::PanX
                | Self::PanY
        )
    }
}

/// Everything that affects the grid or the dither. d resets the selected row.
#[derive(Clone, PartialEq)]
pub struct Settings {
    /// Physical card before bleed.
    pub card: Card,
    /// Dot size in hundredths of a millimetre.
    pub dot: i32,
    /// Margin in tenths of a millimetre.
    pub margin: i32,
    /// Bleed in whole millimetres.
    pub bleed: i32,
    /// Cover magnification in hundredths.
    pub zoom: i32,
    /// Horizontal alignment in hundredths.
    pub pan_x: i32,
    /// Vertical alignment in hundredths.
    pub pan_y: i32,
    /// Source rotation inside the card.
    pub rotation: Rotation,
    /// Mirrors left and right on the card after rotation.
    pub flip: bool,
    /// Selected palette label, source colors and validated dither colors.
    pub palette: Palette,
    /// Selected quantization algorithm.
    ///
    /// Strength, serpentine and seed are kept as plain values next to it and are
    /// remembered across algorithm switches. Only the values that apply to this
    /// choice reach the engine, through [`Settings::options`].
    pub dither: Dither,
    /// Fraction of error diffused, used by diffusion algorithms only.
    pub strength: f64,
    /// Whether alternate rows run in reverse, used by diffusion algorithms only.
    pub serpentine: bool,
    /// Seed retained for noise patterns. The panel edits a u32 range while the
    /// library accepts any u64.
    pub seed: u32,
    /// Color model used for quantization.
    pub mode: Mode,
    /// Adjustments before quantization.
    pub tone: Tone,
}

impl Default for Settings {
    fn default() -> Self {
        let preset = &Palettes::builtin().presets[0];
        Self {
            card: CARDS[0],
            dot: 20,
            margin: 0,
            bleed: 0,
            zoom: 100,
            pan_x: 0,
            pan_y: 0,
            rotation: Rotation::R0,
            flip: false,
            palette: Palette::new(preset.name.clone(), preset.colors.clone())
                .expect("built-in palette"),
            dither: Dither::default(),
            strength: 1.0,
            serpentine: true,
            seed: 0,
            mode: preset.mode.unwrap_or_default(),
            tone: Tone::default(),
        }
    }
}

impl Settings {
    /// Uses one millimetre per dot to preserve source pixels in grid tests.
    #[cfg(test)]
    pub fn for_pixels(w: u32, h: u32) -> Self {
        Self {
            card: Card {
                id: "test",
                label: "Test",
                size_mm: Some((w, h)),
            },
            dot: 100,
            ..Self::default()
        }
    }

    /// Resolves grid, paper inset and physical size including bleed from the source.
    pub fn dimensions(&self, source: (u32, u32)) -> Result<Dimensions, &'static str> {
        if self.dot <= 0 || self.bleed < 0 {
            return Err("invalid print dimensions");
        }
        if source.0 == 0 || source.1 == 0 {
            return Err("empty source");
        }
        let dot = f64::from(self.dot);
        let inset = if self.margin > 0 || self.card.size_mm.is_none() {
            round((i64::from(self.bleed) * 100 + i64::from(self.margin.max(0)) * 10) as f64 / dot)
                as u32
        } else {
            0
        };
        let (grid, mm) = if let Some((w, h)) = self.card.size_mm {
            let axis = |mm: u32| -> Option<u32> {
                let units = u64::from(mm)
                    .checked_add(u64::try_from(self.bleed).ok()?.checked_mul(2)?)?
                    .checked_mul(100)?;
                let dot = u64::try_from(self.dot).ok()?;
                u32::try_from(((units.checked_mul(2)?.checked_add(dot)?) / (dot * 2)).max(8)).ok()
            };
            (
                (
                    axis(w).ok_or("grid overflows")?,
                    axis(h).ok_or("grid overflows")?,
                ),
                (
                    f64::from(w) + 2.0 * f64::from(self.bleed),
                    f64::from(h) + 2.0 * f64::from(self.bleed),
                ),
            )
        } else {
            let (w, h) = if self.rotation.swaps_axes() {
                (source.1, source.0)
            } else {
                source
            };
            let border = inset.checked_mul(2).ok_or("grid overflows")?;
            let grid = (
                w.checked_add(border).ok_or("grid overflows")?,
                h.checked_add(border).ok_or("grid overflows")?,
            );
            (
                grid,
                (
                    f64::from(grid.0) * dot / 100.0,
                    f64::from(grid.1) * dot / 100.0,
                ),
            )
        };
        if u64::from(grid.0) * u64::from(grid.1) > MAX_DOTS {
            return Err("grid exceeds four million dots");
        }
        let card_mm = self
            .card
            .size_mm
            .map_or(mm, |(w, h)| (f64::from(w), f64::from(h)));
        Ok(Dimensions {
            grid,
            mm,
            card_mm,
            inset,
        })
    }

    /// Builds library options from the current panel values.
    ///
    /// Options are derived on demand rather than stored, so there is no second
    /// copy of the algorithm that could drift from the panel. Inactive values are
    /// left out, which also keeps them out of the dither cache key.
    pub fn options(&self) -> Options {
        Options {
            palette: self.palette.colors().to_vec(),
            mode: self.mode,
            tone: self.tone,
            algorithm: self
                .dither
                .algorithm(self.strength, self.serpentine, self.seed),
        }
    }

    /// Whether Enter can draw a new pattern for the selected setting.
    pub fn can_reseed(&self, setting: Setting) -> bool {
        setting == Setting::Seed && setting.is_active(self)
    }

    /// Changes an active Seed row, ensuring the pattern seed changes.
    ///
    /// The app supplies the entropy and this decides the seed, so the engine stays
    /// deterministic and the rule is testable. A candidate equal to the current
    /// seed moves up by one, wrapping at u32::MAX. Other rows are left unchanged.
    pub fn reseeded(&self, setting: Setting, entropy: u64) -> Self {
        let mut next = self.clone();
        if self.can_reseed(setting) {
            next.seed = entropy as u32;
            if next.seed == self.seed {
                next.seed = next.seed.wrapping_add(1);
            }
        }
        next
    }

    /// Whether the selected name and colors still match this preset.
    pub fn uses(&self, preset: &Preset) -> bool {
        self.palette.name() == preset.name && self.palette.hex() == preset.colors
    }

    fn set_palette(&mut self, preset: &Preset) -> Result<(), String> {
        self.palette = Palette::new(preset.name.clone(), preset.colors.clone())?;
        if let Some(mode) = preset.mode {
            self.mode = mode;
        }
        Ok(())
    }

    /// Applies palette colors and an optional mode, leaving settings unchanged for invalid colors.
    pub fn with_preset(&self, preset: &Preset) -> Self {
        let mut next = self.clone();
        let _ = next.set_palette(preset);
        next
    }

    /// Restores source composition without changing print or dither settings.
    pub fn reset_composition(&mut self) {
        self.rotation = Rotation::R0;
        self.flip = false;
        self.zoom = 100;
        self.pan_x = 0;
        self.pan_y = 0;
    }

    /// Whether resetting `setting` would change these settings.
    pub fn is_changed(&self, setting: Setting, palettes: &Palettes) -> bool {
        self.with_default(setting, palettes) != *self
    }

    /// Returns settings with one parameter restored to its default.
    pub fn with_default(&self, setting: Setting, palettes: &Palettes) -> Self {
        let mut next = self.clone();
        if !setting.is_active(self) {
            return next;
        }
        let mut defaults = Self::default();
        if let Some(default) = setting.integer_mut(&mut defaults) {
            *setting.integer_mut(&mut next).expect("integer setting") = *default;
            return next;
        }
        match setting {
            Setting::Card => next.card = defaults.card,
            Setting::Mode => next.mode = defaults.mode,
            Setting::Palette => next = next.with_preset(&palettes.presets[0]),
            Setting::Dither => next.dither = defaults.dither,
            Setting::Seed => next.seed = defaults.seed,
            Setting::Serpentine => next.serpentine = defaults.serpentine,
            _ => {
                *setting.value_mut(&mut next).unwrap() = *setting.value_mut(&mut defaults).unwrap()
            }
        }
        next
    }

    /// Returns settings moved `dir` steps and clamped to the range. Mode and
    /// Serpentine toggle, Palette wraps around the presets.
    pub fn adjusted(&self, setting: Setting, dir: i32, palettes: &Palettes) -> Self {
        let mut next = self.clone();
        if !setting.is_active(self) {
            return next;
        }
        let step = |value: f64| {
            let (steps, _, _) = setting.range().expect("numeric parameter");
            setting.clamp(((value * steps).round() + f64::from(dir)) / steps)
        };
        if let Some(value) = setting.integer_mut(&mut next) {
            let (steps, _, _) = setting.range().expect("numeric parameter");
            *value = setting
                .clamp(f64::from(*value) + f64::from(dir) / steps)
                .round() as i32;
        } else {
            match setting {
                Setting::Card => {
                    let current = CARDS
                        .iter()
                        .position(|card| *card == self.card)
                        .unwrap_or(0);
                    next.card = CARDS[wrap(current, dir, CARDS.len())];
                }
                Setting::Mode => {
                    next.mode = match self.mode {
                        Mode::Tone => Mode::Color,
                        Mode::Color => Mode::Tone,
                    }
                }
                Setting::Palette => {
                    let current = palettes.find(self.palette.name()).unwrap_or(0);
                    let i = wrap(current, dir, palettes.len());
                    next = next.with_preset(&palettes.presets[i]);
                }
                Setting::Dither => {
                    let current = Dither::ALL.iter().position(|&d| d == self.dither).unwrap();
                    let i = wrap(current, dir, Dither::ALL.len());
                    next.dither = Dither::ALL[i];
                }
                Setting::Seed => next.seed = self.seed.saturating_add_signed(dir),
                Setting::Serpentine => next.serpentine = !self.serpentine,
                _ => {
                    let value = setting.value_mut(&mut next).unwrap();
                    *value = step(*value);
                }
            }
        }
        next
    }
}

fn wrap(index: usize, dir: i32, len: usize) -> usize {
    (index as i64 + i64::from(dir)).rem_euclid(len as i64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dither_labels_fit_the_panel_and_stay_distinct() {
        let palettes = Palettes::builtin();
        let labels: Vec<_> = Dither::ALL
            .iter()
            .map(|&dither| {
                let settings = Settings {
                    dither,
                    ..Settings::default()
                };
                Setting::Dither.label(&settings, &palettes)
            })
            .collect();
        for label in &labels {
            assert!(label.chars().count() <= 25, "{label}");
        }
        let mut unique = labels.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), labels.len());
    }

    #[test]
    fn reseeding_changes_only_an_active_seed_row() {
        let settings = Settings {
            dither: Dither::Noise,
            seed: u32::MAX,
            ..Settings::default()
        };
        assert_eq!(
            settings.reseeded(Setting::Seed, u64::from(u32::MAX)).seed,
            0
        );
        assert_eq!(settings.reseeded(Setting::Seed, 42).seed, 42);
        let settings = Settings {
            seed: 42,
            ..settings
        };
        assert_eq!(settings.reseeded(Setting::Seed, 42).seed, 43);
        assert!(settings.reseeded(Setting::Strength, 42) == settings);
        let inactive = Settings::default();
        assert!(inactive.reseeded(Setting::Seed, 42) == inactive);
    }

    #[test]
    fn seed_steps_saturate_at_both_bounds() {
        let palettes = Palettes::builtin();
        for (seed, dir, expected) in [
            (0, -1, 0),
            (0, i32::MIN, 0),
            (u32::MAX, 1, u32::MAX),
            (u32::MAX, i32::MAX, u32::MAX),
        ] {
            let settings = Settings {
                dither: Dither::Noise,
                seed,
                ..Settings::default()
            };
            assert_eq!(
                settings.adjusted(Setting::Seed, dir, &palettes).seed,
                expected
            );
        }
    }

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
    fn user_presets_validate_replace_and_preserve_builtins() {
        let mut palettes = Palettes::builtin();
        let builtins: Vec<_> = palettes.iter().cloned().collect();
        let mut preset = Preset {
            name: "Paper".into(),
            colors: vec!["#fff".into(), "#123".into()],
            mode: Some(Mode::Color),
            builtin: true,
        };
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
        assert_eq!(settings.mode, Mode::Color);
        assert_eq!(settings.palette.hex(), preset.colors);
        assert_eq!(settings.palette.colors()[0].rgb, [0; 3]);
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
        let builtin = settings.with_preset(palettes.get(0).unwrap());
        assert_eq!(builtin.mode, Mode::Color);
        let unknown = Settings {
            palette: Palette::new("Template".into(), settings.palette.hex().to_vec()).unwrap(),
            ..settings
        };
        assert_eq!(
            unknown
                .adjusted(Setting::Palette, 1, &palettes)
                .palette
                .name(),
            "4-gray"
        );
        assert_eq!(
            unknown
                .adjusted(Setting::Palette, -1, &palettes)
                .palette
                .name(),
            "Paper"
        );
    }

    #[test]
    fn adjustments_use_unsnapped_steps_and_restore_each_default() {
        let palettes = Palettes::builtin();
        let settings = Settings {
            zoom: 103,
            ..Settings::default()
        };
        let next = settings.adjusted(Setting::Zoom, 1, &palettes);
        assert_eq!(next.zoom, 108);
        assert_eq!(next.with_default(Setting::Zoom, &palettes).zoom, 100);
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
    fn templates_restore_snapshots_while_retaining_composition() {
        let mut settings = Settings {
            dot: 60,
            rotation: Rotation::R90,
            ..Settings::default().with_preset(&Preset {
                name: "Paper".into(),
                colors: vec!["#fff".into(), "#123".into()],
                mode: Some(Mode::Color),
                builtin: false,
            })
        };
        settings.dither = Dither::Bayer4;
        settings.seed = 7;
        settings.strength = 0.85;
        settings.serpentine = false;
        settings.tone = Tone {
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
            flip: true,
            zoom: 135,
            pan_x: -25,
            pan_y: 45,
            ..Settings::default()
        };
        let restored = template.apply(&current).unwrap();
        assert!(
            restored
                == Settings {
                    rotation: current.rotation,
                    flip: current.flip,
                    zoom: current.zoom,
                    pan_x: current.pan_x,
                    pan_y: current.pan_y,
                    ..settings
                }
        );
        template.dot = 100_000;
        template.strength = f64::NAN;
        template.tone.gamma = 9.0;
        let mut templates = Templates::default();
        templates.upsert(template.clone()).unwrap();
        let clamped = templates.get(0).unwrap();
        assert_eq!(clamped.dot, 60);
        assert_eq!(clamped.strength, 0.0);
        assert_eq!(clamped.tone.gamma, 2.5);
        template.palette_hex = vec!["invalid".into()];
        assert!(template.apply(&current).is_err());
    }
}
