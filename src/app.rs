//! Interactive state and the key and mouse handling that changes it.

use crate::{
    cells::{Glyphs, Pixels},
    export::Export,
    grid::GridCache,
    memo::Memo,
    modal::{Ctx, Effect, Modal, Next, Step},
    settings::{Palettes, Rotation, Setting, Settings, Template, Templates},
    store::{Change, Loaded},
    viewport::{PreviewGeometry, Viewport, Zoom},
};
use image::RgbaImage;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use rthid_io::{
    dir::Entry,
    io::{Done, Job, Opened, Outcome as IoOutcome},
};
use rthid_widgets::{ListPicker, Outcome};
use std::path::PathBuf;

/// All interactive state: settings, focus, preview viewport, grid caches and
/// the optional companion PNG.
pub struct App {
    export: Option<Export>,
    source_path: Option<PathBuf>,
    notice: Option<String>,
    save_blocked: Option<String>,
    outbox: Vec<Job>,
    pending: Vec<Change>,
    /// Active dialog, which receives input before global bindings.
    pub modal: Option<Modal>,
    /// Saved settings with independent palette colors.
    pub templates: Templates,
    /// Glyph set for sampled Dithered cells and the minimap.
    pub glyphs: Glyphs,
    /// Preview zoom, minimap and dot origin.
    pub viewport: Viewport,
    /// Whether keys go to the preview first (toggled by Tab, left with Esc).
    pub preview_focus: bool,
    last_drag: Option<((u16, u16), (u32, u32))>,
    /// Image processing caches.
    pub grid_cache: GridCache,
    /// Built-in and user palette presets in selection order.
    pub palettes: Palettes,
    /// Current image settings.
    pub settings: Settings,
    /// Parameter selection and retained scroll offset.
    pub panel: ListPicker,
    /// Whether the preview shows Original (the scaled grid) instead of Dithered.
    /// Starts true.
    pub show_original: bool,
    /// Whether the event loop should exit.
    pub quit: bool,
}

impl App {
    /// Builds the source grid and queues a companion PNG when requested.
    pub fn new(
        source: Option<(PathBuf, RgbaImage)>,
        preview_png: Option<PathBuf>,
        palettes: Palettes,
        templates: Templates,
        notice: Option<String>,
        save_blocked: Option<String>,
    ) -> Self {
        let mut app = Self {
            export: preview_png.map(Export::new),
            source_path: None,
            glyphs: Glyphs::default(),
            viewport: Viewport::default(),
            preview_focus: false,
            last_drag: None,
            grid_cache: GridCache {
                source: None,
                grid: Memo::default(),
                dithered: Memo::default(),
            },
            palettes,
            templates,
            notice: None,
            save_blocked,
            outbox: Vec::new(),
            pending: Vec::new(),
            modal: None,
            settings: Settings::default(),
            panel: ListPicker::new(0),
            show_original: true,
            quit: false,
        };
        if let Some((path, image)) = source {
            app.open_image(path, image);
        }
        app.notice = notice;
        app
    }

    /// Builds an app with built-ins and no configuration diagnostics for tests.
    #[cfg(test)]
    pub fn builtin(source: RgbaImage, preview_png: Option<PathBuf>) -> Self {
        Self::new(
            Some((PathBuf::from("test.png"), source)),
            preview_png,
            Palettes::builtin(),
            Templates::default(),
            None,
            None,
        )
    }

