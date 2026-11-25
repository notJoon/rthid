use crate::{
    export::Export,
    memo::Memo,
    view::{Pixels, PreviewGeometry, View},
};
use image::{RgbaImage, imageops};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use rthid::dither::{self, Mode, Options};
use std::path::PathBuf;

/// Named colors available in the parameter panel.
pub struct Preset {
    /// Label shown for the preset.
    pub name: &'static str,
    /// Hex colors used to build the palette.
    pub colors: &'static [&'static str],
}

/// Palette presets available in the parameter panel.
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

/// Clockwise rotation of the source image.
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

    /// Whether rotation exchanges width and height.
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

    /// Rotates the image into this orientation.
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
    /// Source size percentage.
    Scale,
    /// Dither color model.
    Mode,
    /// Palette preset.
    Palette,
    /// Dither error strength.
    Strength,
    /// Alternating row direction.
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

    /// Steps per unit and inclusive bounds for numeric parameters.
    pub fn range(self) -> Option<(f64, f64, f64)> {
        match self {
            Self::Scale => Some((0.1, 10.0, 200.0)),
            Self::Strength => Some((10.0, 0.0, 1.0)),
            Self::Brightness => Some((20.0, -0.5, 0.5)),
            Self::Contrast => Some((20.0, -0.5, 1.0)),
            Self::Mode | Self::Palette | Self::Serpentine => None,
        }
    }

    /// Whether changing this parameter selects the dithered preview.
    pub fn shows_dithered(self) -> bool {
        self != Self::Scale
    }
}

/// Editable values restored by the reset keys.
#[derive(Clone, PartialEq)]
pub struct Params {
    /// Source size as a percentage.
    pub scale: i32,
    /// Source image orientation.
    pub rotation: Rotation,
    palette: usize,
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

    /// Returns settings adjusted by the requested number of steps.
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

/// Scaled size before rotation and clockwise quarter turns.
#[derive(PartialEq)]
struct GridKey {
    width: u32,
    height: u32,
    rotation: Rotation,
}

/// Processed grid version and settings used to dither it.
#[derive(PartialEq)]
struct DitherKey {
    grid: u64,
    options: Options,
}

/// Source image and versioned image processing results.
pub struct Pipeline {
    source: RgbaImage,
    grid: Memo<GridKey, RgbaImage>,
    dithered: Memo<DitherKey, Vec<u8>>,
}

impl Pipeline {
    fn scaled_size(&self, params: &Params) -> (u32, u32) {
        let scaled = |n| {
            (f64::from(n) * f64::from(params.scale) / 100.0)
                .round()
                .max(1.0) as u32
        };
        (scaled(self.source.width()), scaled(self.source.height()))
    }

    /// Refreshes required results and reports whether the dithered result rebuilt.
    pub fn refresh(&mut self, params: &Params, need_dither: bool) -> bool {
        let (width, height) = self.scaled_size(params);
        self.grid.update(
            GridKey {
                width,
                height,
                rotation: params.rotation,
            },
            || {
                let scaled =
                    imageops::resize(&self.source, width, height, imageops::FilterType::Triangle);
                params.rotation.apply(scaled)
            },
        );
        if !need_dither {
            return false;
        }
        let grid = self.grid.get().expect("processed grid");
        let (w, h) = grid.dimensions();
        let version = self.grid_version();
        self.dithered.update_by(
            |key| key.grid == version && key.options == params.options,
            || DitherKey {
                grid: version,
                options: params.options.clone(),
            },
            || dither::dither(grid.as_raw(), w as usize, h as usize, &params.options),
        )
    }

    /// Processed image after scaling and rotation.
    pub fn grid(&self) -> &RgbaImage {
        self.grid.get().expect("processed grid")
    }

    /// Build version for downstream preview caches.
    pub fn grid_version(&self) -> u64 {
        self.grid.version()
    }

    /// Palette indices when a consumer has requested dithering.
    pub fn dithered(&self) -> Option<&Vec<u8>> {
        self.dithered.get()
    }

