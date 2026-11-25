//! Interactive state and the key and mouse handling that changes it.

use crate::{
    cells::Pixels,
    export::Export,
    grid::GridCache,
    memo::Memo,
    settings::{Setting, Settings},
    viewport::{PreviewGeometry, Viewport},
};
use image::RgbaImage;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use std::path::PathBuf;

/// All interactive state: settings, focus, preview viewport, grid caches and
/// the optional companion PNG.
pub struct App {
    export: Option<Export>,
    /// Preview mode (Overview or 1:1), glyphs, minimap and image origin.
    pub viewport: Viewport,
    /// Whether keys go to the preview first (toggled by Tab, left with Esc).
    pub preview_focus: bool,
    last_drag: Option<(u16, u16)>,
    /// Image processing caches.
    pub grid_cache: GridCache,
    /// Current image settings.
    pub settings: Settings,
    /// Index into `Setting::ALL` of the highlighted row.
    pub selected: usize,
    /// Whether the preview shows Original (the scaled grid) instead of Dithered.
    /// Starts true.
    pub show_original: bool,
    /// Whether the event loop should exit.
    pub quit: bool,
}

impl App {
    /// Starts the companion PNG writer when a path is given and builds the
    /// first grid. With a running writer it also dithers at once.
    pub fn new(source: RgbaImage, preview_png: Option<PathBuf>) -> Self {
        let mut app = Self {
            export: preview_png.map(Export::new),
            viewport: Viewport::default(),
            preview_focus: false,
            last_drag: None,
            grid_cache: GridCache {
                source,
                grid: Memo::default(),
                dithered: Memo::default(),
            },
            settings: Settings::default(),
            selected: 0,
            show_original: true,
            quit: false,
        };
        app.refresh();
        app
    }

    /// Grid size for the current settings after rotation, known before `sync`
    /// rebuilds the grid.
    pub fn grid_size(&self) -> (u32, u32) {
        let (w, h) = self.grid_cache.scaled_size(&self.settings);
        if self.settings.rotation.swaps_axes() {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// Rebuilds the grid and dither if settings changed, sends a new dither to the
    /// companion PNG and clamps the viewport to the preview area. Call before each draw.
    pub fn sync(&mut self, preview: Rect) {
        self.refresh();
        self.clamp_viewport(preview);
    }

    /// Coordinates shared by preview input and rendering.
    pub fn geometry(&self, area: Rect) -> PreviewGeometry {
        crate::viewport::geometry(&self.viewport, self.grid_size(), area)
    }

    /// Keeps the viewport within the current grid and preview area.
    pub fn clamp_viewport(&mut self, area: Rect) {
        let (w, h) = self.grid_size();
        self.viewport.clamp(w, h, area);
    }

    fn refresh(&mut self) {
        let need_dither = !self.show_original || self.has_export();
        if self.grid_cache.refresh(&self.settings, need_dither)
            && let Some(export) = &mut self.export
        {
            let (w, h) = self.grid_cache.grid().dimensions();
            export.sync(
                self.grid_cache.dithered().expect("dithered grid"),
                w,
                h,
                &self.settings.options.palette,
            );
        }
    }

    /// Whether a running companion PNG writer needs every new dither, even while
    /// Original is shown.
    pub fn has_export(&self) -> bool {
        self.export.as_ref().is_some_and(Export::needs_dither)
    }

    /// Collects finished PNG writes and reports whether the status line changed.
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
            Pixels::Rgb(self.grid_cache.grid(), self.grid_cache.grid_version())
        } else {
            Pixels::Indexed {
                indices: self.grid_cache.dithered().expect("dithered grid"),
                palette: &self.settings.options.palette,
            }
        }
    }

    /// Runs on quit. Sends any pending dither, waits for the PNG writer and
    /// returns its last error.
    pub fn finish_png(&mut self) -> std::io::Result<()> {
        // A change and quit can arrive in the same input batch.
        self.refresh();
        self.export.as_mut().map_or(Ok(()), Export::finish)
    }