    /// Builds an app without an image for tests.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self::new(
            None,
            None,
            Palettes::builtin(),
            Templates::default(),
            None,
            None,
        )
    }

    /// Path of the loaded source, if any.
    pub fn source_path(&self) -> Option<&std::path::Path> {
        self.source_path.as_deref()
    }

    /// Replaces the source and resets preview navigation while retaining settings.
    pub fn open_image(&mut self, path: PathBuf, image: RgbaImage) {
        self.notice = None;
        if matches!(self.modal, Some(Modal::OpenFile(_))) {
            self.modal = None;
        }
        self.source_path = Some(path);
        self.grid_cache.set_source(image);
        self.viewport = Viewport::default();
        self.last_drag = None;
        self.settings.rotation = Rotation::R0;
        self.show_original = true;
        self.refresh();
    }

    /// Supplies directory entries to the open dialog if it is still open.
    pub fn set_listing(&mut self, dir: PathBuf, entries: Vec<Entry>) {
        if let Some(Modal::OpenFile(picker)) = &mut self.modal {
            picker.set_listing(dir, entries);
        }
        self.notice = None;
    }

    /// Current configuration diagnostic for the status line.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Reports a validation or configuration error and whether the status changed.
    pub fn set_notice(&mut self, notice: String) -> bool {
        let changed = self.notice.as_ref() != Some(&notice);
        self.notice = Some(notice);
        changed
    }

    /// Drains pending edits unless configuration loading blocked writes.
    pub fn take_changes(&mut self) -> Vec<Change> {
        if self.save_blocked.is_some() {
            Vec::new()
        } else {
            std::mem::take(&mut self.pending)
        }
    }

    /// Replaces saved lists while retaining the current palette selection by name.
    pub fn saved(&mut self, loaded: Loaded) {
        self.notice = loaded.notice();
        self.palettes = loaded.palettes;
        self.templates = loaded.templates;
        self.save_blocked = loaded.save_blocked;
    }

    /// Restores failed edits before newer ones and reports the save error.
    pub fn save_failed(&mut self, mut changes: Vec<Change>, error: String) {
        changes.append(&mut self.pending);
        self.pending = changes;
        self.notice = Some(format!("config: {error}"));
    }

    /// Drains filesystem jobs for submission after sync.
    pub fn take_jobs(&mut self) -> Vec<Job> {
        std::mem::take(&mut self.outbox)
    }

    /// Applies a finished job and reports whether the display changed.
    pub fn complete(&mut self, done: Done) -> bool {
        let mut changed = false;
        if self
            .notice
            .as_deref()
            .is_some_and(|notice| notice.starts_with("Loading "))
        {
            self.notice = None;
            changed = true;
        }
        match done.outcome {
            IoOutcome::Opened(Ok(Opened::Image { path, image })) => {
                self.open_image(path, image);
                true
            }
            IoOutcome::Opened(Ok(Opened::Dir(listing))) => {
                if !matches!(self.modal, Some(Modal::OpenFile(_))) {
                    self.modal = Some(Modal::open_file(listing.dir.clone()));
                }
                self.set_listing(listing.dir, listing.entries);
                true
            }
            IoOutcome::Listed(Ok(listing)) => {
                if matches!(self.modal, Some(Modal::OpenFile(_))) {
                    self.set_listing(listing.dir, listing.entries);
                    changed = true;
                }
                changed
            }
            IoOutcome::Exported(result) => {
                if let Some(export) = &mut self.export {
                    if result.is_ok()
                        && self
                            .notice
                            .as_deref()
                            .is_some_and(|notice| Some(notice) == export.status())
                    {
                        self.notice = None;
                        changed = true;
                    }
                    changed |= export.complete(result.as_ref().map(|_| ()));
                }
                if let Err(error) = result {
                    changed |= self.set_notice(error.to_string());
                }
                changed
            }
            IoOutcome::Opened(Err(error)) | IoOutcome::Listed(Err(error)) => {
                self.set_notice(error.to_string()) | changed
            }
        }
    }

    /// Queues an image load and shows its file name while loading.
    pub fn request_open(&mut self, path: PathBuf, dir: Option<PathBuf>) {
        self.set_notice(format!(
            "Loading {}",
            path.file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
        ));
        self.outbox.push(Job::Open {
            path,
            dir,
            protect: self
                .export
                .as_ref()
                .map(|export| export.path().to_path_buf()),
        });
    }

    fn request_list(&mut self, path: PathBuf) {
        self.outbox.push(Job::List(path));
    }

    /// Inserts pasted text into the active field or requests a dropped image.
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(modal) = &mut self.modal {
            if let Modal::OpenFile(picker) = modal
                && let Some(path) = rthid_io::paste::dropped_path(text)
            {
                picker.input = rthid_widgets::TextInput::new(path.to_string_lossy(), usize::MAX);
                picker.typed = true;
                return;
            }
            if modal.text_input_mut().is_some() {
                for c in text.chars().filter(|c| !c.is_control()) {
                    modal.handle_key(
                        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                        Ctx {
                            palettes: &self.palettes,
                            templates: &self.templates,
                            settings: &self.settings,
                            show_original: self.show_original,
                        },
                    );
                }
            }
        } else if let Some(path) = rthid_io::paste::dropped_path(text) {
            self.request_open(path, None);
        }
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
    /// Returns whether the grid or dither was rebuilt.
    pub fn sync(&mut self, preview: Rect) -> bool {
        let before = (
            self.grid_cache.grid.version(),
            self.grid_cache.dithered.version(),
        );
        self.refresh();
        self.clamp_viewport(preview);
        before
            != (
                self.grid_cache.grid.version(),
                self.grid_cache.dithered.version(),
            )
    }

    /// Coordinates shared by preview input and rendering.
    pub fn geometry(&self, area: Rect) -> PreviewGeometry {
        crate::viewport::geometry(&self.viewport, self.grid_size(), area)
    }

    /// Keeps the viewport within the current grid and preview area.
    pub fn clamp_viewport(&mut self, area: Rect) {
        if !self.grid_cache.has_source() {
            return;
        }
        let (w, h) = self.grid_size();
        self.viewport.clamp(w, h, area);
    }

    fn refresh(&mut self) {
        let need_dither = !self.show_original || self.has_export();
        if self.grid_cache.refresh(&self.settings, need_dither)
            && let Some(export) = &mut self.export
        {
            let (w, h) = self.grid_cache.grid().expect("processed grid").dimensions();
            self.outbox.push(export.job(
                self.grid_cache.dithered().expect("dithered grid"),
                w,
                h,
                &self.settings.options.palette,
            ));
        }
    }

    /// Whether companion PNG output needs every new dither while Original is shown.
    pub fn has_export(&self) -> bool {
        self.export.is_some()
    }

    /// Latest companion PNG error for the status line.
    pub fn export_status(&self) -> Option<&str> {
        self.export.as_ref().and_then(Export::status)
    }

    /// Selected preview pixels after sync, or None without a source.
    ///
    /// Panics in dithered mode if its result was never built.
    pub fn displayed(&self) -> Option<Pixels<'_>> {
        let grid = self.grid_cache.grid()?;
        Some(if self.show_original {
            Pixels::Rgb(grid, self.grid_cache.grid_version())
        } else {
            Pixels::Indexed {
                indices: self.grid_cache.dithered().expect("dithered grid"),
                palette: &self.settings.options.palette,
            }
        })
    }

    /// Queues the final export including changes received with quit.
    pub fn finish(&mut self) {
        self.refresh();
    }

    /// Returns the last companion PNG write result.
    pub fn export_result(&self) -> std::io::Result<()> {
        self.export.as_ref().map_or(Ok(()), Export::result)
    }

    /// Routes modal mouse input or pans and recenters the preview.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, geometry: &PreviewGeometry, screen: Rect) {
        if let Some(modal) = &mut self.modal {
            self.last_drag = None;
            let ctx = Ctx {
                palettes: &self.palettes,
                templates: &self.templates,
                settings: &self.settings,
                show_original: self.show_original,
            };
            let layout = modal.layout(geometry.area, screen, &ctx);
            let step = modal.handle_mouse(mouse, &layout, ctx);
            self.apply_step(step);
            return;
        }
        if self.source_path().is_none() {
            self.last_drag = None;
            return;
        }
        let prev = self.last_drag.take();
        let point = (mouse.column, mouse.row);
        if !geometry.overflows() || !geometry.area.contains(point.into()) {
            return;
        }
        let (dx, dy) = geometry.zoom.dots_per_cell(geometry.grid, geometry.area);
        let (sx, sy) = ((dx * 6.0).round() as i64, (dy * 6.0).round() as i64);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(minimap) = geometry.minimap
                    && minimap.map.contains(point.into())
                {
                    self.viewport.center_on_minimap(point, geometry);
                } else {
                    if matches!(mouse.kind, MouseEventKind::Drag(_))
                        && let Some(((x, y), origin)) = prev
                    {
                        let anchored = PreviewGeometry {
                            origin,
                            ..*geometry
                        };
                        self.viewport.pan(
                            (f64::from(i32::from(x) - i32::from(mouse.column)) * dx).round() as i64,
                            (f64::from(i32::from(y) - i32::from(mouse.row)) * dy).round() as i64,
                            &anchored,
                        );
                    }
                    self.last_drag = Some(prev.unwrap_or((point, geometry.origin)));
                }
            }
            MouseEventKind::ScrollUp => self.viewport.pan(0, -sy, geometry),
            MouseEventKind::ScrollDown => self.viewport.pan(0, sy, geometry),
            // Trackpads send horizontal swipes as sideways scrolls.
            MouseEventKind::ScrollLeft => self.viewport.pan(-sx, 0, geometry),
            MouseEventKind::ScrollRight => self.viewport.pan(sx, 0, geometry),
            _ => {}
        }
    }

    /// Borrows the preview and saved entries needed by dialogs.
    pub fn modal_ctx(&self) -> Ctx<'_> {
        Ctx {
            palettes: &self.palettes,
            templates: &self.templates,
            settings: &self.settings,
            show_original: self.show_original,
        }
    }

    fn apply_step(&mut self, step: Step) {
        if step.effect.is_some() {
            self.notice = None;
        }
        if let Some(effect) = step.effect
            && let Err(error) = self.apply(effect)
        {
            self.set_notice(error);
            return;
        }
        match step.next {
            Next::Stay => {}
            Next::Close => self.modal = None,
            Next::Replace(modal) => self.modal = Some(modal),
        }
    }

    /// Handles global zoom and focus keys, then preview panning or parameter edits.
    pub fn handle_key(&mut self, key: KeyEvent, geometry: &PreviewGeometry) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        self.notice = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return;
        }
        // Borrowing fields one by one lets the modal stay in place while it reads the rest of App.
        if let Some(modal) = &mut self.modal {
            let step = modal.handle_key(
                key,
                Ctx {
                    palettes: &self.palettes,
                    templates: &self.templates,
                    settings: &self.settings,
                    show_original: self.show_original,
                },
            );
            self.apply_step(step);
            return;
        }
        if key.code == KeyCode::Char('o') {
            let start = self
                .source_path()
                .and_then(|path| path.parent())
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf();
            self.last_drag = None;
            self.modal = Some(Modal::open_file(start.clone()));
            self.request_list(start);
            return;
        }
        if key.code == KeyCode::Char('P') {
            self.last_drag = None;
            self.modal = Some(Modal::presets(&self.settings, &self.palettes));
            return;
        }
        if key.code == KeyCode::Char('T') {
            self.last_drag = None;
            self.modal = Some(Modal::templates());
            return;
        }
        if key.code == KeyCode::Tab {
            self.preview_focus = !self.preview_focus;
            return;
        }
        if self.source_path().is_none()
            && (matches!(key.code, KeyCode::Char(' ' | 'm' | '+' | '=' | '-' | '0'))
                || (self.preview_focus
                    && pan_delta(key.code, geometry.visible, geometry.pan_step()).is_some()))
        {
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
        match key.code {
            KeyCode::Char('m') => {
                self.viewport.hide_minimap = !self.viewport.hide_minimap;
                return;
            }
            KeyCode::Char('+' | '=' | '-' | '0') => {
                let zoom = if key.code == KeyCode::Char('0') {
                    Zoom::Fit
                } else {
                    self.viewport.zoom.step(
                        key.code != KeyCode::Char('-'),
                        geometry.grid,
                        geometry.area,
                    )
                };
                self.viewport.set_zoom(zoom, geometry);
                self.last_drag = None;
                return;
            }
            _ => {}
        }
        if self.preview_focus
            && let Some((dx, dy)) = pan_delta(key.code, geometry.visible, geometry.pan_step())
        {
            self.viewport.pan(dx, dy, geometry);
            return;
        }
        match self.panel.handle_key(key, Setting::ALL.len()) {
            Outcome::Ignored => {}
            Outcome::Consumed | Outcome::Submit(_) | Outcome::Cancel => return,
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('r') => self.settings.rotation = self.settings.rotation.cw(),
            KeyCode::Char('R') => self.settings.rotation = self.settings.rotation.ccw(),
            KeyCode::Char('d') => {
                self.settings = self
                    .settings
                    .with_default(Setting::ALL[self.panel.selected()], &self.palettes)
            }
            KeyCode::Char('D') => self.settings = Settings::default(),
            KeyCode::Left | KeyCode::Char('h') => self.adjust(-1),
            KeyCode::Right | KeyCode::Char('l') => self.adjust(1),
            _ => {}
        }
    }

    fn apply(&mut self, effect: Effect) -> Result<(), String> {
        match effect {
            Effect::Open(path, dir) => self.request_open(path, Some(dir)),
            Effect::ListDir(path) => self.request_list(path),
            Effect::Notice(message) => {
                self.set_notice(message);
            }
            Effect::Preview(settings) => {
                self.settings = settings;
                self.show_original = false;
            }
            Effect::Restore {
                settings,
                show_original,
            } => {
                self.settings = settings;
                self.show_original = show_original;
            }
            Effect::SavePreset { preset, base } => {
                let index = self.palettes.upsert(preset.clone())?;
                self.apply(Effect::Preview(
                    base.with_preset(self.palettes.get(index).unwrap()),
                ))?;
                self.pending.push(Change::UpsertPreset(preset));
            }
            Effect::RemovePreset(index) => {
                let name = self
                    .palettes
                    .get(index)
                    .ok_or("preset does not exist")?
                    .name
                    .clone();
                self.palettes.remove(index)?;
                self.pending.push(Change::RemovePreset(name));
            }
            Effect::SaveTemplate(name) => {
                let template = Template::capture(name, &self.settings);
                self.templates.upsert(template.clone())?;
                self.pending.push(Change::UpsertTemplate(template));
            }
            Effect::RemoveTemplate(index) => {
                if let Some(template) = self.templates.get(index) {
                    let name = template.name.clone();
                    self.templates.remove(index);
                    self.pending.push(Change::RemoveTemplate(name));
                }
            }
        }
        Ok(())
    }

    fn adjust(&mut self, direction: i32) {
        let setting = Setting::ALL[self.panel.selected()];
        let next = self.settings.adjusted(setting, direction, &self.palettes);
        // Clamped presses preserve the current preview selection.
        if next != self.settings && setting.shows_dithered() {
            self.apply(Effect::Preview(next)).expect("preview is valid");
        } else {
            self.settings = next;
        }
    }
}

