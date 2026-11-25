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

/// Palette presets available in the parameter panel.
pub const PALETTES: [(&str, &[&str]); 3] = [
    ("Black/white", &["#000", "#fff"]),
    ("4-gray", &["#000", "#555", "#aaa", "#fff"]),
    ("Warm inks", &["#ffffff", "#ee6a45", "#a3300f", "#1c0d07"]),
];

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

/// Parameter panel order.
pub const PARAMS: [Param; 7] = [
    Param::Scale,
    Param::Mode,
    Param::Palette,
    Param::Strength,
    Param::Serpentine,
    Param::Brightness,
    Param::Contrast,
];

/// Editable values restored by the reset keys.
#[derive(Clone, PartialEq)]
pub struct Params {
    /// Source size as a percentage.
    pub scale: i32,
    /// Clockwise quarter turns.
    pub rotation: u8,
    /// Selected palette preset.
    pub palette: usize,
    /// Dither and tone settings.
    pub options: Options,
}

impl Default for Params {
    fn default() -> Self {
        let mut params = Self {
            scale: 100,
            rotation: 0,
            palette: 0,
            options: Options::default(),
        };
        params.set_palette(0);
        params
    }
}

impl Params {
    fn set_palette(&mut self, i: usize) {
        self.palette = i;
        self.options.palette = dither::make_palette(PALETTES[i].1).expect("valid preset");
    }
}