    /// Loaded dimensions before scaling and rotation.
    pub fn source_size(&self) -> (u32, u32) {
        self.source.dimensions()
    }
}

/// Interactive parameters, preview state and image consumers.
pub struct App {
    export: Option<Export>,
    /// Preview position and display mode.
    pub view: View,
    /// Whether keys control the preview.
    pub preview_focus: bool,
    last_drag: Option<(u16, u16)>,
    /// Image processing caches.
    pub pipeline: Pipeline,
    /// Current image settings.
    pub params: Params,
    /// Selected parameter row.
    pub selected: usize,
    /// Whether the preview shows RGB pixels.
    pub show_original: bool,
    /// Whether the event loop should exit.
    pub quit: bool,
}

impl App {
    /// Initializes image consumers and builds the initial preview grid.
    pub fn new(source: RgbaImage, preview_png: Option<PathBuf>) -> Self {
        let mut app = Self {
            export: preview_png.map(Export::new),
            view: View::default(),
            preview_focus: false,
            last_drag: None,
            pipeline: Pipeline {
                source,
                grid: Memo::default(),
                dithered: Memo::default(),
            },
            params: Params::default(),
            selected: 0,
            show_original: true,
            quit: false,
        };
        app.refresh();
        app
    }

    /// Returns scaled dimensions after rotation.
    pub fn grid_size(&self) -> (u32, u32) {
        let (w, h) = self.pipeline.scaled_size(&self.params);
        if self.params.rotation.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// Refreshes image consumers and clamps the view to the preview area.
    pub fn sync(&mut self, preview: Rect) {
        self.refresh();
        self.clamp_view(preview);
    }

    /// Coordinates shared by preview input and rendering.
    pub fn geometry(&self, area: Rect) -> PreviewGeometry {
        crate::view::geometry(&self.view, self.grid_size(), area)
    }

    /// Keeps the view within the current grid and preview area.
    pub fn clamp_view(&mut self, area: Rect) {
        let (w, h) = self.grid_size();
        self.view.clamp(w, h, area);
    }

    fn refresh(&mut self) {
        let need_dither = !self.show_original || self.has_export();
        if self.pipeline.refresh(&self.params, need_dither)
            && let Some(export) = &mut self.export
        {
            let (w, h) = self.pipeline.grid().dimensions();
            export.sync(
                self.pipeline.dithered().expect("dithered grid"),
                w,
                h,
                &self.params.options.palette,
            );
        }
    }

    /// Whether a companion PNG writer needs dithered pixels.
    pub fn has_export(&self) -> bool {
        self.export.as_ref().is_some_and(Export::needs_dither)
    }

    /// Collects PNG results and reports whether the status changed.
    pub fn poll_export(&mut self) -> bool {
        self.export.as_mut().is_some_and(Export::poll)
    }

    /// Latest companion PNG error for the status line.
    pub fn export_status(&self) -> Option<&str> {
        self.export.as_ref().and_then(Export::status)
    }

    /// Selected preview pixels after sync.
    ///
    /// Panics in dithered mode if its result was never built.
    pub fn displayed(&self) -> Pixels<'_> {
        if self.show_original {
            Pixels::Rgb(self.pipeline.grid(), self.pipeline.grid_version())
        } else {
            Pixels::Indexed {
                indices: self.pipeline.dithered().expect("dithered grid"),
                palette: &self.params.options.palette,
            }
        }
    }

    /// Flushes pending image changes and waits for the PNG worker.
    pub fn finish_png(&mut self) -> std::io::Result<()> {
        // A change and quit can arrive in the same input batch.
        self.refresh();
        self.export.as_mut().map_or(Ok(()), Export::finish)
    }

