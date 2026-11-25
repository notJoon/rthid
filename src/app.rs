use crate::view::{Pixels, View};
use image::{RgbaImage, imageops};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use rthid::dither::{self, Mode, Options};
use rthid::png::{Job, Worker};
use std::path::PathBuf;

pub const PALETTES: [(&str, &[&str]); 3] = [
    ("Black/white", &["#000", "#fff"]),
    ("4-gray", &["#000", "#555", "#aaa", "#fff"]),
    ("Warm inks", &["#ffffff", "#ee6a45", "#a3300f", "#1c0d07"]),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Param {
    Scale,
    Mode,
    Palette,
    Strength,
    Serpentine,
    Brightness,
    Contrast,
}

// Panel order.
pub const PARAMS: [Param; 7] = [
    Param::Scale,
    Param::Mode,
    Param::Palette,
    Param::Strength,
    Param::Serpentine,
    Param::Brightness,
    Param::Contrast,
];

// Everything the user edits; `Default` is what the reset keys restore.
#[derive(Clone, PartialEq)]
pub struct Params {
    pub scale: i32,
    pub rotation: u8,
    pub palette: usize,
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

#[derive(Debug, PartialEq, Eq)]
pub struct GraphicsKey {
    pub generation: u64,
    pub x: u32,
    pub y: u32,
    pub exact: bool,
    pub area: Rect,
    pub font: (u16, u16),
}

pub const KITTY_IMAGE_ID: u32 = 0x727468;

pub struct App {
    pub graphics: Option<ratatui_image::picker::Picker>,
    pub graphics_cache: Option<(GraphicsKey, Rect, ratatui_image::protocol::StatefulProtocol)>,
    pub graphics_error: Option<String>,
    pub generation: u64,
    pub preview_png: Option<PathBuf>,
    pub png_error: Option<String>,
    png_worker: Option<Worker>,
    pub view: View,
    pub preview_focus: bool,
    pub preview_area: Rect,
    last_drag: Option<(u16, u16)>,
    pub source: RgbaImage,
    pub params: Params,
    pub parameter_list: ListState,
    pub indices: Vec<u8>,
    pub original: Vec<[u8; 3]>,
    pub show_original: bool,
    pub dirty: bool,
    pub quit: bool,
}

impl App {
    pub fn new(source: RgbaImage, preview_png: Option<PathBuf>) -> Self {
        let (png_worker, png_error) =
            match preview_png.as_ref().map(|path| Worker::new(path.clone())) {
                Some(Ok(worker)) => (Some(worker), None),
                Some(Err(error)) => (None, Some(error)),
                None => (None, None),
            };
        let mut app = Self {
            graphics: None,
            graphics_cache: None,
            graphics_error: None,
            generation: 0,
            preview_png,
            png_error: None,
            png_worker,
            view: View::default(),
            preview_focus: false,
            preview_area: Rect::default(),
            last_drag: None,
            source,
            params: Params::default(),
            parameter_list: ListState::default().with_selected(Some(0)),
            indices: Vec::new(),
            original: Vec::new(),
            show_original: true,
            dirty: true,
            quit: false,
        };
        app.set_png_error(png_error);
        app.refresh();
        app
    }

    // Source size after scaling, before rotation.
    fn scaled_size(&self) -> (u32, u32) {
        let scaled = |n| {
            (f64::from(n) * f64::from(self.params.scale) / 100.0)
                .round()
                .max(1.0) as u32
        };
        (scaled(self.source.width()), scaled(self.source.height()))
    }

    pub fn grid_size(&self) -> (u32, u32) {
        let (w, h) = self.scaled_size();
        if self.params.rotation.is_multiple_of(2) {
            (w, h)
        } else {
            (h, w)
        }
    }

    // Re-dithers once after any number of parameter changes.
    pub fn refresh(&mut self) -> bool {
        let mut status_changed = self.poll_png();
        if !self.dirty {
            return status_changed;
        }
        self.dirty = false;
        self.generation += 1;
        let (w, h) = self.scaled_size();
        let scaled = imageops::resize(&self.source, w, h, imageops::FilterType::Triangle);
        let grid = match self.params.rotation {
            1 => imageops::rotate90(&scaled),
            2 => imageops::rotate180(&scaled),
            3 => imageops::rotate270(&scaled),
            _ => scaled,
        };
        let (gw, gh) = grid.dimensions();
        self.original = grid
            .pixels()
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        self.indices = dither::dither(
            grid.as_raw(),
            gw as usize,
            gh as usize,
            &self.params.options,
        );
        if let Some(worker) = &mut self.png_worker {
            let submitted = worker.submit(Job {
                indices: self.indices.clone(),
                w: gw,
                h: gh,
                palette: self.params.options.palette.clone(),
            });
            if let Err(error) = submitted {
                status_changed |= self.set_png_error(Some(error));
            }
        }
        status_changed
    }

    pub fn displayed(&self) -> Pixels<'_> {
        if self.show_original {
            Pixels::Rgb(&self.original)
        } else {
            Pixels::Indexed {
                indices: &self.indices,
                palette: &self.params.options.palette,
            }
        }
    }

    fn set_png_error(&mut self, error: Option<std::io::Error>) -> bool {
        let error = error.map(|error| {
            format!(
                "PNG {}: {error}",
                self.preview_png
                    .as_ref()
                    .expect("PNG destination")
                    .display()
            )
        });
        let changed = self.png_error != error;
        self.png_error = error;
        changed
    }

    fn poll_png(&mut self) -> bool {
        if let Some(worker) = &self.png_worker
            && let Some(error) = worker.poll()
        {
            return self.set_png_error(error);
        }
        false
    }

    pub fn finish_png(&mut self) -> std::io::Result<()> {
        // A change and quit can arrive in the same input batch.
        self.refresh();
        if let Some(worker) = self.png_worker.take() {
            self.set_png_error(worker.finish().err());
        }
        match &self.png_error {
            Some(error) => Err(std::io::Error::other(error.clone())),
            None => Ok(()),
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let prev = self.last_drag.take();
        let point = (mouse.column, mouse.row);
        if !self.view.exact || !self.preview_area.contains(point.into()) {
            return;
        }
        let (w, h) = self.grid_size();
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((_, map)) = self.view.minimap(w, h, self.preview_area)
                    && map.contains(point.into())
                {
                    self.view
                        .center_on_minimap(point, w, h, self.preview_area, map);
                } else {
                    if matches!(mouse.kind, MouseEventKind::Drag(_))
                        && let Some((x, y)) = prev
                    {
                        self.view.pan(
                            i64::from(x) - i64::from(mouse.column),
                            (i64::from(y) - i64::from(mouse.row)) * 2,
                            w,
                            h,
                            self.preview_area,
                        );
                    }
                    self.last_drag = Some(point);
                }
            }
            MouseEventKind::ScrollUp => self.view.pan(0, -6, w, h, self.preview_area),
            MouseEventKind::ScrollDown => self.view.pan(0, 6, w, h, self.preview_area),
            // Trackpads send horizontal swipes as sideways scrolls.
            MouseEventKind::ScrollLeft => self.view.pan(-6, 0, w, h, self.preview_area),
            MouseEventKind::ScrollRight => self.view.pan(6, 0, w, h, self.preview_area),
            _ => {}
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            // Raw mode delivers Ctrl+C as a key; other Ctrl chords are not bound.
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
        // Esc only leaves preview focus; quitting is q or Ctrl+C so a stray Esc never exits.
        if key.code == KeyCode::Esc {
            self.preview_focus = false;
            return;
        }
        if self.preview_focus {
            let (w, h) = self.grid_size();
            let (vw, vh) = self.view.visible(w, h, self.preview_area);
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
                    self.view.pan(dx, dy, w, h, self.preview_area);
                }
                return;
            }
        }
        let selected = self.parameter_list.selected().unwrap_or(0);
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.parameter_list.select_previous(),
            KeyCode::Down | KeyCode::Char('j') => {
                self.parameter_list
                    .select(Some((selected + 1).min(PARAMS.len() - 1)));
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
        self.dirty = true;
    }

    // Restores the selected parameter, or every parameter and the rotation when `all` is set.
    fn reset(&mut self, all: bool) {
        let before = self.params.clone();
        let defaults = Params::default();
        let (p, d) = (&mut self.params, &defaults.options);
        match PARAMS[self.parameter_list.selected().unwrap_or(0)] {
            _ if all => *p = defaults.clone(),
            Param::Scale => p.scale = defaults.scale,
            Param::Mode => p.options.mode = d.mode,
            Param::Palette => p.set_palette(defaults.palette),
            Param::Strength => p.options.strength = d.strength,
            Param::Serpentine => p.options.serpentine = d.serpentine,
            Param::Brightness => p.options.tone.brightness = d.tone.brightness,
            Param::Contrast => p.options.tone.contrast = d.tone.contrast,
        }
        self.dirty |= self.params != before;
    }

    fn adjust(&mut self, direction: i32) {
        let p = &mut self.params;
        let o = &mut p.options;
        // Steps a float and reports whether it moved, so clamped presses skip the re-dither.
        let step = |value: &mut f64, steps: f64, low: f64, high: f64| {
            let next = (((*value * steps).round() + f64::from(direction)) / steps).clamp(low, high);
            let changed = next != *value;
            *value = next;
            changed
        };
        let param = PARAMS[self.parameter_list.selected().unwrap_or(0)];
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
        self.dirty |= changed;
        if changed && param != Param::Scale {
            self.show_original = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Returns whether the key asked for a re-dither, then performs it.
    fn press(app: &mut App, code: KeyCode) -> bool {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        let dirty = app.dirty;
        app.refresh();
        dirty
    }

    fn wait_for(app: &mut App, ready: impl Fn(&App) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.refresh();
            if ready(app) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "PNG completion timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn app_at(param: Param) -> App {
        let mut app = App::new(RgbaImage::new(4, 4), None);
        app.parameter_list
            .select(PARAMS.iter().position(|&p| p == param));
        app
    }

    #[test]
    fn graphics_defaults_to_fallback_and_generation_changes_only_on_refresh() {
        let mut app = App::new(RgbaImage::new(4, 4), None);
        assert!(app.graphics.is_none());
        assert!(app.graphics_cache.is_none());
        let generation = app.generation;
        app.refresh();
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        app.refresh();
        assert_eq!(app.generation, generation);
        app.dirty = true;
        app.refresh();
        assert_eq!(app.generation, generation + 1);
    }

    #[test]
    fn preview_toggle_and_adjustments_select_view_without_extra_redithering() {
        let mut app = app_at(Param::Strength);
        assert!(app.show_original);
        for focus in [false, true] {
            app.preview_focus = focus;
            assert!(!press(&mut app, KeyCode::Char(' ')));
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
        app.parameter_list.select(Some(0));
        assert!(press(&mut app, KeyCode::Left));
        assert!(app.show_original);
        for code in [KeyCode::Char('r'), KeyCode::Char('d'), KeyCode::Char('D')] {
            assert!(press(&mut app, code));
            assert!(app.show_original);
        }
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
        let Pixels::Rgb(pixels) = app.displayed() else {
            panic!("expected original RGB pixels");
        };
        assert_eq!(pixels, [[255, 0, 0], [25, 26, 128], [255; 3]]);
        assert_eq!(app.original, pixels);
    }

    #[test]
    fn navigation_stays_in_range_and_skips_redither() {
        let mut app = app_at(Param::Scale);
        assert!(!press(&mut app, KeyCode::Up));
        assert_eq!(app.parameter_list.selected(), Some(0));
        for _ in 0..10 {
            assert!(!press(&mut app, KeyCode::Char('j')));
        }
        assert_eq!(
            PARAMS[app.parameter_list.selected().unwrap()],
            Param::Contrast
        );
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
        app.parameter_list.select(Some(1));
        press(&mut app, KeyCode::Right);
        assert!(press(&mut app, KeyCode::Char('d')));
        assert!(matches!(app.params.options.mode, Mode::Tone));
        assert_eq!((app.params.scale, app.params.rotation), (110, 1));
        app.parameter_list.select(Some(2));
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('D'));
        let p = &app.params;
        assert_eq!((p.scale, p.rotation, p.palette), (100, 0, 0));
        assert_eq!(p.options.palette.len(), 2);
        assert_eq!(app.indices.len(), 16);
    }

    #[test]
    fn reset_only_redithers_changed_values_and_preserves_pending_changes() {
        for param in PARAMS {
            let mut app = app_at(param);
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
        assert!(press(&mut app, KeyCode::Char('D')));
        assert_eq!(app.params.options.tone.gamma, 1.0);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(app.dirty);
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
        app.handle_key(release);
        assert!(!app.quit);
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        app.handle_key(ctrl('r'));
        assert!(!app.dirty);
        app.handle_key(ctrl('c'));
        assert!(app.quit);
    }

    #[test]
    fn rotation_and_scale_set_the_actual_dither_grid() {
        let mut app = App::new(RgbaImage::new(256, 128), None);
        assert_eq!(app.grid_size(), (256, 128));
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.grid_size(), (128, 256));
        assert_eq!(app.indices.len(), 128 * 256);
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.grid_size(), (256, 128));
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(app.params.rotation, 1);
        press(&mut app, KeyCode::Char('R'));
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(app.params.rotation, 3);
        assert_eq!(app.grid_size(), (128, 256));
        let mut app = App::new(RgbaImage::new(15, 1), None);
        for _ in 0..9 {
            press(&mut app, KeyCode::Left);
        }
        assert_eq!(app.grid_size(), (2, 1));
        assert_eq!(app.indices.len(), 2);
    }
    #[test]
    fn tab_and_preview_keys_change_view_without_redithering() {
        let mut app = App::new(RgbaImage::new(40, 40), None);
        app.preview_area = ratatui::layout::Rect::new(0, 0, 10, 5);
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
        assert_eq!(app.parameter_list.selected(), Some(0));
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
            app.preview_area = Rect::new(2, 3, 48, 18);
            app.view.exact = true;
            app.view.glyphs = glyphs;
            let mut buffer = Buffer::empty(app.preview_area);
            let (_, map, _) = app
                .view
                .render_minimap(app.displayed(), 120, 120, app.preview_area, &mut buffer)
                .unwrap();
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
                app.handle_mouse(MouseEvent {
                    kind,
                    column,
                    row,
                    modifiers: KeyModifiers::NONE,
                });
                assert_eq!((app.view.x, app.view.y), expected, "{glyphs:?}");
                assert!(!app.dirty);
            }
        }
    }

    #[test]
    fn mouse_centers_pans_and_scrolls_without_focus_or_redithering() {
        let mut app = App::new(RgbaImage::new(120, 120), None);
        app.preview_area = Rect::new(2, 3, 48, 18);
        app.view.exact = true;
        let (_, map) = app.view.minimap(120, 120, app.preview_area).unwrap();
        let mouse = |app: &mut App, kind, column, row| {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: KeyModifiers::NONE,
            });
            assert!(!app.dirty);
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
        app.preview_area = Rect::new(2, 3, 10, 5);
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
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        assert!(app.quit);
        assert!(app.dirty);
        app.finish_png().unwrap();
        assert_eq!(image::open(&path).unwrap().to_rgba8().dimensions(), (8, 16));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn idle_refresh_requests_redraw_only_when_png_status_changes() {
        let path = std::env::temp_dir().join(format!("rthid-status-test-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let mut app = App::new(RgbaImage::new(4, 2), Some(path.clone()));
        let wait_for_change = |app: &mut App| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !app.refresh() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "PNG status change timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        wait_for_change(&mut app);
        assert!(
            app.png_error
                .as_ref()
                .unwrap()
                .starts_with(&format!("PNG {}: ", path.display()))
        );
        assert!(!app.refresh());
        std::fs::remove_dir(&path).unwrap();
        app.dirty = true;
        assert!(!app.refresh());
        wait_for_change(&mut app);
        assert!(app.png_error.is_none());
        assert!(!app.refresh());
        drop(app);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn refresh_keeps_write_error_until_background_success() {
        let dir = std::env::temp_dir().join(format!("rthid-error-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("preview.png");
        std::fs::create_dir(&path).unwrap();
        let mut app = App::new(RgbaImage::new(4, 2), Some(path.clone()));
        wait_for(&mut app, |app| app.png_error.is_some());
        std::fs::remove_dir(&path).unwrap();
        app.png_error = Some("previous write failed".into());
        app.dirty = true;
        app.refresh();
        assert_eq!(app.png_error.as_deref(), Some("previous write failed"));
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refresh_updates_companion_and_reports_write_errors() {
        let dir = std::env::temp_dir().join(format!("rthid-refresh-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("preview.png");
        let mut app = App::new(RgbaImage::new(4, 2), Some(path.clone()));
        wait_for(&mut app, |_| {
            image::open(&path).is_ok_and(|image| image.width() == 16)
        });
        press(&mut app, KeyCode::Char('r'));
        wait_for(&mut app, |_| {
            image::open(&path).is_ok_and(|image| image.width() == 8)
        });
        std::fs::remove_file(&path).unwrap();
        app.refresh();
        assert!(!path.exists());
        std::fs::create_dir(&path).unwrap();
        app.dirty = true;
        app.refresh();
        wait_for(&mut app, |app| app.png_error.is_some());
        app.refresh();
        assert!(app.png_error.is_some());
        std::fs::remove_dir(&path).unwrap();
        app.dirty = true;
        app.refresh();
        wait_for(&mut app, |app| app.png_error.is_none());
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