fn pan_delta(code: KeyCode, visible: (u32, u32), step: (u32, u32)) -> Option<(i64, i64)> {
    let page_x = i64::from(visible.0.saturating_sub(step.0 * 2).max(step.0));
    let page_y = i64::from(visible.1.saturating_sub(step.1 * 2).max(step.1));
    let (sx, sy) = (i64::from(step.0), i64::from(step.1));
    match code {
        KeyCode::Char('H') => Some((-page_x, 0)),
        KeyCode::Char('L') => Some((page_x, 0)),
        KeyCode::Char('K') => Some((0, -page_y)),
        KeyCode::Char('J') => Some((0, page_y)),
        KeyCode::Left | KeyCode::Char('h') => Some((-sx, 0)),
        KeyCode::Right | KeyCode::Char('l') => Some((sx, 0)),
        KeyCode::Up | KeyCode::Char('k') => Some((0, -sy)),
        KeyCode::Down | KeyCode::Char('j') => Some((0, sy)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        settings::{Palettes, Rotation},
        ui::Layout,
    };
    use ratatui::layout::Rect;
    use rthid_dither::dither::Mode;

    fn done(outcome: IoOutcome) -> Done {
        Done {
            id: 1,
            elapsed: std::time::Duration::ZERO,
            outcome,
        }
    }

    fn failure(op: rthid_io::io::Op) -> rthid_io::io::IoError {
        rthid_io::io::IoError {
            op,
            path: "failed.png".into(),
            cause: rthid_io::io::Cause::Io(std::io::Error::other("denied")),
        }
    }

    #[test]
    fn completions_route_images_directories_listings_and_errors() {
        use rthid_io::io::{Listing, Op};
        let listing = || Listing {
            dir: "/images".into(),
            entries: vec![],
        };
        let mut app = App::empty();
        app.set_notice("retained".into());
        assert!(!app.complete(done(IoOutcome::Listed(Ok(listing())))));
        assert_eq!(app.notice(), Some("retained"));
        assert!(app.modal.is_none());
        app.request_open("/images".into(), None);
        assert!(app.complete(done(IoOutcome::Opened(Ok(Opened::Dir(listing()))))));
        assert!(app.notice().is_none());
        let Some(Modal::OpenFile(picker)) = &app.modal else {
            panic!("open dialog expected")
        };
        assert_eq!(picker.dir, PathBuf::from("/images"));
        for outcome in [
            IoOutcome::Opened(Err(failure(Op::Decode))),
            IoOutcome::Listed(Err(failure(Op::List))),
        ] {
            app.request_open("failed.png".into(), None);
            let expected = match &outcome {
                IoOutcome::Opened(Err(error)) | IoOutcome::Listed(Err(error)) => error.to_string(),
                _ => unreachable!(),
            };
            assert!(app.complete(done(outcome)));
            assert_eq!(app.notice(), Some(expected.as_str()));
            assert!(matches!(app.modal, Some(Modal::OpenFile(_))));
        }
        assert!(app.complete(done(IoOutcome::Listed(Ok(listing())))));
        assert!(app.notice().is_none());
        app.request_open("photo.png".into(), None);
        assert!(app.complete(done(IoOutcome::Opened(Ok(Opened::Image {
            path: "photo.png".into(),
            image: RgbaImage::new(2, 3),
        })))));
        assert!(app.modal.is_none());
        assert!(app.notice().is_none());
        assert_eq!(app.source_path(), Some(std::path::Path::new("photo.png")));
        assert_eq!(app.grid_size(), (2, 3));
    }

    #[test]
    fn export_jobs_protect_the_destination_and_retain_errors_until_success() {
        use rthid_io::io::Op;
        let path = PathBuf::from("preview.png");
        let mut app = App::builtin(RgbaImage::new(4, 2), Some(path.clone()));
        app.request_open("photo.png".into(), Some("/images".into()));
        let jobs = app.take_jobs();
        assert!(
            matches!(jobs.as_slice(), [Job::Export { path: output, grid }, Job::Open { path: source, dir: Some(dir), protect: Some(protect) }]
            if output == &path && protect == &path && source == &PathBuf::from("photo.png") && dir == &PathBuf::from("/images")
                && (grid.w, grid.h) == (4, 2) && grid.indices.len() == 8 && grid.palette == app.settings.options.palette)
        );
        assert!(app.complete(done(IoOutcome::Exported(Err(failure(Op::Encode))))));
        let error = app.export_status().unwrap().to_owned();
        assert_eq!(app.notice(), Some(error.as_str()));
        assert!(app.export_result().is_err());
        app.settings.rotation = Rotation::R90;
        app.finish();
        assert!(matches!(app.take_jobs().as_slice(), [Job::Export { .. }]));
        assert_eq!(app.export_status(), Some(error.as_str()));
        assert!(!app.complete(done(IoOutcome::Exported(Err(failure(Op::Encode))))));
        assert!(app.complete(done(IoOutcome::Exported(Ok(path.clone())))));
        assert!(app.export_status().is_none());
        assert!(app.notice().is_none());
        assert!(app.export_result().is_ok());
        assert!(!app.complete(done(IoOutcome::Exported(Ok(path)))));
    }

    #[test]
    fn sync_reports_only_grid_or_dither_rebuilds() {
        let mut app = App::builtin(RgbaImage::new(4, 2), None);
        let preview = test_layout().preview;
        assert!(!app.sync(preview));
        app.settings.scale = 200;
        assert!(app.sync(preview));
        assert!(!app.sync(preview));
        app.show_original = false;
        assert!(app.sync(preview));
        assert!(!app.sync(preview));
        app.settings.options.strength = 0.5;
        assert!(app.sync(preview));
        assert!(!app.sync(preview));
        assert!(!App::empty().sync(preview));
    }

    #[test]
    fn open_dialog_requests_listing_and_retains_failed_open() {
        for source in [None, Some(PathBuf::from("/images/photo.png"))] {
            let mut app = App::empty();
            app.source_path = source.clone();
            press(&mut app, KeyCode::Char('o'));
            let dir = if source.is_some() { "/images" } else { "." };
            assert!(
                matches!(app.take_jobs().as_slice(), [Job::List(path)] if path == &PathBuf::from(dir))
            );
            app.set_listing(
                PathBuf::from(dir),
                vec![Entry {
                    name: "bad.png".into(),
                    dir: false,
                }],
            );
            press(&mut app, KeyCode::Enter);
            assert!(
                matches!(app.take_jobs().as_slice(), [Job::Open { path, dir: Some(base), protect: None }]
                if path == &PathBuf::from("bad.png") && base == &PathBuf::from(dir))
            );
            app.set_notice("open: invalid image".into());
            assert!(matches!(app.modal, Some(Modal::OpenFile(_))));
            assert_eq!(app.notice(), Some("open: invalid image"));
            app.handle_paste("'file:///tmp/My%20Photo.png'");
            assert!(app.take_jobs().is_empty());
            let Some(Modal::OpenFile(picker)) = &app.modal else {
                panic!("open dialog expected")
            };
            assert_eq!(picker.input.text(), "/tmp/My Photo.png");
            assert!(picker.typed);
            app.handle_paste(" suffix");
            assert_eq!(
                app.modal.as_mut().unwrap().text_input_mut().unwrap().text(),
                "/tmp/My Photo.png suffix"
            );
            for path in ["/typed.png", "relative.png"] {
                app.handle_paste(path);
                press(&mut app, KeyCode::Enter);
                assert!(
                    matches!(app.take_jobs().as_slice(), [Job::Open { path: entered, dir: Some(base), protect: None }]
                    if entered == &PathBuf::from(path) && base == &PathBuf::from(dir))
                );
            }
        }
    }

    #[test]
    fn paste_edits_active_fields_or_requests_an_image() {
        let mut app = App::empty();
        for code in [KeyCode::Char('P'), KeyCode::Char('n'), KeyCode::Enter] {
            press(&mut app, code);
        }
        app.handle_paste("My\n Pa\tlette猫\u{1b}");
        assert_eq!(
            app.modal.as_mut().unwrap().text_input_mut().unwrap().text(),
            "My Palette猫"
        );
        assert!(app.take_jobs().is_empty());
        press(&mut app, KeyCode::Esc);
        app.handle_paste("'/tmp/ignored.jpg'");
        assert!(app.take_jobs().is_empty());

        app.modal = Some(Modal::templates());
        press(&mut app, KeyCode::Char('n'));
        app.handle_paste("Template");
        assert_eq!(
            app.modal.as_mut().unwrap().text_input_mut().unwrap().text(),
            "Template"
        );
        app.modal = None;
        app.handle_paste("ordinary text");
        app.handle_paste("notes.txt");
        assert!(app.take_jobs().is_empty());
        app.handle_paste("'/tmp/My Photo.jpg'");
        assert_eq!(app.notice(), Some("Loading My Photo.jpg"));
        press(&mut app, KeyCode::Char('o'));
        app.apply(Effect::ListDir(PathBuf::from("/other"))).unwrap();
        assert!(matches!(app.take_jobs().as_slice(), [
            Job::Open { path, dir: None, protect: None }, Job::List(_), Job::List(listed)
        ] if path == &PathBuf::from("/tmp/My Photo.jpg") && listed == &PathBuf::from("/other")));
        assert!(app.take_jobs().is_empty());
    }

    #[test]
    fn empty_preview_ignores_navigation_but_keeps_panel_edits() {
        let mut app = App::empty();
        assert!(app.source_path().is_none());
        assert!(app.displayed().is_none());
        assert_eq!(app.grid_size(), (0, 0));
        assert_eq!(app.grid_cache.source_size(), None);
        for focus in [false, true] {
            app.preview_focus = focus;
            for code in [' ', '+', '=', '-', '0', 'm'] {
                press(&mut app, KeyCode::Char(code));
            }
            if focus {
                for code in [
                    KeyCode::Left,
                    KeyCode::Right,
                    KeyCode::Up,
                    KeyCode::Down,
                    KeyCode::Char('h'),
                    KeyCode::Char('j'),
                    KeyCode::Char('k'),
                    KeyCode::Char('l'),
                    KeyCode::Char('H'),
                    KeyCode::Char('J'),
                    KeyCode::Char('K'),
                    KeyCode::Char('L'),
                ] {
                    press(&mut app, code);
                }
            }
            app.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: 2,
                    row: 3,
                    modifiers: KeyModifiers::NONE,
                },
                &app.geometry(test_layout().preview),
                test_layout().screen,
            );
            assert_eq!(app.viewport.zoom, Zoom::Fit);
            assert_eq!((app.viewport.x, app.viewport.y), (0, 0));
            assert!(!app.viewport.hide_minimap);
            assert!(app.last_drag.is_none());
            assert!(app.show_original);
            assert_eq!(versions(&app), (0, 0));
        }
        app.preview_focus = false;
        press(&mut app, KeyCode::Right);
        assert_eq!(app.settings.scale, 110);
        assert!(app.displayed().is_none());
        app.open_image(PathBuf::from("first.png"), RgbaImage::new(20, 10));
        assert_eq!(app.grid_size(), (22, 11));
        assert!(app.displayed().is_some());
        assert!(app.show_original);
        assert_eq!(versions(&app), (1, 0));
    }

    #[test]
    fn open_image_resets_navigation_and_rewrites_companion() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("preview.png");
        let mut app = App::builtin(RgbaImage::new(4, 2), Some(output.clone()));
        assert!(
            matches!(app.take_jobs().as_slice(), [Job::Export { path, grid }]
            if path == &output && (grid.w, grid.h) == (4, 2))
        );
        app.settings = app.settings.with_preset(app.palettes.get(1).unwrap());
        app.settings.options.tone.brightness = 0.2;
        app.settings.rotation = Rotation::R90;
        app.show_original = false;
        app.refresh();
        let settings = app.settings.clone();
        let before = versions(&app);
        app.viewport.zoom = Zoom::In(2);
        app.viewport.x = 1;
        app.viewport.hide_minimap = true;
        app.last_drag = Some(((3, 4), (1, 0)));
        let path = PathBuf::from("replacement.png");
        app.open_image(path.clone(), RgbaImage::new(3, 5));
        assert_eq!(app.source_path(), Some(path.as_path()));
        assert_eq!(app.viewport.zoom, Zoom::Fit);
        assert_eq!((app.viewport.x, app.viewport.y), (0, 0));
        assert!(!app.viewport.hide_minimap);
        assert!(app.last_drag.is_none() && app.show_original);
        assert_eq!(app.settings.rotation, Rotation::R0);
        assert_eq!(app.settings.options, settings.options);
        assert_eq!(app.settings.palette_hex, settings.palette_hex);
        assert_eq!(app.settings.palette_name, settings.palette_name);
        assert_eq!(app.grid_size(), (3, 5));
        assert_eq!(versions(&app), (before.0 + 1, before.1 + 1));
        assert!(
            matches!(app.take_jobs().last(), Some(Job::Export { path, grid })
            if path == &output && (grid.w, grid.h) == (3, 5))
        );
    }

    #[test]
    fn template_manager_saves_and_applies_settings() {
        let mut app = app_at(Setting::Strength);
        app.settings.options.strength = 0.5;
        let saved = app.settings.clone();
        for code in [
            KeyCode::Char('T'),
            KeyCode::Char('n'),
            KeyCode::Char('Z'),
            KeyCode::Enter,
        ] {
            press(&mut app, code);
        }
        assert!(app.modal.is_none() && !app.take_changes().is_empty());
        assert_eq!(
            app.templates.get(0),
            Some(&Template::capture("Z".into(), &saved))
        );
        app.settings.options.strength = 0.9;
        app.settings.rotation = Rotation::R90;
        for code in [KeyCode::Char('T'), KeyCode::Enter] {
            press(&mut app, code);
        }
        assert_eq!(app.settings.options.strength, 0.5);
        assert_eq!(app.settings.rotation, Rotation::R90);
        assert!(!app.show_original && app.modal.is_none() && app.take_changes().is_empty());
    }

    #[test]
    fn preset_editor_saves_and_keeps_failed_save_open() {
        let mut app = app_at(Setting::Palette);
        for code in [KeyCode::Char('P'), KeyCode::Char('n'), KeyCode::Char('s')] {
            press(&mut app, code);
        }
        assert!(app.notice().unwrap().contains("must not be empty"));
        assert!(app.modal.is_some() && app.take_changes().is_empty());
        press(&mut app, KeyCode::Down);
        app.handle_key(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &app.geometry(test_layout().preview),
        );
        assert!(
            matches!(&app.modal, Some(Modal::Editor(editor)) if editor.draft.colors == app.settings.palette_hex && editor.rows.selected() == 1 && editor.input.is_none())
        );
        press(&mut app, KeyCode::Up);
        for code in [
            KeyCode::Enter,
            KeyCode::Char('Z'),
            KeyCode::Enter,
            KeyCode::Char('s'),
        ] {
            press(&mut app, code);
        }
        assert_eq!(app.settings.palette_name, "Z");
        assert!(app.palettes.find("Z").is_some());
        assert!(app.modal.is_none() && !app.take_changes().is_empty() && !app.show_original);
    }

    #[test]
    fn failed_save_requeues_edits_before_newer_changes() {
        let mut app = App::empty();
        app.apply(Effect::SaveTemplate("First".into())).unwrap();
        app.apply(Effect::RemoveTemplate(0)).unwrap();
        let changes = app.take_changes();
        assert!(
            matches!(&changes[..], [Change::UpsertTemplate(t), Change::RemoveTemplate(name)]
            if t.name == "First" && name == "First")
        );
        app.apply(Effect::SaveTemplate("Later".into())).unwrap();
        let mut expected = changes.clone();
        expected.extend(app.pending.clone());
        app.save_failed(changes, "busy".into());
        assert_eq!(app.notice(), Some("config: busy"));
        assert_eq!(app.take_changes(), expected);
        assert!(app.take_changes().is_empty());
    }

    #[test]
    fn saved_keeps_palette_selection_by_name_and_reports_warnings() {
        let mut app = App::empty();
        let preset = crate::settings::Preset {
            name: "Selected".into(),
            colors: vec!["#123456".into()],
            mode: None,
            builtin: false,
        };
        app.apply(Effect::SavePreset {
            preset,
            base: app.settings.clone(),
        })
        .unwrap();
        let settings = app.settings.clone();
        let index = app.palettes.find("Selected").unwrap();
        let loaded = crate::store::parse(
            "version = 1\n[[palette]]\nname = 'Other'\ncolors = ['#fff']\n[[palette]]\nname = 'Selected'\ncolors = ['#123456']\n[[template]]\nname = 'Zine'\n[[palette]]\nname = 'Bad'\ncolors = ['oops']\n",
        );
        let notice = loaded.notice();
        app.saved(loaded);
        assert_eq!(app.settings.palette_name, "Selected");
        assert!(app.settings == settings);
        assert_eq!(app.palettes.find("Selected"), Some(index + 1));
        assert!(app.templates.find("Zine").is_some());
        assert_eq!(app.notice(), notice.as_deref());
        assert!(app.notice().unwrap().contains("1 entries skipped"));
        app.apply(Effect::RemovePreset(index + 1)).unwrap();
        assert!(
            matches!(app.take_changes().last(), Some(Change::RemovePreset(name)) if name == "Selected")
        );
        app.saved(Loaded::default());
        assert!(app.notice().is_none());
    }

    #[test]
    fn notices_and_save_blocking_survive_input() {
        for blocked in [false, true] {
            let mut templates = Templates::default();
            templates
                .upsert(Template::capture("Zine".into(), &Settings::default()))
                .unwrap();
            let mut app = App::new(
                Some((PathBuf::from("test.png"), RgbaImage::new(4, 4))),
                None,
                Palettes::builtin(),
                templates.clone(),
                Some("config warning".into()),
                blocked.then(|| "broken file".into()),
            );
            assert_eq!(app.templates, templates);
            assert_eq!(app.notice(), Some("config warning"));
            assert!(app.take_changes().is_empty());
            app.handle_key(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                &app.geometry(Rect::new(0, 0, 20, 10)),
            );
            assert!(app.notice().is_none());
            app.apply(Effect::SaveTemplate("Another".into())).unwrap();
            assert_eq!(app.take_changes().is_empty(), blocked);
            assert!(app.take_changes().is_empty());
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
                assert_eq!(pan_delta(code, (20, 12), (1, 1)), Some(expected));
            }
        }
        for (code, expected) in [
            ('H', (-18, 0)),
            ('L', (18, 0)),
            ('K', (0, -10)),
            ('J', (0, 10)),
        ] {
            assert_eq!(
                pan_delta(KeyCode::Char(code), (20, 12), (1, 1)),
                Some(expected)
            );
        }
        for visible in [(0, 0), (1, 1), (2, 2)] {
            assert_eq!(
                pan_delta(KeyCode::Char('H'), visible, (1, 1)),
                Some((-1, 0))
            );
            assert_eq!(pan_delta(KeyCode::Char('J'), visible, (1, 1)), Some((0, 1)));
        }
        for code in ['m', 'z', 'q'] {
            assert_eq!(pan_delta(KeyCode::Char(code), (20, 12), (1, 1)), None);
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
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        app.panel = ListPicker::new(Setting::ALL.iter().position(|&p| p == setting).unwrap());
        app
    }

    #[test]
    fn empty_preview_navigation_and_scaled_pages() {
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        app.preview_focus = true;
        for zoom in [Zoom::Fit, Zoom::Out(2), Zoom::In(4)] {
            app.viewport.zoom = zoom;
            for code in [
                KeyCode::Left,
                KeyCode::Char('J'),
                KeyCode::Char('+'),
                KeyCode::Char('-'),
            ] {
                app.handle_key(
                    KeyEvent::new(code, KeyModifiers::NONE),
                    &app.geometry(Rect::default()),
                );
            }
        }
        for (code, expected) in [('h', (-2, 0)), ('j', (0, 5)), ('L', (16, 0)), ('J', (0, 5))] {
            assert_eq!(
                pan_delta(KeyCode::Char(code), (20, 12), (2, 5)),
                Some(expected)
            );
        }
    }

    #[test]
    fn grid_cache_rebuilds_only_stale_stages() {
        let mut app = App::builtin(RgbaImage::new(20, 10), None);
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
        let before = versions(&app);
        let replacement = RgbaImage::from_pixel(20, 10, image::Rgba([255, 255, 255, 255]));
        app.grid_cache.set_source(replacement);
        assert!(app.grid_cache.grid().is_none());
        assert!(app.grid_cache.dithered().is_none());
        assert_eq!(versions(&app), before);
        app.sync(test_layout().preview);
        assert_eq!(versions(&app), (before.0 + 1, before.1 + 1));
        assert_eq!(app.grid_size(), (w, h));
        assert_eq!(app.grid_cache.grid().unwrap().get_pixel(0, 0).0, [255; 4]);
    }

    #[test]
    fn sync_clamps_after_layout_and_grid_changes() {
        let mut app = App::builtin(RgbaImage::new(120, 80), None);
        app.viewport.zoom = Zoom::In(1);
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
        let mut app = App::builtin(RgbaImage::new(20, 10), None);
        app.panel = ListPicker::new(
            Setting::ALL
                .iter()
                .position(|&p| p == Setting::Strength)
                .unwrap(),
        );
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
        app.panel = ListPicker::new(0);
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
        let app = App::builtin(source, None);
        let Some(Pixels::Rgb(pixels, _)) = app.displayed() else {
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
        assert_eq!(app.panel.selected(), 0);
        for _ in 0..Setting::ALL.len() + 2 {
            assert!(!press(&mut app, KeyCode::Char('j')));
        }
        assert_eq!(Setting::ALL[app.panel.selected()], Setting::Sharpen);
        for code in [KeyCode::Char('x'), KeyCode::Enter, KeyCode::Esc] {
            assert!(!press(&mut app, code));
            assert_eq!(app.panel.selected(), Setting::ALL.len() - 1);
            assert!(!app.quit);
        }
    }

    #[test]
    fn numeric_settings_clamp_without_redither() {
        type Getter = fn(&Settings) -> f64;
        let cases: [(Setting, Getter); 8] = [
            (Setting::Scale, |p| f64::from(p.scale)),
            (Setting::Strength, |p| p.options.strength),
            (Setting::Brightness, |p| p.options.tone.brightness),
            (Setting::Contrast, |p| p.options.tone.contrast),
            (Setting::Gamma, |p| p.options.tone.gamma),
            (Setting::Black, |p| p.options.tone.black),
            (Setting::White, |p| p.options.tone.white),
            (Setting::Sharpen, |p| p.options.tone.sharpen),
        ];
        for (setting, get) in cases {
            let (_, low, high) = setting.range().unwrap();
            let mut app = app_at(setting);
            for (key, limit) in [(KeyCode::Left, low), (KeyCode::Right, high)] {
                for _ in 0..100 {
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
        // Wrapping back lands on the last built-in, which also switches to its Color mode.
        press(&mut app, KeyCode::Left);
        assert_eq!(app.settings.palette_name, "Pratt");
        assert_eq!(app.settings.options.palette.len(), 5);
        assert_eq!(app.settings.options.mode, Mode::Color);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.settings.palette_name, "Black/white");
        assert_eq!(app.settings.options.palette.len(), 2);
    }

    #[test]
    fn reset_restores_selected_or_all_defaults() {
        let mut app = app_at(Setting::Scale);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('r'));
        app.panel = ListPicker::new(1);
        press(&mut app, KeyCode::Right);
        assert!(press(&mut app, KeyCode::Char('d')));
        assert!(matches!(app.settings.options.mode, Mode::Tone));
        assert_eq!(
            (app.settings.scale, app.settings.rotation),
            (110, Rotation::R90)
        );
        app.panel = ListPicker::new(2);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('D'));
        let p = &app.settings;
        assert_eq!((p.scale, p.rotation), (100, Rotation::R0));
        assert_eq!(p.palette_name, "Black/white");
        assert_eq!(p.options.palette.len(), 2);
        assert_eq!(app.grid_cache.dithered.get().unwrap().len(), 16);
    }

    #[test]
    fn reset_only_redithers_changed_values_and_preserves_pending_changes() {
        for setting in Setting::ALL {
            let mut app = App::builtin(RgbaImage::new(20, 10), None);
            app.panel = ListPicker::new(Setting::ALL.iter().position(|&p| p == setting).unwrap());
            assert!(!press(&mut app, KeyCode::Char('d')), "{setting:?}");
            assert!(!press(&mut app, KeyCode::Char('D')), "{setting:?}");
            let change = if matches!(setting, Setting::Black | Setting::Sharpen) {
                KeyCode::Right
            } else {
                KeyCode::Left
            };
            assert!(press(&mut app, change));
            assert!(press(&mut app, KeyCode::Char('d')), "{setting:?}");
            assert!(!press(&mut app, KeyCode::Char('d')), "{setting:?}");
            assert!(press(&mut app, change));
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
        let mut app = App::builtin(RgbaImage::new(256, 128), None);
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
        let mut app = App::builtin(RgbaImage::new(15, 1), None);
        app.show_original = false;
        for _ in 0..9 {
            press(&mut app, KeyCode::Left);
        }
        assert_eq!(app.grid_size(), (2, 1));
        assert_eq!(app.grid_cache.dithered.get().unwrap().len(), 2);
    }
    #[test]
    fn global_zoom_and_preview_pans_skip_redithering() {
        let mut app = App::builtin(RgbaImage::new(400, 400), None);
        for focus in [false, true] {
            app.preview_focus = focus;
            assert!(!press(&mut app, KeyCode::Char('0')));
            assert_eq!(app.viewport.zoom, Zoom::Fit);
            assert!(!press(&mut app, KeyCode::Char('+')));
            assert_eq!(app.viewport.zoom, Zoom::Out(4));
            assert!(!press(&mut app, KeyCode::Char('=')));
            assert_eq!(app.viewport.zoom, Zoom::Out(3));
            assert!(!press(&mut app, KeyCode::Char('-')));
            assert_eq!(app.viewport.zoom, Zoom::Out(4));
            assert!(!press(&mut app, KeyCode::Char('m')));
        }
        app.viewport.zoom = Zoom::Out(2);
        app.viewport.x = 0;
        app.viewport.y = 0;
        assert!(!press(&mut app, KeyCode::Right));
        assert_eq!(app.viewport.x, 2);
        assert!(!press(&mut app, KeyCode::Char('L')));
        assert_eq!(app.viewport.x, 18);
        assert!(!press(&mut app, KeyCode::Down));
        assert_eq!(app.viewport.y, 5);
        for _ in 0..100 {
            press(&mut app, KeyCode::Char('J'));
        }
        assert_eq!(app.viewport.y, 378);
        press(&mut app, KeyCode::Char('0'));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.viewport.x, 0);
        assert_eq!(app.viewport.zoom, Zoom::Fit);
        press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.viewport.zoom, Zoom::Fit);
        press(&mut app, KeyCode::Tab);
        assert!(press(&mut app, KeyCode::Right));
        assert_eq!(app.settings.scale, 110);
    }

    #[test]
    fn mouse_centers_pans_and_scrolls_without_focus_or_redithering() {
        let mut app = App::builtin(RgbaImage::new(120, 120), None);
        let area = Rect::new(2, 3, 48, 18);
        app.viewport.zoom = Zoom::In(1);
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
                test_layout().screen,
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
        assert_eq!(app.viewport.y, 68);
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
        app.viewport.zoom = Zoom::Fit;
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            map.x,
            map.y,
        );
        mouse(&mut app, MouseEventKind::ScrollDown, 13, 12);
        assert_eq!((app.viewport.x, app.viewport.y), (69, 80));
        app.viewport.zoom = Zoom::In(1);
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
        let mut app = App::builtin(RgbaImage::new(120, 120), None);
        app.preview_focus = true;
        app.viewport.zoom = Zoom::In(1);
        assert!(press(&mut app, KeyCode::Char(' ')));
        for hidden in [true, false] {
            assert!(!press(&mut app, KeyCode::Char('m')));
            assert_eq!(app.viewport.hide_minimap, hidden);
        }
    }

    #[test]
    fn mouse_drag_and_scroll_follow_zoom_without_focus() {
        let area = Rect::new(0, 0, 48, 18);
        for (zoom, dragged, scrolled) in [
            (Zoom::Out(2), (106, 109), (118, 136)),
            (Zoom::In(2), (102, 102), (105, 108)),
            (Zoom::In(4), (101, 101), (103, 104)),
        ] {
            let mut app = App::builtin(RgbaImage::new(1000, 1000), None);
            app.viewport = Viewport {
                zoom,
                x: 100,
                y: 100,
                hide_minimap: true,
            };
            for (kind, column, row) in [
                (MouseEventKind::Down(MouseButton::Left), 10, 10),
                (MouseEventKind::Drag(MouseButton::Left), 9, 9),
                (MouseEventKind::Drag(MouseButton::Left), 7, 8),
            ] {
                app.handle_mouse(
                    MouseEvent {
                        kind,
                        column,
                        row,
                        modifiers: KeyModifiers::NONE,
                    },
                    &app.geometry(area),
                    test_layout().screen,
                );
            }
            assert_eq!((app.viewport.x, app.viewport.y), dragged, "{zoom:?}");
            for kind in [MouseEventKind::ScrollRight, MouseEventKind::ScrollDown] {
                app.handle_mouse(
                    MouseEvent {
                        kind,
                        column: 7,
                        row: 8,
                        modifiers: KeyModifiers::NONE,
                    },
                    &app.geometry(area),
                    test_layout().screen,
                );
            }
            assert_eq!((app.viewport.x, app.viewport.y), scrolled, "{zoom:?}");
            assert!(!app.preview_focus);
        }
    }

    #[test]
    fn finish_exports_changes_queued_with_quit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preview.png");
        let mut app = App::builtin(RgbaImage::new(4, 2), Some(path.clone()));
        for code in [KeyCode::Char('r'), KeyCode::Char('q')] {
            app.handle_key(
                KeyEvent::new(code, KeyModifiers::NONE),
                &app.geometry(test_layout().preview),
            );
        }
        assert!(app.quit);
        assert_eq!(app.grid_cache.grid.version(), 1);
        app.take_jobs();
        app.finish();
        assert!(
            matches!(app.take_jobs().as_slice(), [Job::Export { path: output, grid }]
            if output == &path && (grid.w, grid.h) == (2, 4))
        );
        assert!(app.take_jobs().is_empty());
    }
}