    /// Applies mouse input using the current preview coordinates.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, geometry: &PreviewGeometry) {
        let prev = self.last_drag.take();
        let point = (mouse.column, mouse.row);
        if !self.view.exact || !geometry.area.contains(point.into()) {
            return;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(minimap) = geometry.minimap
                    && minimap.map.contains(point.into())
                {
                    self.view.center_on_minimap(point, geometry);
                } else {
                    if matches!(mouse.kind, MouseEventKind::Drag(_))
                        && let Some((x, y)) = prev
                    {
                        self.view.pan(
                            i64::from(x) - i64::from(mouse.column),
                            (i64::from(y) - i64::from(mouse.row)) * 2,
                            geometry,
                        );
                    }
                    self.last_drag = Some(point);
                }
            }
            MouseEventKind::ScrollUp => self.view.pan(0, -6, geometry),
            MouseEventKind::ScrollDown => self.view.pan(0, 6, geometry),
            // Trackpads send horizontal swipes as sideways scrolls.
            MouseEventKind::ScrollLeft => self.view.pan(-6, 0, geometry),
            MouseEventKind::ScrollRight => self.view.pan(6, 0, geometry),
            _ => {}
        }
    }

    // TODO: Introduce an Action enum with a pure key mapping once more bindings arrive.
    /// Applies a key using the current preview coordinates for panning.
    pub fn handle_key(&mut self, key: KeyEvent, geometry: &PreviewGeometry) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            // Raw mode delivers Ctrl+C as a key. Other Ctrl chords are not bound.
            self.quit |= key.code == KeyCode::Char('c');
            return;
        }
        if key.code == KeyCode::Tab {
            self.preview_focus = !self.preview_focus;
            return;
        }
        if key.code == KeyCode::Char(' ') {
            self.show_original = !self.show_original;
            return;
        }
        // Esc leaves preview focus. Only q or Ctrl+C exits.
        if key.code == KeyCode::Esc {
            self.preview_focus = false;
            return;
        }
        if self.preview_focus {
            match key.code {
                KeyCode::Char('m') => {
                    self.view.hide_minimap = !self.view.hide_minimap;
                    return;
                }
                KeyCode::Char('z') => {
                    self.view.exact = !self.view.exact;
                    self.last_drag = None;
                    return;
                }
                _ => {}
            }
            let pan = pan_delta(key.code, geometry.visible);
            if let Some((dx, dy)) = pan {
                if self.view.exact {
                    self.view.pan(dx, dy, geometry);
                }
                return;
            }
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(Param::ALL.len() - 1);
            }
            KeyCode::Char('r') => self.params.rotation = self.params.rotation.cw(),
            KeyCode::Char('R') => self.params.rotation = self.params.rotation.ccw(),
            KeyCode::Char('d') => self.params = self.params.with_default(Param::ALL[self.selected]),
            KeyCode::Char('D') => self.params = Params::default(),
            KeyCode::Left | KeyCode::Char('h') => self.adjust(-1),
            KeyCode::Right | KeyCode::Char('l') => self.adjust(1),
            _ => {}
        }
    }

    fn adjust(&mut self, direction: i32) {
        let param = Param::ALL[self.selected];
        let next = self.params.adjusted(param, direction);
        // Clamped presses preserve the current preview selection.
        if next != self.params && param.shows_dithered() {
            self.show_original = false;
        }
        self.params = next;
    }
}

