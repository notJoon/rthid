use image::{RgbaImage, imageops};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use rthid::dither::{self, Mode, Options};

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
#[derive(Clone)]
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

pub struct App {
    pub source: RgbaImage,
    pub params: Params,
    pub selected: usize,
    pub indices: Vec<u8>,
    pub dirty: bool,
    pub quit: bool,
}

impl App {
    pub fn new(source: RgbaImage) -> Self {
        let mut app = Self {
            source,
            params: Params::default(),
            selected: 0,
            indices: Vec::new(),
            dirty: true,
            quit: false,
        };
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
    pub fn refresh(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let (w, h) = self.scaled_size();
        let scaled = imageops::resize(&self.source, w, h, imageops::FilterType::Triangle);
        let grid = match self.params.rotation {
            1 => imageops::rotate90(&scaled),
            2 => imageops::rotate180(&scaled),
            3 => imageops::rotate270(&scaled),
            _ => scaled,
        };
        let (gw, gh) = grid.dimensions();
        self.indices = dither::dither(
            grid.as_raw(),
            gw as usize,
            gh as usize,
            &self.params.options,
        );
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
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
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
        self.dirty = true;
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
        self.dirty = true;
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
        let changed = match PARAMS[self.selected] {
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

    fn app_at(param: Param) -> App {
        let mut app = App::new(RgbaImage::new(4, 4));
        app.selected = PARAMS.iter().position(|&p| p == param).unwrap();
        app
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
        assert_eq!(app.indices.len(), 16);
    }

    #[test]
    fn quit_keys_and_ctrl_chords() {
        for code in [KeyCode::Char('q'), KeyCode::Esc] {
            let mut app = app_at(Param::Scale);
            press(&mut app, code);
            assert!(app.quit);
        }
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
        let mut app = App::new(RgbaImage::new(256, 128));
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
        let mut app = App::new(RgbaImage::new(15, 1));
        for _ in 0..9 {
            press(&mut app, KeyCode::Left);
        }
        assert_eq!(app.grid_size(), (2, 1));
        assert_eq!(app.indices.len(), 2);
    }
}