/// Scaled size before rotation and clockwise quarter turns.
#[derive(PartialEq)]
struct GridKey {
    width: u32,
    height: u32,
    rotation: u8,
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
                match params.rotation {
                    1 => imageops::rotate90(&scaled),
                    2 => imageops::rotate180(&scaled),
                    3 => imageops::rotate270(&scaled),
                    _ => scaled,
                }
            },
        );
        if !need_dither {
            return false;
        }
        let grid = self.grid.get().expect("processed grid");
        let (w, h) = grid.dimensions();
        let key = DitherKey {
            grid: self.grid_version(),
            options: params.options.clone(),
        };
        self.dithered.update(key, || {
            dither::dither(grid.as_raw(), w as usize, h as usize, &params.options)
        })
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

    /// Mutable indices for rendering fixtures.
    #[cfg(test)]
    pub fn dithered_mut(&mut self) -> &mut Vec<u8> {
        self.dithered.get_mut().unwrap()
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
        if self.params.rotation.is_multiple_of(2) {
            (w, h)
        } else {
            (h, w)
        }
    }

    /// Refreshes image consumers and clamps the view to the preview area.
    pub fn sync(&mut self, preview: Rect) {
        self.refresh();
        let (w, h) = self.grid_size();
        self.view.clamp(w, h, preview);
    }

    fn refresh(&mut self) {
        let need_dither =
            !self.show_original || self.export.as_ref().is_some_and(Export::needs_dither);
        if self.pipeline.refresh(&self.params, need_dither)
            && let Some(export) = &mut self.export
        {
            let (w, h) = self.pipeline.grid().dimensions();
            export.sync(
                self.pipeline.dithered().expect("dithered grid"),
                w,
                h,
                &self.params.options.palette,
                self.pipeline.dithered.version(),
            );
        }
    }

    /// Whether a companion PNG destination was requested.
    pub fn has_export(&self) -> bool {
        self.export.is_some()
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
        match &mut self.export {
            Some(export) => export.finish(),
            None => Ok(()),
        }
    }

    /// Applies mouse input using the current preview coordinates.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, geometry: &PreviewGeometry) {
        let prev = self.last_drag.take();
        let point = (mouse.column, mouse.row);
        if !self.view.exact || !geometry.area.contains(point.into()) {
            return;
        }
        let (w, h) = geometry.grid;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(minimap) = geometry.minimap
                    && minimap.map.contains(point.into())
                {
                    self.view
                        .center_on_minimap(point, w, h, geometry.area, minimap.map);
                } else {
                    if matches!(mouse.kind, MouseEventKind::Drag(_))
                        && let Some((x, y)) = prev
                    {
                        self.view.pan(
                            i64::from(x) - i64::from(mouse.column),
                            (i64::from(y) - i64::from(mouse.row)) * 2,
                            w,
                            h,
                            geometry.area,
                        );
                    }
                    self.last_drag = Some(point);
                }
            }
            MouseEventKind::ScrollUp => self.view.pan(0, -6, w, h, geometry.area),
            MouseEventKind::ScrollDown => self.view.pan(0, 6, w, h, geometry.area),
            // Trackpads send horizontal swipes as sideways scrolls.
            MouseEventKind::ScrollLeft => self.view.pan(-6, 0, w, h, geometry.area),
            MouseEventKind::ScrollRight => self.view.pan(6, 0, w, h, geometry.area),
            _ => {}
        }
    }

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
            let (w, h) = geometry.grid;
            let (vw, vh) = geometry.visible;
            let page_x = i64::from(vw.saturating_sub(2).max(1));
            let page_y = i64::from(vh.saturating_sub(2).max(1));
            let pan = match key.code {
                KeyCode::Char('m') => {
                    self.view.hide_minimap = !self.view.hide_minimap;
                    return;
                }
                KeyCode::Char('z') => {
                    self.view.exact = !self.view.exact;
                    self.last_drag = None;
                    return;
                }
                KeyCode::Char('H') => Some((-page_x, 0)),
                KeyCode::Char('L') => Some((page_x, 0)),
                KeyCode::Char('K') => Some((0, -page_y)),
                KeyCode::Char('J') => Some((0, page_y)),
                KeyCode::Left | KeyCode::Char('h') => Some((-1, 0)),
                KeyCode::Right | KeyCode::Char('l') => Some((1, 0)),
                KeyCode::Up | KeyCode::Char('k') => Some((0, -1)),
                KeyCode::Down | KeyCode::Char('j') => Some((0, 1)),
                _ => None,
            };
            if let Some((dx, dy)) = pan {
                if self.view.exact {
                    self.view.pan(dx, dy, w, h, geometry.area);
                }
                return;
            }
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(PARAMS.len() - 1);
            }
            KeyCode::Char('r') => self.rotate(1),
            KeyCode::Char('R') => self.rotate(3),
            KeyCode::Char('d') => self.reset(false),
            KeyCode::Char('D') => self.reset(true),
            KeyCode::Left | KeyCode::Char('h') => self.adjust(-1),
            KeyCode::Right | KeyCode::Char('l') => self.adjust(1),
            _ => {}
        }
    }

    fn rotate(&mut self, quarter_turns: u8) {
        self.params.rotation = (self.params.rotation + quarter_turns) % 4;
    }

    // Restores the selected parameter, or every parameter and the rotation when `all` is set.
    fn reset(&mut self, all: bool) {
        let defaults = Params::default();
        let (p, d) = (&mut self.params, &defaults.options);
        match PARAMS[self.selected] {
            _ if all => *p = defaults.clone(),
            Param::Scale => p.scale = defaults.scale,
            Param::Mode => p.options.mode = d.mode,
            Param::Palette => p.set_palette(defaults.palette),
            Param::Strength => p.options.strength = d.strength,
            Param::Serpentine => p.options.serpentine = d.serpentine,
            Param::Brightness => p.options.tone.brightness = d.tone.brightness,
            Param::Contrast => p.options.tone.contrast = d.tone.contrast,
        }
    }

    fn adjust(&mut self, direction: i32) {
        let p = &mut self.params;
        let o = &mut p.options;
        // Clamped presses preserve the current preview selection.
        let step = |value: &mut f64, steps: f64, low: f64, high: f64| {
            let next = (((*value * steps).round() + f64::from(direction)) / steps).clamp(low, high);
            let changed = next != *value;
            *value = next;
            changed
        };
        let param = PARAMS[self.selected];
        let changed = match param {
            Param::Scale => {
                let next = (p.scale + direction * 10).clamp(10, 200);
                let changed = next != p.scale;
                p.scale = next;
                changed
            }
            Param::Mode => {
                o.mode = match o.mode {
                    Mode::Tone => Mode::Color,
                    Mode::Color => Mode::Tone,
                };
                true
            }
            Param::Palette => {
                let i = (p.palette as i32 + direction).rem_euclid(PALETTES.len() as i32);
                p.set_palette(i as usize);
                true
            }
            Param::Strength => step(&mut o.strength, 10.0, 0.0, 1.0),
            Param::Serpentine => {
                o.serpentine = !o.serpentine;
                true
            }
            Param::Brightness => step(&mut o.tone.brightness, 20.0, -0.5, 0.5),
            Param::Contrast => step(&mut o.tone.contrast, 20.0, -0.5, 1.0),
        };
        if changed && param != Param::Scale {
            self.show_original = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::Layout;
    use ratatui::layout::Rect;

    fn test_layout() -> Layout {
        crate::ui::layout(Rect::new(0, 0, 41, 9))
    }

    fn preview_geometry(app: &App, area: Rect) -> PreviewGeometry {
        crate::view::geometry(&app.view, app.grid_size(), area)
    }

    // Compares build versions after applying the key.
    fn press(app: &mut App, code: KeyCode) -> bool {
        let before = (app.pipeline.grid.version(), app.pipeline.dithered.version());
        app.handle_key(
            KeyEvent::new(code, KeyModifiers::NONE),
            &preview_geometry(app, test_layout().preview),
        );
        app.sync(test_layout().preview);
        before != (app.pipeline.grid.version(), app.pipeline.dithered.version())
    }

    fn app_at(param: Param) -> App {
        let mut app = App::new(RgbaImage::new(4, 4), None);
        app.selected = PARAMS.iter().position(|&p| p == param).unwrap();
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
        app.params.rotation = 2;
        app.sync(test_layout().preview);
        assert_eq!(app.pipeline.grid.version(), 2);
        assert_eq!(app.pipeline.dithered.version(), 2);
        app.show_original = false;
        app.sync(test_layout().preview);
        assert_eq!(app.pipeline.dithered.version(), 3);
        app.pipeline.dithered.clear();
        app.sync(test_layout().preview);
        assert_eq!(app.pipeline.dithered.version(), 4);
    }

    #[test]
    fn sync_clamps_after_layout_and_grid_changes() {
        let mut app = App::new(RgbaImage::new(120, 80), None);
        for (screen, scale, rotation, expected) in [
            (Rect::new(0, 0, 41, 9), 100, 0, (110, 70)),
            (Rect::new(0, 0, 100, 40), 100, 0, (51, 8)),
            (Rect::new(0, 0, 100, 40), 50, 0, (0, 0)),
            (Rect::new(0, 0, 100, 40), 100, 1, (11, 48)),
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
        app.selected = PARAMS.iter().position(|&p| p == Param::Strength).unwrap();
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
            &preview_geometry(&app, test_layout().preview),
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
        assert_eq!(PARAMS[app.selected], Param::Contrast);
        assert!(!press(&mut app, KeyCode::Char('x')));
    }

    #[test]
    fn numeric_params_clamp_without_redither() {
        type Getter = fn(&Params) -> f64;
        let cases: [(Param, Getter, f64, f64); 4] = [
            (Param::Scale, |p| f64::from(p.scale), 10.0, 200.0),
            (Param::Strength, |p| p.options.strength, 0.0, 1.0),
            (Param::Brightness, |p| p.options.tone.brightness, -0.5, 0.5),
            (Param::Contrast, |p| p.options.tone.contrast, -0.5, 1.0),
        ];
        for (param, get, low, high) in cases {
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
        assert_eq!((app.params.scale, app.params.rotation), (110, 1));
        app.selected = 2;
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('D'));
        let p = &app.params;
        assert_eq!((p.scale, p.rotation, p.palette), (100, 0, 0));
        assert_eq!(p.options.palette.len(), 2);
        assert_eq!(app.pipeline.dithered.get().unwrap().len(), 16);
    }

    #[test]
    fn reset_only_redithers_changed_values_and_preserves_pending_changes() {
        for param in PARAMS {
            let mut app = App::new(RgbaImage::new(20, 10), None);
            app.selected = PARAMS.iter().position(|&p| p == param).unwrap();
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
            &preview_geometry(&app, test_layout().preview),
        );
        app.handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &preview_geometry(&app, test_layout().preview),
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
        app.handle_key(release, &preview_geometry(&app, test_layout().preview));
        assert!(!app.quit);
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let version = (app.pipeline.grid.version(), app.pipeline.dithered.version());
        app.handle_key(ctrl('r'), &preview_geometry(&app, test_layout().preview));
        app.refresh();
        assert_eq!(
            (app.pipeline.grid.version(), app.pipeline.dithered.version()),
            version
        );
        app.handle_key(ctrl('c'), &preview_geometry(&app, test_layout().preview));
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
        assert_eq!(app.params.rotation, 1);
        press(&mut app, KeyCode::Char('R'));
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(app.params.rotation, 3);
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
            let geometry = preview_geometry(&app, area);
            let map = geometry.minimap.unwrap().map;
            app.view.render_minimap(
                app.displayed(),
                &geometry,
                &mut buffer,
                &mut Memo::default(),
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
                    &preview_geometry(&app, area),
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
        let map = preview_geometry(&app, area).minimap.unwrap().map;
        let mouse = |app: &mut App, kind, column, row| {
            let version = (app.pipeline.grid.version(), app.pipeline.dithered.version());
            app.handle_mouse(
                MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                },
                &preview_geometry(app, area),
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
                &preview_geometry(&app, test_layout().preview),
            );
        }
        assert!(app.quit);
        assert_eq!(app.pipeline.grid.version(), 1);
        app.finish_png().unwrap();
        assert_eq!(image::open(&path).unwrap().to_rgba8().dimensions(), (8, 16));
        std::fs::remove_file(path).unwrap();
    }
}