fn pan_delta(code: KeyCode, visible: (u32, u32)) -> Option<(i64, i64)> {
    let page_x = i64::from(visible.0.saturating_sub(2).max(1));
    let page_y = i64::from(visible.1.saturating_sub(2).max(1));
    match code {
        KeyCode::Char('H') => Some((-page_x, 0)),
        KeyCode::Char('L') => Some((page_x, 0)),
        KeyCode::Char('K') => Some((0, -page_y)),
        KeyCode::Char('J') => Some((0, page_y)),
        KeyCode::Left | KeyCode::Char('h') => Some((-1, 0)),
        KeyCode::Right | KeyCode::Char('l') => Some((1, 0)),
        KeyCode::Up | KeyCode::Char('k') => Some((0, -1)),
        KeyCode::Down | KeyCode::Char('j') => Some((0, 1)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Layout;
    use ratatui::layout::Rect;

    #[test]
    fn rotation_preserves_pixels_and_cycles() {
        let image = RgbaImage::from_raw(2, 3, (0..24).collect()).unwrap();
        for (rotation, degrees, expected) in [
            (Rotation::R0, 0, image.clone()),
            (Rotation::R90, 90, imageops::rotate90(&image)),
            (Rotation::R180, 180, imageops::rotate180(&image)),
            (Rotation::R270, 270, imageops::rotate270(&image)),
        ] {
            assert_eq!(rotation.cw().ccw(), rotation);
            assert_eq!(rotation.ccw().cw(), rotation);
            assert_eq!(rotation.cw().cw().cw().cw(), rotation);
            assert_eq!(rotation.degrees(), degrees);
            let rotated = rotation.apply(image.clone());
            assert_eq!(rotated, expected);
            assert_eq!(
                rotated.dimensions() != image.dimensions(),
                rotation.swaps_axes()
            );
        }
    }

    #[test]
    fn adjustments_and_defaults_leave_the_input_unchanged() {
        let params = Params {
            scale: 105,
            ..Params::default()
        };
        let next = params.adjusted(Param::Scale, 1);
        assert_eq!(params.scale, 105);
        assert_eq!(next.scale, 115);
        assert_eq!(next.with_default(Param::Scale).scale, 100);
        for param in Param::ALL {
            let next = params.adjusted(param, -1);
            let reset = next.with_default(param);
            assert_eq!(param.label(&reset), param.label(&Params::default()));
            assert_eq!(param.shows_dithered(), param != Param::Scale);
            if param == Param::Palette {
                assert_eq!(next.palette_name(), PALETTES[2].name);
                assert_eq!(
                    next.options.palette,
                    dither::make_palette(PALETTES[2].colors).unwrap()
                );
            }
        }
    }

    #[test]
    fn pan_keys_use_pixels_and_visible_pages() {
        for (codes, expected) in [
            ([KeyCode::Left, KeyCode::Char('h')], (-1, 0)),
            ([KeyCode::Right, KeyCode::Char('l')], (1, 0)),
            ([KeyCode::Up, KeyCode::Char('k')], (0, -1)),
            ([KeyCode::Down, KeyCode::Char('j')], (0, 1)),
        ] {
            for code in codes {
                assert_eq!(pan_delta(code, (20, 12)), Some(expected));
            }
        }
        for (code, expected) in [
            ('H', (-18, 0)),
            ('L', (18, 0)),
            ('K', (0, -10)),
            ('J', (0, 10)),
        ] {
            assert_eq!(pan_delta(KeyCode::Char(code), (20, 12)), Some(expected));
        }
        for visible in [(0, 0), (1, 1), (2, 2)] {
            assert_eq!(pan_delta(KeyCode::Char('H'), visible), Some((-1, 0)));
            assert_eq!(pan_delta(KeyCode::Char('J'), visible), Some((0, 1)));
        }
        for code in ['m', 'z', 'q'] {
            assert_eq!(pan_delta(KeyCode::Char(code), (20, 12)), None);
        }
    }

    fn test_layout() -> Layout {
        crate::ui::layout(Rect::new(0, 0, 41, 9))
    }

    // Compares build versions after applying the key.
    fn press(app: &mut App, code: KeyCode) -> bool {
        let before = (app.pipeline.grid.version(), app.pipeline.dithered.version());
        app.handle_key(
            KeyEvent::new(code, KeyModifiers::NONE),
            &app.geometry(test_layout().preview),
        );
        app.sync(test_layout().preview);
        before != (app.pipeline.grid.version(), app.pipeline.dithered.version())
    }

    fn app_at(param: Param) -> App {
        let mut app = App::new(RgbaImage::new(4, 4), None);
        app.selected = Param::ALL.iter().position(|&p| p == param).unwrap();
        app
    }

    #[test]
    fn pipeline_keys_control_builds() {
        let mut app = App::new(RgbaImage::new(20, 10), None);
        assert_eq!(app.pipeline.grid.version(), 1);
        assert_eq!(app.pipeline.dithered.version(), 0);
        app.show_original = false;
        app.sync(test_layout().preview);
        assert_eq!(app.pipeline.dithered.version(), 1);
        app.sync(test_layout().preview);
        assert_eq!(app.pipeline.grid.version(), 1);
        assert_eq!(app.pipeline.dithered.version(), 1);
        app.params.options.strength = 0.5;
        app.sync(test_layout().preview);
        assert_eq!(app.pipeline.grid.version(), 1);
        assert_eq!(app.pipeline.dithered.version(), 2);
        app.show_original = true;
        app.params.rotation = Rotation::R180;
        app.sync(test_layout().preview);
        assert_eq!(app.pipeline.grid.version(), 2);
        assert_eq!(app.pipeline.dithered.version(), 2);
        app.show_original = false;
        app.sync(test_layout().preview);
        assert_eq!(app.pipeline.dithered.version(), 3);
    }

    #[test]
    fn sync_clamps_after_layout_and_grid_changes() {
        let mut app = App::new(RgbaImage::new(120, 80), None);
        for (screen, scale, rotation, expected) in [
            (Rect::new(0, 0, 41, 9), 100, Rotation::R0, (110, 70)),
            (Rect::new(0, 0, 100, 40), 100, Rotation::R0, (51, 8)),
            (Rect::new(0, 0, 100, 40), 50, Rotation::R0, (0, 0)),
            (Rect::new(0, 0, 100, 40), 100, Rotation::R90, (11, 48)),
        ] {
            app.params.scale = scale;
            app.params.rotation = rotation;
            app.view.x = 999;
            app.view.y = 999;
            app.sync(crate::ui::layout(screen).preview);
            assert_eq!((app.view.x, app.view.y), expected);
            assert_eq!(
                app.pipeline.grid.get().unwrap().dimensions(),
                app.grid_size()
            );
        }
    }

    #[test]
    fn preview_toggle_and_adjustments_select_view_without_extra_redithering() {
        let mut app = App::new(RgbaImage::new(20, 10), None);
        app.selected = Param::ALL
            .iter()
            .position(|&p| p == Param::Strength)
            .unwrap();
        assert!(app.show_original);
        for (focus, rebuilt) in [(false, true), (true, false)] {
            app.preview_focus = focus;
            assert_eq!(press(&mut app, KeyCode::Char(' ')), rebuilt);
            assert!(!app.show_original);
            assert!(!press(&mut app, KeyCode::Char(' ')));
            assert!(app.show_original);
        }
        app.preview_focus = false;
        assert!(!press(&mut app, KeyCode::Right));
        assert!(app.show_original);
        assert!(press(&mut app, KeyCode::Left));
        assert!(!app.show_original);
        assert!(!press(&mut app, KeyCode::Char(' ')));
        app.selected = 0;
        assert!(press(&mut app, KeyCode::Left));
        assert!(app.show_original);
        for code in [KeyCode::Char('r'), KeyCode::Char('d'), KeyCode::Char('D')] {
            assert!(press(&mut app, code));
            assert!(app.show_original);
        }
    }

    #[test]
    fn original_defers_dithering_until_space_then_refresh() {
        let mut app = App::new(RgbaImage::new(20, 10), None);
        assert!(app.pipeline.dithered.get().is_none());
        assert!(press(&mut app, KeyCode::Right));
        assert_eq!(app.pipeline.grid.get().unwrap().dimensions(), (22, 11));
        assert!(app.show_original);
        assert!(app.pipeline.dithered.get().is_none());
        assert_eq!(app.pipeline.dithered.version(), 0);
        app.handle_key(
            KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
            &app.geometry(test_layout().preview),
        );
        assert_eq!(app.pipeline.dithered.version(), 0);
        app.sync(test_layout().preview);
        let (w, h) = app.grid_size();
        assert_eq!(app.pipeline.dithered.get().unwrap().len(), (w * h) as usize);
        assert_eq!(app.pipeline.dithered.version(), 1);
    }

    #[test]
    fn original_preserves_rgb_and_ignores_alpha() {
        let source = RgbaImage::from_raw(
            3,
            1,
            vec![255, 0, 0, 0, 25, 26, 128, 255, 255, 255, 255, 255],
        )
        .unwrap();
        let app = App::new(source, None);
        let Pixels::Rgb(pixels, _) = app.displayed() else {
            panic!("expected original RGB pixels");
        };
        assert_eq!(
            pixels.as_raw(),
            &[255, 0, 0, 0, 25, 26, 128, 255, 255, 255, 255, 255]
        );
        assert_eq!(app.pipeline.grid.get().unwrap(), pixels);
    }

    #[test]
    fn navigation_stays_in_range_and_skips_redither() {
        let mut app = app_at(Param::Scale);
        assert!(!press(&mut app, KeyCode::Up));
        assert_eq!(app.selected, 0);
        for _ in 0..10 {
            assert!(!press(&mut app, KeyCode::Char('j')));
        }
        assert_eq!(Param::ALL[app.selected], Param::Contrast);
        assert!(!press(&mut app, KeyCode::Char('x')));
    }

    #[test]
    fn numeric_params_clamp_without_redither() {
        type Getter = fn(&Params) -> f64;
        let cases: [(Param, Getter); 4] = [
            (Param::Scale, |p| f64::from(p.scale)),
            (Param::Strength, |p| p.options.strength),
            (Param::Brightness, |p| p.options.tone.brightness),
            (Param::Contrast, |p| p.options.tone.contrast),
        ];
        for (param, get) in cases {
            let (_, low, high) = param.range().unwrap();
            let mut app = app_at(param);
            for (key, limit) in [(KeyCode::Left, low), (KeyCode::Right, high)] {
                for _ in 0..50 {
                    press(&mut app, key);
                }
                assert_eq!(get(&app.params), limit, "{param:?}");
                assert!(!press(&mut app, key), "{param:?} re-dithered at its limit");
            }
        }
    }

    #[test]
    fn toggles_and_palette_cycle() {
        let mut app = app_at(Param::Mode);
        assert!(press(&mut app, KeyCode::Char('l')));
        assert!(matches!(app.params.options.mode, Mode::Color));
        let mut app = app_at(Param::Serpentine);
        press(&mut app, KeyCode::Right);
        assert!(!app.params.options.serpentine);
        let mut app = app_at(Param::Palette);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.params.palette, 2);
        assert_eq!(app.params.options.palette.len(), 4);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.params.palette, 0);
        assert_eq!(app.params.options.palette.len(), 2);
    }

    #[test]
    fn reset_restores_selected_or_all_defaults() {
        let mut app = app_at(Param::Scale);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('r'));
        app.selected = 1;
        press(&mut app, KeyCode::Right);
        assert!(press(&mut app, KeyCode::Char('d')));
        assert!(matches!(app.params.options.mode, Mode::Tone));
        assert_eq!(
            (app.params.scale, app.params.rotation),
            (110, Rotation::R90)
        );
        app.selected = 2;
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('D'));
        let p = &app.params;
        assert_eq!((p.scale, p.rotation, p.palette), (100, Rotation::R0, 0));
        assert_eq!(p.options.palette.len(), 2);
        assert_eq!(app.pipeline.dithered.get().unwrap().len(), 16);
    }

    #[test]
    fn reset_only_redithers_changed_values_and_preserves_pending_changes() {
        for param in Param::ALL {
            let mut app = App::new(RgbaImage::new(20, 10), None);
            app.selected = Param::ALL.iter().position(|&p| p == param).unwrap();
            assert!(!press(&mut app, KeyCode::Char('d')), "{param:?}");
            assert!(!press(&mut app, KeyCode::Char('D')), "{param:?}");
            assert!(press(&mut app, KeyCode::Left));
            assert!(press(&mut app, KeyCode::Char('d')), "{param:?}");
            assert!(!press(&mut app, KeyCode::Char('d')), "{param:?}");
            assert!(press(&mut app, KeyCode::Left));
            assert!(press(&mut app, KeyCode::Char('D')), "{param:?}");
            assert!(!press(&mut app, KeyCode::Char('D')), "{param:?}");
        }
        let mut app = app_at(Param::Scale);
        press(&mut app, KeyCode::Char('r'));
        assert!(!press(&mut app, KeyCode::Char('d')));
        assert!(press(&mut app, KeyCode::Char('D')));
        app.params.options.tone.gamma = 2.0;
        app.show_original = false;
        app.sync(test_layout().preview);
        assert!(press(&mut app, KeyCode::Char('D')));
        assert_eq!(app.params.options.tone.gamma, 1.0);
        app.handle_key(
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &app.geometry(test_layout().preview),
        );
        app.handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &app.geometry(test_layout().preview),
        );
        let version = (app.pipeline.grid.version(), app.pipeline.dithered.version());
        app.refresh();
        assert_eq!(
            (app.pipeline.grid.version(), app.pipeline.dithered.version()),
            version
        );
    }

    #[test]
    fn quit_keys_and_ctrl_chords() {
        let mut app = app_at(Param::Scale);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Esc);
        assert!(!app.preview_focus);
        press(&mut app, KeyCode::Esc);
        assert!(!app.quit);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
        let mut app = app_at(Param::Scale);
        let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        app.handle_key(release, &app.geometry(test_layout().preview));
        assert!(!app.quit);
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let version = (app.pipeline.grid.version(), app.pipeline.dithered.version());
        app.handle_key(ctrl('r'), &app.geometry(test_layout().preview));
        app.refresh();
        assert_eq!(
            (app.pipeline.grid.version(), app.pipeline.dithered.version()),
            version
        );
        app.handle_key(ctrl('c'), &app.geometry(test_layout().preview));
        assert!(app.quit);
    }

    #[test]
    fn rotation_and_scale_set_the_actual_dither_grid() {
        let mut app = App::new(RgbaImage::new(256, 128), None);
        app.show_original = false;
        assert_eq!(app.grid_size(), (256, 128));
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.grid_size(), (128, 256));
        assert_eq!(app.pipeline.dithered.get().unwrap().len(), 128 * 256);
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.grid_size(), (256, 128));
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(app.params.rotation, Rotation::R90);
        press(&mut app, KeyCode::Char('R'));
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(app.params.rotation, Rotation::R270);
        assert_eq!(app.grid_size(), (128, 256));
        let mut app = App::new(RgbaImage::new(15, 1), None);
        app.show_original = false;
        for _ in 0..9 {
            press(&mut app, KeyCode::Left);
        }
        assert_eq!(app.grid_size(), (2, 1));
        assert_eq!(app.pipeline.dithered.get().unwrap().len(), 2);
    }
    #[test]
    fn tab_and_preview_keys_change_view_without_redithering() {
        let mut app = App::new(RgbaImage::new(40, 40), None);
        assert!(!press(&mut app, KeyCode::Char('z')));
        assert!(!app.view.exact);
        assert!(!press(&mut app, KeyCode::Tab));
        assert!(app.preview_focus);
        assert!(!press(&mut app, KeyCode::Char('z')));
        assert!(app.view.exact);
        for key in [
            KeyCode::Right,
            KeyCode::Char('l'),
            KeyCode::Down,
            KeyCode::Char('j'),
        ] {
            assert!(!press(&mut app, key));
        }
        assert_eq!((app.view.x, app.view.y), (2, 2));
        assert_eq!(app.selected, 0);
        for _ in 0..100 {
            press(&mut app, KeyCode::Right);
            press(&mut app, KeyCode::Down);
        }
        assert_eq!((app.view.x, app.view.y), (30, 30));
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Up);
        assert_eq!((app.view.x, app.view.y), (29, 29));
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.view.x, 29);
        press(&mut app, KeyCode::Tab);
        assert!(press(&mut app, KeyCode::Right));
        assert_eq!(app.params.scale, 110);
    }

    #[test]
    fn minimap_clicks_match_drawn_geometry_for_each_glyph_set() {
        use crate::view::Glyphs;
        use ratatui::buffer::Buffer;
        for (glyphs, expected, center) in [
            (Glyphs::Sextant, Rect::new(38, 4, 9, 4), (36, 27)),
            (Glyphs::Quadrant, Rect::new(38, 4, 9, 4), (36, 27)),
        ] {
            let mut app = App::new(RgbaImage::new(120, 120), None);
            let area = Rect::new(2, 3, 48, 18);
            app.view.exact = true;
            app.view.glyphs = glyphs;
            let mut buffer = Buffer::empty(area);
            let geometry = app.geometry(area);
            let map = geometry.minimap.unwrap().map;
            crate::view::render(
                app.displayed(),
                &app.view,
                &geometry,
                &mut buffer,
                &mut crate::view::OverviewCaches::default(),
            );
            assert_eq!(map, expected);
            assert_ne!(buffer[(map.x, map.y)].bg, ratatui::style::Color::Reset);
            for (kind, column, row, expected) in [
                (
                    MouseEventKind::Down(MouseButton::Left),
                    map.x + map.width / 2,
                    map.y + 1,
                    center,
                ),
                (
                    MouseEventKind::Drag(MouseButton::Left),
                    map.right() - 1,
                    map.bottom() - 1,
                    (72, 84),
                ),
                (
                    MouseEventKind::Drag(MouseButton::Left),
                    map.x,
                    map.y,
                    (0, 0),
                ),
            ] {
                let version = (app.pipeline.grid.version(), app.pipeline.dithered.version());
                app.handle_mouse(
                    MouseEvent {
                        kind,
                        column,
                        row,
                        modifiers: KeyModifiers::NONE,
                    },
                    &app.geometry(area),
                );
                assert_eq!((app.view.x, app.view.y), expected, "{glyphs:?}");
                app.refresh();
                assert_eq!(
                    (app.pipeline.grid.version(), app.pipeline.dithered.version()),
                    version
                );
            }
        }
    }

    #[test]
    fn mouse_centers_pans_and_scrolls_without_focus_or_redithering() {
        let mut app = App::new(RgbaImage::new(120, 120), None);
        let area = Rect::new(2, 3, 48, 18);
        app.view.exact = true;
        let map = app.geometry(area).minimap.unwrap().map;
        let mouse = |app: &mut App, kind, column, row| {
            let version = (app.pipeline.grid.version(), app.pipeline.dithered.version());
            app.handle_mouse(
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                &app.geometry(area),
            );
            app.refresh();
            assert_eq!(
                (app.pipeline.grid.version(), app.pipeline.dithered.version()),
                version
            );
            assert!(!app.preview_focus);
        };
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            map.x + 4,
            map.y + 1,
        );
        assert_eq!((app.view.x, app.view.y), (36, 27));
        mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            map.right() - 1,
            map.bottom() - 1,
        );
        assert_eq!((app.view.x, app.view.y), (72, 84));
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 10, 10);
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 10, 10);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 13, 12);
        assert_eq!((app.view.x, app.view.y), (69, 80));
        mouse(&mut app, MouseEventKind::ScrollUp, 13, 12);
        assert_eq!(app.view.y, 74);
        mouse(&mut app, MouseEventKind::ScrollDown, 13, 12);
        assert_eq!(app.view.y, 80);
        mouse(&mut app, MouseEventKind::ScrollLeft, 13, 12);
        assert_eq!(app.view.x, 63);
        mouse(&mut app, MouseEventKind::ScrollRight, 13, 12);
        assert_eq!(app.view.x, 69);
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Right),
            map.x,
            map.y,
        );
        mouse(&mut app, MouseEventKind::ScrollDown, 0, 0);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 14, 13);
        assert_eq!((app.view.x, app.view.y), (69, 80));
        app.view.exact = false;
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            map.x,
            map.y,
        );
        mouse(&mut app, MouseEventKind::ScrollDown, 13, 12);
        assert_eq!((app.view.x, app.view.y), (69, 80));
    }

    #[test]
    fn preview_page_keys_use_overlap_and_clamp_without_redithering() {
        let mut app = App::new(RgbaImage::new(40, 40), None);
        app.preview_focus = true;
        app.view.exact = true;
        assert!(!press(&mut app, KeyCode::Char('L')));
        assert!(!press(&mut app, KeyCode::Char('J')));
        assert_eq!((app.view.x, app.view.y), (8, 8));
        assert!(!press(&mut app, KeyCode::Char('H')));
        assert!(!press(&mut app, KeyCode::Char('K')));
        assert_eq!((app.view.x, app.view.y), (0, 0));
        for _ in 0..10 {
            press(&mut app, KeyCode::Char('L'));
            press(&mut app, KeyCode::Char('J'));
        }
        assert_eq!((app.view.x, app.view.y), (30, 30));
    }

    #[test]
    fn finish_png_exports_changes_queued_with_quit() {
        let path = std::env::temp_dir().join(format!("rthid-quit-test-{}.png", std::process::id()));
        let mut app = App::new(RgbaImage::new(4, 2), Some(path.clone()));
        for code in [KeyCode::Char('r'), KeyCode::Char('q')] {
            app.handle_key(
                KeyEvent::new(code, KeyModifiers::NONE),
                &app.geometry(test_layout().preview),
            );
        }
        assert!(app.quit);
        assert_eq!(app.pipeline.grid.version(), 1);
        app.finish_png().unwrap();
        assert_eq!(image::open(&path).unwrap().to_rgba8().dimensions(), (8, 16));
        std::fs::remove_file(path).unwrap();
    }
}