    /// Handles the mouse only in 1:1 over the preview, regardless of focus. A left
    /// click or drag on the minimap recenters there, a drag elsewhere moves the
    /// image with the pointer, and the wheel pans six dots (grid pixels).
    pub fn handle_mouse(&mut self, mouse: MouseEvent, geometry: &PreviewGeometry) {
        let prev = self.last_drag.take();
        let point = (mouse.column, mouse.row);
        if !self.viewport.exact || !geometry.area.contains(point.into()) {
            return;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(minimap) = geometry.minimap
                    && minimap.map.contains(point.into())
                {
                    self.viewport.center_on_minimap(point, geometry);
                } else {
                    if matches!(mouse.kind, MouseEventKind::Drag(_))
                        && let Some((x, y)) = prev
                    {
                        self.viewport.pan(
                            i64::from(x) - i64::from(mouse.column),
                            (i64::from(y) - i64::from(mouse.row)) * 2,
                            geometry,
                        );
                    }
                    self.last_drag = Some(point);
                }
            }
            MouseEventKind::ScrollUp => self.viewport.pan(0, -6, geometry),
            MouseEventKind::ScrollDown => self.viewport.pan(0, 6, geometry),
            // Trackpads send horizontal swipes as sideways scrolls.
            MouseEventKind::ScrollLeft => self.viewport.pan(-6, 0, geometry),
            MouseEventKind::ScrollRight => self.viewport.pan(6, 0, geometry),
            _ => {}
        }
    }

    // TODO: Introduce an Action enum with a pure key mapping once more bindings arrive.
    /// Global keys come first (Ctrl+C, Tab, Space, Esc). With preview focus, m, z
    /// and the pan keys act on the viewport, and pans do nothing in Overview. All other
    /// keys fall through to the quit, row, rotation, reset and adjust keys.
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
                    self.viewport.hide_minimap = !self.viewport.hide_minimap;
                    return;
                }
                KeyCode::Char('z') => {
                    self.viewport.exact = !self.viewport.exact;
                    self.last_drag = None;
                    return;
                }
                _ => {}
            }
            let pan = pan_delta(key.code, geometry.visible);
            if let Some((dx, dy)) = pan {
                if self.viewport.exact {
                    self.viewport.pan(dx, dy, geometry);
                }
                return;
            }
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(Setting::ALL.len() - 1);
            }
            KeyCode::Char('r') => self.settings.rotation = self.settings.rotation.cw(),
            KeyCode::Char('R') => self.settings.rotation = self.settings.rotation.ccw(),
            KeyCode::Char('d') => {
                self.settings = self.settings.with_default(Setting::ALL[self.selected])
            }
            KeyCode::Char('D') => self.settings = Settings::default(),
            KeyCode::Left | KeyCode::Char('h') => self.adjust(-1),
            KeyCode::Right | KeyCode::Char('l') => self.adjust(1),
            _ => {}
        }
    }

    fn adjust(&mut self, direction: i32) {
        let setting = Setting::ALL[self.selected];
        let next = self.settings.adjusted(setting, direction);
        // Clamped presses preserve the current preview selection.
        if next != self.settings && setting.shows_dithered() {
            self.show_original = false;
        }
        self.settings = next;
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
    use crate::{settings::Rotation, ui::Layout};
    use ratatui::layout::Rect;
    use rthid::dither::Mode;

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

    fn versions(app: &App) -> (u64, u64) {
        (
            app.grid_cache.grid.version(),
            app.grid_cache.dithered.version(),
        )
    }

    // Compares build versions after applying the key.
    fn press(app: &mut App, code: KeyCode) -> bool {
        let before = versions(app);
        app.handle_key(
            KeyEvent::new(code, KeyModifiers::NONE),
            &app.geometry(test_layout().preview),
        );
        app.sync(test_layout().preview);
        before != versions(app)
    }

    fn app_at(setting: Setting) -> App {
        let mut app = App::new(RgbaImage::new(4, 4), None);
        app.selected = Setting::ALL.iter().position(|&p| p == setting).unwrap();
        app
    }

    #[test]
    fn grid_cache_rebuilds_only_stale_stages() {
        let mut app = App::new(RgbaImage::new(20, 10), None);
        assert_eq!(app.grid_cache.grid.version(), 1);
        assert_eq!(app.grid_cache.dithered.version(), 0);
        app.show_original = false;
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.dithered.version(), 1);
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.grid.version(), 1);
        assert_eq!(app.grid_cache.dithered.version(), 1);
        app.settings.options.strength = 0.5;
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.grid.version(), 1);
        assert_eq!(app.grid_cache.dithered.version(), 2);
        app.show_original = true;
        app.settings.rotation = Rotation::R180;
        app.settings.scale = 110;
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.grid.version(), 2);
        assert_eq!(app.grid_cache.dithered.version(), 2);
        app.show_original = false;
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.dithered.version(), 3);
        assert_eq!(app.grid_size(), (22, 11));
        let (w, h) = app.grid_size();
        assert_eq!(
            app.grid_cache.dithered.get().unwrap().len(),
            (w * h) as usize
        );
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
            app.settings.scale = scale;
            app.settings.rotation = rotation;
            app.viewport.x = 999;
            app.viewport.y = 999;
            app.sync(crate::ui::layout(screen).preview);
            assert_eq!((app.viewport.x, app.viewport.y), expected);
            assert_eq!(
                app.grid_cache.grid.get().unwrap().dimensions(),
                app.grid_size()
            );
        }
    }

    #[test]
    fn preview_toggle_and_adjustments_select_view_without_extra_redithering() {
        let mut app = App::new(RgbaImage::new(20, 10), None);
        app.selected = Setting::ALL
            .iter()
            .position(|&p| p == Setting::Strength)
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
    fn original_preserves_rgb_and_alpha() {
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
        assert_eq!(app.grid_cache.grid.get().unwrap(), pixels);
    }

    #[test]
    fn navigation_stays_in_range_and_skips_redither() {
        let mut app = app_at(Setting::Scale);
        assert!(!press(&mut app, KeyCode::Up));
        assert_eq!(app.selected, 0);
        for _ in 0..10 {
            assert!(!press(&mut app, KeyCode::Char('j')));
        }
        assert_eq!(Setting::ALL[app.selected], Setting::Contrast);
        assert!(!press(&mut app, KeyCode::Char('x')));
    }

    #[test]
    fn numeric_settings_clamp_without_redither() {
        type Getter = fn(&Settings) -> f64;
        let cases: [(Setting, Getter); 4] = [
            (Setting::Scale, |p| f64::from(p.scale)),
            (Setting::Strength, |p| p.options.strength),
            (Setting::Brightness, |p| p.options.tone.brightness),
            (Setting::Contrast, |p| p.options.tone.contrast),
        ];
        for (setting, get) in cases {
            let (_, low, high) = setting.range().unwrap();
            let mut app = app_at(setting);
            for (key, limit) in [(KeyCode::Left, low), (KeyCode::Right, high)] {
                for _ in 0..50 {
                    press(&mut app, key);
                }
                assert_eq!(get(&app.settings), limit, "{setting:?}");
                assert!(
                    !press(&mut app, key),
                    "{setting:?} re-dithered at its limit"
                );
            }
        }
    }

    #[test]
    fn toggles_and_palette_cycle() {
        let mut app = app_at(Setting::Mode);
        assert!(press(&mut app, KeyCode::Char('l')));
        assert!(matches!(app.settings.options.mode, Mode::Color));
        let mut app = app_at(Setting::Serpentine);
        press(&mut app, KeyCode::Right);
        assert!(!app.settings.options.serpentine);
        let mut app = app_at(Setting::Palette);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.settings.palette, 2);
        assert_eq!(app.settings.options.palette.len(), 4);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.settings.palette, 0);
        assert_eq!(app.settings.options.palette.len(), 2);
    }

    #[test]
    fn reset_restores_selected_or_all_defaults() {
        let mut app = app_at(Setting::Scale);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('r'));
        app.selected = 1;
        press(&mut app, KeyCode::Right);
        assert!(press(&mut app, KeyCode::Char('d')));
        assert!(matches!(app.settings.options.mode, Mode::Tone));
        assert_eq!(
            (app.settings.scale, app.settings.rotation),
            (110, Rotation::R90)
        );
        app.selected = 2;
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('D'));
        let p = &app.settings;
        assert_eq!((p.scale, p.rotation, p.palette), (100, Rotation::R0, 0));
        assert_eq!(p.options.palette.len(), 2);
        assert_eq!(app.grid_cache.dithered.get().unwrap().len(), 16);
    }

    #[test]
    fn reset_only_redithers_changed_values_and_preserves_pending_changes() {
        for setting in Setting::ALL {
            let mut app = App::new(RgbaImage::new(20, 10), None);
            app.selected = Setting::ALL.iter().position(|&p| p == setting).unwrap();
            assert!(!press(&mut app, KeyCode::Char('d')), "{setting:?}");
            assert!(!press(&mut app, KeyCode::Char('D')), "{setting:?}");
            assert!(press(&mut app, KeyCode::Left));
            assert!(press(&mut app, KeyCode::Char('d')), "{setting:?}");
            assert!(!press(&mut app, KeyCode::Char('d')), "{setting:?}");
            assert!(press(&mut app, KeyCode::Left));
            assert!(press(&mut app, KeyCode::Char('D')), "{setting:?}");
            assert!(!press(&mut app, KeyCode::Char('D')), "{setting:?}");
        }
        let mut app = app_at(Setting::Scale);
        press(&mut app, KeyCode::Char('r'));
        assert!(!press(&mut app, KeyCode::Char('d')));
        assert!(press(&mut app, KeyCode::Char('D')));
        app.settings.options.tone.gamma = 2.0;
        app.show_original = false;
        app.sync(test_layout().preview);
        assert!(press(&mut app, KeyCode::Char('D')));
        assert_eq!(app.settings.options.tone.gamma, 1.0);
        app.handle_key(
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &app.geometry(test_layout().preview),
        );
        app.handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &app.geometry(test_layout().preview),
        );
        let version = versions(&app);
        app.refresh();
        assert_eq!(versions(&app), version);
    }

    #[test]
    fn quit_keys_and_ctrl_chords() {
        let mut app = app_at(Setting::Scale);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Esc);
        assert!(!app.preview_focus);
        press(&mut app, KeyCode::Esc);
        assert!(!app.quit);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
        let mut app = app_at(Setting::Scale);
        let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        app.handle_key(release, &app.geometry(test_layout().preview));
        assert!(!app.quit);
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let version = versions(&app);
        app.handle_key(ctrl('r'), &app.geometry(test_layout().preview));
        app.refresh();
        assert_eq!(versions(&app), version);
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
        assert_eq!(app.grid_cache.dithered.get().unwrap().len(), 128 * 256);
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.grid_size(), (256, 128));
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(app.settings.rotation, Rotation::R90);
        press(&mut app, KeyCode::Char('R'));
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(app.settings.rotation, Rotation::R270);
        assert_eq!(app.grid_size(), (128, 256));
        let mut app = App::new(RgbaImage::new(15, 1), None);
        app.show_original = false;
        for _ in 0..9 {
            press(&mut app, KeyCode::Left);
        }
        assert_eq!(app.grid_size(), (2, 1));
        assert_eq!(app.grid_cache.dithered.get().unwrap().len(), 2);
    }
    #[test]
    fn tab_and_preview_keys_change_viewport_without_redithering() {
        let mut app = App::new(RgbaImage::new(40, 40), None);
        assert!(!press(&mut app, KeyCode::Char('z')));
        assert!(!app.viewport.exact);
        assert!(!press(&mut app, KeyCode::Tab));
        assert!(app.preview_focus);
        assert!(!press(&mut app, KeyCode::Char('z')));
        assert!(app.viewport.exact);
        assert!(!press(&mut app, KeyCode::Char('L')));
        assert_eq!(app.viewport.x, 8);
        assert!(!press(&mut app, KeyCode::Right));
        assert_eq!((app.viewport.x, app.viewport.y), (9, 0));
        assert_eq!(app.selected, 0);
        for _ in 0..100 {
            press(&mut app, KeyCode::Right);
            press(&mut app, KeyCode::Down);
        }
        assert_eq!((app.viewport.x, app.viewport.y), (30, 30));
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Up);
        assert_eq!((app.viewport.x, app.viewport.y), (29, 29));
        press(&mut app, KeyCode::Char('z'));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.viewport.x, 29);
        press(&mut app, KeyCode::Tab);
        assert!(press(&mut app, KeyCode::Right));
        assert_eq!(app.settings.scale, 110);
    }

    #[test]
    fn mouse_centers_pans_and_scrolls_without_focus_or_redithering() {
        let mut app = App::new(RgbaImage::new(120, 120), None);
        let area = Rect::new(2, 3, 48, 18);
        app.viewport.exact = true;
        let map = app.geometry(area).minimap.unwrap().map;
        let mouse = |app: &mut App, kind, column, row| {
            let version = versions(app);
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
            assert_eq!(versions(app), version);
            assert!(!app.preview_focus);
        };
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            map.x + 4,
            map.y + 1,
        );
        assert_eq!((app.viewport.x, app.viewport.y), (36, 27));
        mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            map.right() - 1,
            map.bottom() - 1,
        );
        assert_eq!((app.viewport.x, app.viewport.y), (72, 84));
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 10, 10);
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 10, 10);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 13, 12);
        assert_eq!((app.viewport.x, app.viewport.y), (69, 80));
        mouse(&mut app, MouseEventKind::ScrollUp, 13, 12);
        assert_eq!(app.viewport.y, 74);
        mouse(&mut app, MouseEventKind::ScrollDown, 13, 12);
        assert_eq!(app.viewport.y, 80);
        mouse(&mut app, MouseEventKind::ScrollLeft, 13, 12);
        assert_eq!(app.viewport.x, 63);
        mouse(&mut app, MouseEventKind::ScrollRight, 13, 12);
        assert_eq!(app.viewport.x, 69);
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Right),
            map.x,
            map.y,
        );
        mouse(&mut app, MouseEventKind::ScrollDown, 0, 0);
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 14, 13);
        assert_eq!((app.viewport.x, app.viewport.y), (69, 80));
        app.viewport.exact = false;
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            map.x,
            map.y,
        );
        mouse(&mut app, MouseEventKind::ScrollDown, 13, 12);
        assert_eq!((app.viewport.x, app.viewport.y), (69, 80));
        app.viewport.exact = true;
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            map.right() - 1,
            map.bottom() - 1,
        );
        mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            map.x,
            map.y,
        );
        assert_eq!((app.viewport.x, app.viewport.y), (0, 0));
    }

    #[test]
    fn minimap_toggle_changes_visibility_without_redithering() {
        let mut app = App::new(RgbaImage::new(120, 120), None);
        app.preview_focus = true;
        app.viewport.exact = true;
        assert!(press(&mut app, KeyCode::Char(' ')));
        for hidden in [true, false] {
            assert!(!press(&mut app, KeyCode::Char('m')));
            assert_eq!(app.viewport.hide_minimap, hidden);
        }
    }

    #[test]
    fn finish_png_exports_changes_queued_with_quit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.png");
        let mut app = App::new(RgbaImage::new(4, 2), Some(path.clone()));
        for code in [KeyCode::Char('r'), KeyCode::Char('q')] {
            app.handle_key(
                KeyEvent::new(code, KeyModifiers::NONE),
                &app.geometry(test_layout().preview),
            );
        }
        assert!(app.quit);
        assert_eq!(app.grid_cache.grid.version(), 1);
        app.finish_png().unwrap();
        assert_eq!(image::open(&path).unwrap().to_rgba8().dimensions(), (8, 16));
    }
}
