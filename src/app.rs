//! Interactive state and the key and mouse handling that changes it.

use crate::{
    cells::{Glyphs, Layer, Pixels},
    export::Export,
    grid::GridCache,
    memo::Memo,
    modal::{Ctx, Effect, Modal, Next, PATH_MAX_CHARS, Step},
    settings::{Rotation, Setting, Settings, Template},
    store::{Change, Library, Loaded},
    viewport::{PreviewGeometry, Viewport, Zoom},
};
use image::RgbaImage;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use rthid_dither::{
    dither::PaletteColor,
    mass::{self, MassOptions},
};
use rthid_io::{
    dir::Entry,
    io::{Done, Job, Opened, Outcome as IoOutcome},
};
use rthid_widgets::{ListPicker, Outcome};
use std::{
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

struct OpenRequest {
    /// Tag the result echoes, unique to this request.
    token: u64,
    /// Generation of the dialog that asked, if any.
    dialog: Option<u64>,
    /// Status label shown until this request finishes.
    label: String,
}

#[derive(Clone, Copy)]
struct Drag {
    point: (u16, u16),
    origin: (u32, u32),
    moved: bool,
    minimap: bool,
}

enum Source {
    Export,
    Other,
}

enum PreviewKey {
    ToggleOriginal,
    ToggleMass,
    ToggleMinimap,
    ToggleMinimapMass,
    Zoom(Zoom),
    Pan(i64, i64),
}

struct Notice {
    source: Source,
    text: String,
}

/// All interactive state: settings, focus, preview viewport, grid caches and
/// the optional companion PNG.
pub struct App {
    export: Option<Export>,
    source_path: Option<PathBuf>,
    notice: Option<Notice>,
    outbox: Vec<(Job, u64)>,
    open_request: Option<OpenRequest>,
    // Last value handed out as a dialog generation or job token.
    // Zero tags requests without a dialog.
    last_token: u64,
    /// Active dialog, which receives input before global bindings.
    pub modal: Option<Modal>,
    /// Glyph set for sampled Dithered cells and the minimap.
    pub glyphs: Glyphs,
    /// Preview zoom, minimap and dot origin.
    pub viewport: Viewport,
    /// Whether keys go to the preview first (toggled by Tab, left with Esc).
    pub preview_focus: bool,
    last_drag: Option<Drag>,
    /// Whether the last synced preview has a visible minimap.
    minimap_visible: bool,
    /// Image processing caches.
    pub grid_cache: GridCache,
    /// Named configuration lists and their pending saves.
    pub library: Library,
    /// Current image settings.
    pub settings: Settings,
    /// Parameter selection and retained scroll offset.
    pub panel: ListPicker,
    /// Selected preview layer, initially Original.
    pub layer: Layer,
    /// Layer that `a` returns to from the mass study.
    pub mass_return: Layer,
    /// Whether the minimap shows the mass study instead of the main layer.
    pub mass_minimap: bool,
    /// Geometry controls for the mass study.
    pub mass: MassOptions,
    /// Fixed mass study tones, or None to follow the dither palette.
    pub mass_tones: Option<[PaletteColor; 4]>,
    /// Whether the event loop should exit.
    pub quit: bool,
}

impl App {
    /// Builds the source grid and queues a companion PNG when requested.
    pub fn new(
        source: Option<(PathBuf, RgbaImage)>,
        preview_png: Option<PathBuf>,
        dpi: Option<u32>,
        loaded: Loaded,
    ) -> Self {
        let mut app = Self {
            export: preview_png.map(|path| Export::new(path, dpi)),
            source_path: None,
            glyphs: Glyphs::default(),
            viewport: Viewport::default(),
            preview_focus: false,
            last_drag: None,
            minimap_visible: false,
            grid_cache: GridCache {
                source: None,
                grid: Memo::default(),
                dithered: Memo::default(),
                mass: Memo::default(),
            },
            library: Library::new(loaded),
            notice: None,
            outbox: Vec::new(),
            open_request: None,
            last_token: 0,
            modal: None,
            settings: Settings::default(),
            panel: ListPicker::new(0),
            layer: Layer::Original,
            mass_return: Layer::Original,
            mass_minimap: true,
            mass: MassOptions::default(),
            mass_tones: None,
            quit: false,
        };
        if let Some((path, image)) = source {
            app.open_image(path, image);
        }
        app
    }

    /// Builds an app with built-ins and no configuration diagnostics for tests.
    #[cfg(test)]
    pub fn builtin(source: RgbaImage, preview_png: Option<PathBuf>) -> Self {
        Self::new(
            Some((PathBuf::from("test.png"), source)),
            preview_png,
            None,
            Loaded::default(),
        )
    }

    /// Builds a scrollable image with the minimap hidden for drag tests.
    #[cfg(test)]
    pub(crate) fn drag_fixture() -> Self {
        let mut app = Self::builtin(RgbaImage::new(120, 120), None);
        app.settings = Settings::for_pixels(120, 120);
        app.refresh(false);
        app.viewport.zoom = Zoom::In(1);
        app.viewport.hide_minimap = true;
        app
    }

    /// Sends a mouse event at a cell using the supplied preview geometry.
    #[cfg(test)]
    pub(crate) fn mouse_at(
        &mut self,
        kind: MouseEventKind,
        column: u16,
        row: u16,
        geometry: &PreviewGeometry,
        screen: Rect,
    ) -> bool {
        self.handle_mouse(mouse_event(kind, column, row), geometry, screen)
    }

    /// Builds an app without an image for tests.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self::new(None, None, None, Loaded::default())
    }

    /// Path of the loaded source, if any.
    pub fn source_path(&self) -> Option<&std::path::Path> {
        self.source_path.as_deref()
    }

    /// Loaded source size or a single pixel placeholder.
    pub(crate) fn source_size(&self) -> (u32, u32) {
        self.grid_cache
            .source
            .as_ref()
            .map_or((1, 1), RgbaImage::dimensions)
    }

    /// Replaces the source and resets image composition and preview navigation.
    pub fn open_image(&mut self, path: PathBuf, image: RgbaImage) {
        self.source_path = Some(path);
        self.grid_cache.set_source(image);
        self.viewport = Viewport::default();
        self.last_drag = None;
        self.settings.rotation = Rotation::R0;
        self.settings.flip = false;
        self.settings.zoom = 100;
        self.settings.pan_x = 0;
        self.settings.pan_y = 0;
        self.layer = Layer::Original;
        self.mass_return = Layer::Original;
        self.refresh(false);
    }

    /// Supplies directory entries to the open dialog if it is still open.
    pub fn set_listing(&mut self, dir: PathBuf, entries: Vec<Entry>) {
        self.route_listing(dir, entries);
        self.notice = None;
    }

    fn route_listing(&mut self, dir: PathBuf, entries: Vec<Entry>) {
        if let Some(Modal::OpenFile(picker)) = &mut self.modal {
            picker.set_listing(dir, entries);
        }
    }

    /// Status text with loading and the current notice before persistent diagnostics.
    /// Fallbacks are card limits, visible mass errors, library diagnostics and export status.
    pub fn notice(&self) -> Option<&str> {
        self.open_request
            .as_ref()
            .map(|request| request.label.as_str())
            .or_else(|| self.notice.as_ref().map(|notice| notice.text.as_str()))
            .or_else(|| self.settings.dimensions(self.source_size()).err())
            .or_else(|| {
                (self.layer == Layer::Mass || (self.mass_minimap && self.minimap_visible))
                    .then(|| self.grid_cache.mass_error().map(|(_, text)| text))
                    .flatten()
            })
            .or_else(|| self.library.notice())
            .or_else(|| self.export_status())
    }

    /// Reports a notice and returns whether the display changed.
    pub fn set_notice(&mut self, notice: String) -> bool {
        self.report_notice(Notice {
            source: Source::Other,
            text: notice,
        })
    }

    fn report_notice(&mut self, notice: Notice) -> bool {
        let changed = self.open_request.is_none() && self.notice() != Some(notice.text.as_str());
        self.notice = Some(notice);
        changed
    }

    /// Queues a save when edits arrived or the retry deadline passed.
    pub fn queue_save(&mut self, path: &Path, now: Instant) {
        let token = self.next_token();
        if let Some(job) = self.library.save_job(path, now, token) {
            self.outbox.push(job);
        }
    }

    /// Drains filesystem jobs for submission after sync, each with the tag its result echoes.
    pub fn take_jobs(&mut self) -> Vec<(Job, u64)> {
        std::mem::take(&mut self.outbox)
    }

    /// Applies a finished job and reports whether the display changed.
    pub fn complete(&mut self, done: Done) -> bool {
        let mut changed = false;
        // Dialog that asked for the result. Listings always come from one.
        let mut requester = Some(done.tag);
        let opened = matches!(done.outcome, IoOutcome::Opened(_));
        if matches!(done.outcome, IoOutcome::Listed(_)) && self.dialog() != Some(done.tag) {
            return false;
        }
        if opened {
            let Some(request) = self
                .open_request
                .take_if(|request| request.token == done.tag)
            else {
                return false;
            };
            requester = request.dialog;
            changed = true;
        }
        match done.outcome {
            IoOutcome::Opened(Ok(Opened::Image { path, image })) => {
                if requester.is_some() && requester == self.dialog() {
                    self.modal = None;
                }
                self.open_image(path, image);
                changed = true;
            }
            IoOutcome::Opened(Ok(Opened::Dir(listing))) | IoOutcome::Listed(Ok(listing)) => {
                if requester.is_none() && self.modal.is_none() {
                    self.open_dialog(listing.dir.clone());
                }
                // Opens without a dialog fill whichever dialog is open.
                let routed = match requester {
                    Some(generation) => self.dialog() == Some(generation),
                    None => self.dialog().is_some(),
                };
                if routed {
                    if opened {
                        self.route_listing(listing.dir, listing.entries);
                    } else {
                        self.set_listing(listing.dir, listing.entries);
                    }
                    changed = true;
                }
            }
            IoOutcome::Exported(result) => {
                if let Some(export) = &mut self.export {
                    if result.is_ok()
                        && self
                            .notice
                            .as_ref()
                            .is_some_and(|notice| matches!(notice.source, Source::Export))
                    {
                        self.notice = None;
                        changed = true;
                    }
                    changed |= export.complete(result.as_ref().map(|_| ()));
                }
                if let Err(error) = result {
                    changed |= self.report_notice(Notice {
                        source: Source::Export,
                        text: error.to_string(),
                    });
                }
            }
            IoOutcome::Updated(result) => {
                let Some(()) = self.library.complete(done.tag, result, Instant::now()) else {
                    return false;
                };
                // A failure or a newly blocked save shows through the persistent fallback.
                if self.library.notice().is_some() {
                    self.notice = None;
                }
                changed = true;
            }
            IoOutcome::Opened(Err(error)) | IoOutcome::Listed(Err(error)) => {
                changed |= self.set_notice(error.to_string());
            }
        }
        changed
    }

    fn next_token(&mut self) -> u64 {
        self.last_token += 1;
        self.last_token
    }

    // Generation of the open file dialog, if one is open.
    fn dialog(&self) -> Option<u64> {
        match &self.modal {
            Some(Modal::OpenFile(picker)) => Some(picker.generation),
            _ => None,
        }
    }

    fn open_dialog(&mut self, start: PathBuf) {
        let generation = self.next_token();
        self.show_modal(Modal::open_file(start, generation));
    }

    fn show_modal(&mut self, modal: Modal) {
        self.modal = Some(modal);
        self.last_drag = None;
    }

    /// Queues an image load and shows its file name while loading.
    pub fn request_open(&mut self, path: PathBuf, dir: Option<PathBuf>) {
        let token = self.next_token();
        self.open_request = Some(OpenRequest {
            token,
            dialog: self.dialog(),
            label: format!(
                "Loading {}",
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
            ),
        });
        self.notice = None;
        let job = Job::Open {
            path,
            dir,
            protect: self
                .export
                .as_ref()
                .map(|export| export.path().to_path_buf()),
        };
        self.outbox.push((job, token));
    }

    fn request_list(&mut self, path: PathBuf) {
        let tag = self.dialog().unwrap_or(0);
        self.outbox.push((Job::List(path), tag));
    }

    /// Inserts pasted text into the active field or requests a dropped image.
    pub fn handle_paste(&mut self, text: &str) {
        if let Some(modal) = &mut self.modal {
            if let Modal::OpenFile(picker) = modal
                && let Some(path) = rthid_io::paste::dropped_path(text)
            {
                picker.input =
                    rthid_widgets::TextInput::new(path.to_string_lossy(), PATH_MAX_CHARS);
                picker.typed = true;
                return;
            }
            if let Some(input) = modal.text_input_mut() {
                input.insert_str(text);
                if let Modal::OpenFile(picker) = modal {
                    picker.typed = true;
                }
            }
        } else if let Some(path) = rthid_io::paste::dropped_path(text) {
            self.request_open(path, None);
        }
    }

    /// Card grid dimensions, known before the next cache refresh.
    pub fn grid_size(&self) -> (u32, u32) {
        self.grid_cache
            .placement(&self.settings)
            .map_or((0, 0), |p| (p.width, p.height))
    }

    /// Rebuilds the grid and requested layers, sends a new dither to the
    /// companion PNG and clamps the viewport to the preview area. Call before each draw.
    /// Returns whether the grid, dither or mass study was rebuilt.
    pub fn sync(&mut self, preview: Rect) -> bool {
        let versions = |cache: &GridCache| {
            (
                cache.grid.version(),
                cache.dithered.version(),
                cache.mass_version(),
            )
        };
        let before = versions(&self.grid_cache);
        let inset_mass = self.mass_minimap && self.geometry(preview).minimap.is_some();
        self.refresh(inset_mass);
        self.clamp_viewport(preview);
        self.minimap_visible = self.geometry(preview).minimap.is_some();
        before != versions(&self.grid_cache)
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

    fn refresh(&mut self, inset_mass: bool) {
        let need_dither = self.layer == Layer::Dithered || self.has_export();
        let tones = self
            .mass_tones
            .unwrap_or_else(|| mass::tones_from_palette(&self.settings.palette, &self.mass));
        let changed = self.grid_cache.refresh(
            &self.settings,
            need_dither,
            self.layer == Layer::Mass || inset_mass,
            &self.mass,
            &tones,
        );
        let source = self.source_size();
        if self.grid_cache.placement(&self.settings).is_some()
            && let Some(export) = &mut self.export
        {
            let (w, h) = self.grid_cache.grid().expect("processed grid").dimensions();
            let job = export.size(w, h, &self.settings, source).and_then(|size| {
                if !changed && export.png_size == Some(size) {
                    return Ok(None);
                }
                export
                    .job(
                        self.grid_cache.dithered().expect("dithered grid"),
                        w,
                        h,
                        &self.settings,
                        size,
                    )
                    .map(Some)
            });
            match job {
                Ok(Some(job)) => self.outbox.push((job, 0)),
                Ok(None) => {}
                Err(error) => {
                    export.complete(Err(&error));
                }
            }
        }
    }

    /// Whether companion PNG output needs every new dither while Original is shown.
    pub fn has_export(&self) -> bool {
        self.export.is_some()
    }

    /// Physical companion PNG resolution, when `--dpi` was given.
    pub fn export_dpi(&self) -> Option<u32> {
        self.export.as_ref().and_then(Export::dpi)
    }

    /// Latest companion PNG error for the status line.
    pub fn export_status(&self) -> Option<&str> {
        self.export.as_ref().and_then(Export::status)
    }

    /// Selected preview pixels after sync, or None without a source.
    ///
    /// Panics if the selected processed layer was never built.
    pub fn displayed(&self) -> Option<Pixels<'_>> {
        let grid = self.grid_cache.grid()?;
        Some(match self.layer {
            Layer::Original => Pixels::Rgb {
                image: grid,
                version: self.grid_cache.grid_version(),
                layer: Layer::Original,
            },
            Layer::Dithered => Pixels::Indexed {
                indices: self.grid_cache.dithered().expect("dithered grid"),
                palette: &self.settings.palette,
            },
            Layer::Mass => Pixels::Rgb {
                image: self.grid_cache.mass().expect("mass grid"),
                version: self.grid_cache.mass_version(),
                layer: Layer::Mass,
            },
        })
    }

    /// Minimap pixels after sync, or None without a source.
    pub fn inset(&self) -> Option<Pixels<'_>> {
        if self.mass_minimap
            && let Some(image) = self.grid_cache.mass()
        {
            return Some(Pixels::Rgb {
                image,
                version: self.grid_cache.mass_version(),
                layer: Layer::Mass,
            });
        }
        self.displayed()
    }

    /// Queues the final export including changes received with quit.
    pub fn finish(&mut self) {
        self.refresh(false);
    }

    /// Returns the last companion PNG write result.
    pub fn export_result(&self) -> std::io::Result<()> {
        self.export.as_ref().map_or(Ok(()), Export::result)
    }

    /// Whether a left mouse drag has panned the preview.
    pub fn dragging(&self) -> bool {
        self.last_drag.is_some_and(|drag| drag.moved)
    }

    /// Routes mouse input and reports whether the display needs redrawing.
    pub fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        geometry: &PreviewGeometry,
        screen: Rect,
    ) -> bool {
        let was_dragging = self.dragging();
        if let Some(mut modal) = self.modal.take() {
            self.last_drag = None;
            let ctx = self.modal_ctx();
            let before = modal.selection(&ctx);
            let layout = modal.layout(geometry.area, screen, &ctx);
            let step = modal.handle_mouse(mouse, &layout, ctx);
            let changed = before != modal.selection(&self.modal_ctx())
                || step.effect.is_some()
                || !matches!(step.next, Next::Stay);
            self.modal = Some(modal);
            self.apply_step(step);
            return was_dragging || changed;
        }
        if self.source_path().is_none() {
            self.last_drag = None;
            return was_dragging;
        }
        let before = (self.viewport.x, self.viewport.y);
        let prev = self.last_drag.take();
        let point = (mouse.column, mouse.row);
        if !geometry.overflows() || !geometry.area.contains(point.into()) {
            self.last_drag = prev.filter(|drag| {
                drag.minimap
                    && geometry.overflows()
                    && matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left))
            });
            return was_dragging;
        }
        let (dx, dy) = geometry.zoom.dots_per_cell(geometry.grid, geometry.area);
        let (sx, sy) = ((dx * 6.0).round() as i64, (dy * 6.0).round() as i64);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left) => {
                if let Some(minimap) = geometry.minimap
                    && minimap.map.contains(point.into())
                    && !(matches!(mouse.kind, MouseEventKind::Drag(_))
                        && prev.is_some_and(|drag| !drag.minimap))
                {
                    self.viewport.center_on_minimap(point, geometry);
                    self.last_drag = Some(Drag {
                        point,
                        origin: geometry.origin,
                        moved: false,
                        minimap: true,
                    });
                } else if matches!(mouse.kind, MouseEventKind::Down(_))
                    && geometry
                        .minimap
                        .is_some_and(|minimap| minimap.frame.contains(point.into()))
                {
                    return was_dragging;
                } else if matches!(mouse.kind, MouseEventKind::Drag(_))
                    && prev.is_some_and(|drag| drag.minimap)
                {
                    self.last_drag = prev;
                } else {
                    let mut drag = Drag {
                        point,
                        origin: geometry.origin,
                        moved: false,
                        minimap: false,
                    };
                    if matches!(mouse.kind, MouseEventKind::Drag(_))
                        && let Some(anchor) = prev
                    {
                        drag = anchor;
                        let (x, y) = drag.point;
                        let anchored = PreviewGeometry {
                            origin: drag.origin,
                            ..*geometry
                        };
                        let before = (self.viewport.x, self.viewport.y);
                        self.viewport.pan(
                            (f64::from(i32::from(x) - i32::from(mouse.column)) * dx).round() as i64,
                            (f64::from(i32::from(y) - i32::from(mouse.row)) * dy).round() as i64,
                            &anchored,
                        );
                        drag.moved |= before != (self.viewport.x, self.viewport.y);
                    }
                    self.last_drag = Some(drag);
                }
            }
            MouseEventKind::ScrollUp => self.viewport.pan(0, -sy, geometry),
            MouseEventKind::ScrollDown => self.viewport.pan(0, sy, geometry),
            // Trackpads send horizontal swipes as sideways scrolls.
            MouseEventKind::ScrollLeft => self.viewport.pan(-sx, 0, geometry),
            MouseEventKind::ScrollRight => self.viewport.pan(sx, 0, geometry),
            _ => {}
        }
        before != (self.viewport.x, self.viewport.y) || was_dragging != self.dragging()
    }

    /// Borrows the preview and saved entries needed by dialogs.
    pub fn modal_ctx(&self) -> Ctx<'_> {
        Ctx {
            palettes: &self.library.palettes,
            templates: &self.library.templates,
            settings: &self.settings,
            layer: self.layer,
        }
    }

    fn apply_step(&mut self, step: Step) {
        if let Some(effect) = step.effect {
            self.notice = None;
            if let Err(error) = self.apply(effect) {
                self.set_notice(error);
                return;
            }
        }
        match step.next {
            Next::Stay => {}
            Next::Close => self.modal = None,
            Next::Replace(modal) => self.show_modal(modal),
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
        if let Some(mut modal) = self.modal.take() {
            let step = modal.handle_key(key, self.modal_ctx());
            self.modal = Some(modal);
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
            self.open_dialog(start.clone());
            self.request_list(start);
            return;
        }
        if key.code == KeyCode::Char('P') {
            self.show_modal(Modal::presets(&self.settings, &self.library.palettes));
            return;
        }
        if key.code == KeyCode::Char('T') {
            self.show_modal(Modal::templates());
            return;
        }
        if key.code == KeyCode::Tab {
            self.preview_focus = !self.preview_focus;
            return;
        }
        let preview_key = self.preview_key(key.code, geometry);
        if self.source_path().is_none() && preview_key.is_some() {
            return;
        }
        // Esc leaves preview focus. Only q or Ctrl+C exits.
        if key.code == KeyCode::Esc {
            self.preview_focus = false;
            return;
        }
        if let Some(preview_key) = preview_key {
            match preview_key {
                PreviewKey::ToggleOriginal => {
                    self.layer = if self.layer == Layer::Dithered {
                        Layer::Original
                    } else {
                        Layer::Dithered
                    };
                }
                PreviewKey::ToggleMass => {
                    if self.layer == Layer::Mass {
                        self.layer = self.mass_return;
                    } else {
                        self.mass_return = self.layer;
                        self.layer = Layer::Mass;
                    }
                }
                PreviewKey::ToggleMinimap => {
                    self.viewport.hide_minimap = !self.viewport.hide_minimap;
                }
                PreviewKey::ToggleMinimapMass => {
                    self.mass_minimap = !self.mass_minimap;
                }
                PreviewKey::Zoom(zoom) => {
                    self.viewport.set_zoom(zoom, geometry);
                    self.last_drag = None;
                }
                PreviewKey::Pan(dx, dy) => self.viewport.pan(dx, dy, geometry),
            }
            return;
        }
        match self.panel.handle_key(key, Setting::ALL.len()) {
            Outcome::Ignored => {}
            Outcome::Submit(_) => {
                let setting = Setting::ALL[self.panel.selected(Setting::ALL.len()).unwrap()];
                // Enter picks a new seed on an active Seed row. The clock is read only
                // here, Settings decides the seed and the engine stays deterministic.
                let entropy = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                self.apply_setting(setting, self.settings.reseeded(setting, entropy));
                return;
            }
            Outcome::Consumed | Outcome::Cancel => return,
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('f') => self.settings.flip = !self.settings.flip,
            KeyCode::Char('r') => self.settings.rotation = self.settings.rotation.cw(),
            KeyCode::Char('R') => self.settings.rotation = self.settings.rotation.ccw(),
            KeyCode::Char('d') => {
                let next = self.settings.with_default(
                    Setting::ALL[self.panel.selected(Setting::ALL.len()).unwrap()],
                    &self.library.palettes,
                );
                let _ = self.set_settings(next, self.layer);
            }
            KeyCode::Char('D') => self.settings = Settings::default(),
            KeyCode::Left | KeyCode::Char('h') => self.adjust(-1),
            KeyCode::Right | KeyCode::Char('l') => self.adjust(1),
            _ => {}
        }
    }

    fn preview_key(&self, code: KeyCode, geometry: &PreviewGeometry) -> Option<PreviewKey> {
        match code {
            KeyCode::Char(' ') => Some(PreviewKey::ToggleOriginal),
            KeyCode::Char('a') => Some(PreviewKey::ToggleMass),
            KeyCode::Char('m') => Some(PreviewKey::ToggleMinimap),
            KeyCode::Char('M') => Some(PreviewKey::ToggleMinimapMass),
            KeyCode::Char('0') => Some(PreviewKey::Zoom(Zoom::Fit)),
            KeyCode::Char('+' | '=' | '-') => Some(PreviewKey::Zoom(self.viewport.zoom.step(
                code != KeyCode::Char('-'),
                geometry.grid,
                geometry.area,
            ))),
            _ if self.preview_focus => pan_delta(code, geometry.visible, geometry.pan_step())
                .map(|(dx, dy)| PreviewKey::Pan(dx, dy)),
            _ => None,
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
                let layer = if self.layer == Layer::Mass {
                    Layer::Mass
                } else {
                    Layer::Dithered
                };
                self.set_settings(settings, layer)?;
            }
            Effect::Restore { settings, layer } => {
                self.set_settings(settings, layer)?;
            }
            Effect::SavePreset { preset, base } => {
                let name = preset.name.clone();
                self.library.commit(Change::UpsertPreset(preset))?;
                let preset = self
                    .library
                    .palettes
                    .find(&name)
                    .and_then(|i| self.library.palettes.get(i));
                let settings = base.with_preset(preset.unwrap());
                self.apply(Effect::Preview(settings))?;
            }
            Effect::RemovePreset(name) => self.library.commit(Change::RemovePreset(name))?,
            Effect::SaveTemplate(name) => {
                let template = Template::capture(name, &self.settings);
                self.library.commit(Change::UpsertTemplate(template))?;
            }
            Effect::RemoveTemplate(name) => self.library.commit(Change::RemoveTemplate(name))?,
        }
        Ok(())
    }

    fn adjust(&mut self, direction: i32) {
        let setting = Setting::ALL[self.panel.selected(Setting::ALL.len()).unwrap()];
        let next = self
            .settings
            .adjusted(setting, direction, &self.library.palettes);
        self.apply_setting(setting, next);
    }

    fn set_settings(&mut self, settings: Settings, layer: Layer) -> Result<(), String> {
        settings.dimensions(self.source_size())?;
        self.settings = settings;
        self.layer = layer;
        Ok(())
    }

    // Enter and arrow keys share the preview rule and preserve it for rejected edits.
    // The mass study stays selected because palette edits recolor its tones.
    fn apply_setting(&mut self, setting: Setting, next: Settings) {
        let layer =
            if next != self.settings && setting.shows_dithered() && self.layer != Layer::Mass {
                Layer::Dithered
            } else {
                self.layer
            };
        let _ = self.set_settings(next, layer);
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
fn mouse_event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        settings::{BUSINESS_CARD, Dither, Rotation, Templates},
        ui::Layout,
    };
    use ratatui::layout::Rect;
    use rthid_dither::dither::Mode;

    fn business_app(source: RgbaImage, preview_png: Option<PathBuf>) -> App {
        let mut app = App::new(None, preview_png, None, Loaded::default());
        app.settings.card = BUSINESS_CARD;
        app.open_image("test.png".into(), source);
        app
    }

    fn opened_photo() -> IoOutcome {
        IoOutcome::Opened(Ok(Opened::Image {
            path: "photo.png".into(),
            image: RgbaImage::new(2, 3),
        }))
    }

    // Echoes the tag of the newest matching request, as the worker would.
    fn done(app: &App, outcome: IoOutcome) -> Done {
        let tag = match outcome {
            IoOutcome::Opened(_) => app.open_request.as_ref().map_or(0, |r| r.token),
            IoOutcome::Listed(_) => app.dialog().unwrap_or(0),
            _ => app.last_token,
        };
        Done::new(tag, std::time::Duration::ZERO, outcome)
    }

    // Drains unsaved edits as a save that succeeded would.
    fn take_changes(app: &mut App) -> Vec<Change> {
        let changes = app.library.unsaved();
        if !changes.is_empty() {
            let (tag, path, edit) = queued_save(app);
            let text = edit.apply(None).unwrap();
            app.complete(save_done(tag, Ok(rthid_io::io::Updated { path, text })));
        }
        changes
    }

    fn jobs(app: &mut App) -> Vec<Job> {
        app.take_jobs().into_iter().map(|(job, _)| job).collect()
    }

    #[test]
    fn image_export_updates_for_dot_size_and_rotation() {
        let mut app = App::new(
            Some(("test.png".into(), RgbaImage::new(30, 20))),
            Some("preview.png".into()),
            Some(254),
            Loaded::default(),
        );
        assert_eq!(
            encoded_export(jobs(&mut app).pop().unwrap()).1.dimensions(),
            (60, 40)
        );
        app.layer = Layer::Mass;
        app.refresh(false);
        assert!(jobs(&mut app).is_empty());
        let before = versions(&app);
        let mass_version = app.grid_cache.mass_version();
        app.settings.dot = 40;
        app.refresh(false);
        assert_eq!(versions(&app), before);
        assert_eq!(app.grid_cache.mass_version(), mass_version);
        assert_eq!(app.layer, Layer::Mass);
        assert_eq!(app.grid_size(), (30, 20));
        assert_eq!(
            encoded_export(jobs(&mut app).pop().unwrap()).1.dimensions(),
            (120, 80)
        );
        app.settings.rotation = Rotation::R90;
        app.refresh(false);
        assert_eq!(app.grid_size(), (20, 30));
        assert_eq!(
            encoded_export(jobs(&mut app).pop().unwrap()).1.dimensions(),
            (80, 120)
        );
        app.settings.margin = 20;
        app.settings.bleed = 3;
        app.refresh(false);
        assert_eq!(
            encoded_export(jobs(&mut app).pop().unwrap()).1.dimensions(),
            (184, 224)
        );
        app.refresh(false);
        assert!(jobs(&mut app).is_empty());

        let mut app = App::builtin(RgbaImage::new(30, 20), Some("preview.png".into()));
        jobs(&mut app);
        let before = versions(&app);
        app.settings.dot = 40;
        app.refresh(false);
        assert_eq!(versions(&app), before);
        assert!(jobs(&mut app).is_empty());
    }

    #[test]
    fn export_size_snapshots_use_preview_or_physical_dimensions() {
        for (dpi, expected) in [(None, (1000, 1800)), (Some(600), (1181, 2126))] {
            let mut app = App::new(None, Some("preview.png".into()), dpi, Loaded::default());
            app.settings.card = BUSINESS_CARD;
            app.open_image("test.png".into(), RgbaImage::new(4, 4));
            let job = jobs(&mut app).pop().unwrap();
            app.settings.bleed = 3;
            assert_eq!(encoded_export(job).1.dimensions(), expected);
            app.refresh(false);
            let expected = if dpi.is_some() {
                (1323, 2268)
            } else {
                (1120, 1920)
            };
            assert_eq!(
                encoded_export(jobs(&mut app).pop().unwrap()).1.dimensions(),
                expected
            );
        }
    }

    #[test]
    fn oversized_png_is_reported_before_submission() {
        let mut app = App::new(
            None,
            Some("preview.png".into()),
            Some(4800),
            Loaded::default(),
        );
        app.settings.card = crate::settings::Card::from_id("postcard");
        app.open_image("test.png".into(), RgbaImage::new(4, 4));
        assert!(jobs(&mut app).is_empty());
        assert_eq!(
            app.export_status(),
            Some("PNG preview.png: output size 18898x27969 exceeds 268435456 pixels")
        );
        assert!(app.export_result().is_err());
    }

    fn encoded_export(job: Job) -> (PathBuf, image::RgbaImage) {
        let Job::Export { path, write } = job else {
            panic!("expected export");
        };
        let mut file = tempfile::tempfile().unwrap();
        write(&mut file).unwrap();
        use std::io::{Read, Seek};
        file.rewind().unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        (path, image::load_from_memory(&bytes).unwrap().to_rgba8())
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
        use rthid_io::{dir::Listing, io::Op};
        let listing = || Listing {
            dir: "/images".into(),
            entries: vec![],
        };
        let mut app = App::empty();
        app.settings.card = BUSINESS_CARD;
        app.set_notice("retained".into());
        assert!(!app.complete(done(&app, IoOutcome::Listed(Ok(listing())))));
        assert_eq!(app.notice(), Some("retained"));
        assert!(app.modal.is_none());
        app.request_open("/images".into(), None);
        assert!(app.complete(done(&app, IoOutcome::Opened(Ok(Opened::Dir(listing()))))));
        assert!(app.notice().is_none());
        let Some(Modal::OpenFile(picker)) = &app.modal else {
            panic!("open dialog expected")
        };
        assert_eq!(picker.dir, PathBuf::from("/images"));
        for outcome in [
            IoOutcome::Opened(Err(failure(Op::Decode))),
            IoOutcome::Listed(Err(failure(Op::List))),
        ] {
            if matches!(outcome, IoOutcome::Opened(_)) {
                app.request_open("failed.png".into(), None);
            }
            let expected = match &outcome {
                IoOutcome::Opened(Err(error)) | IoOutcome::Listed(Err(error)) => error.to_string(),
                _ => unreachable!(),
            };
            assert!(app.complete(done(&app, outcome)));
            assert_eq!(app.notice(), Some(expected.as_str()));
            assert!(matches!(app.modal, Some(Modal::OpenFile(_))));
        }
        assert!(app.complete(done(&app, IoOutcome::Listed(Ok(listing())))));
        assert!(app.notice().is_none());
        app.request_open("photo.png".into(), None);
        assert!(app.complete(done(&app, opened_photo())));
        assert!(app.modal.is_none());
        assert!(app.notice().is_none());
        assert_eq!(app.source_path(), Some(std::path::Path::new("photo.png")));
        assert_eq!(app.grid_size(), (250, 450));
    }

    #[test]
    fn late_opens_preserve_dialogs_and_editor_restore_state() {
        use rthid_io::dir::Listing;
        let listing = || {
            IoOutcome::Opened(Ok(Opened::Dir(Listing {
                dir: "/images".into(),
                entries: vec![],
            })))
        };
        let image = || {
            IoOutcome::Opened(Ok(Opened::Image {
                path: "late.png".into(),
                image: RgbaImage::new(2, 3),
            }))
        };
        let mut app = App::empty();
        let original = app.settings.clone();
        app.request_open("/images".into(), None);
        for key in [KeyCode::Char('P'), KeyCode::Char('n')] {
            press(&mut app, key);
        }
        assert!(matches!(app.modal, Some(Modal::Editor(_))));
        press(&mut app, KeyCode::Enter);
        app.handle_paste("draft");
        assert_eq!(app.notice(), Some("Loading images"));
        assert!(app.complete(done(&app, listing())));
        let Some(Modal::Editor(editor)) = &app.modal else {
            panic!("editor expected")
        };
        assert_eq!(editor.input.as_ref().unwrap().text(), "draft");
        assert!(app.notice().is_none());
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert!(app.settings == original);
        assert_eq!(app.layer, Layer::Original);

        for outcome in [listing(), image()] {
            app.open_dialog("/images".into());
            app.request_open("late.png".into(), Some("/images".into()));
            press(&mut app, KeyCode::Esc);
            assert_eq!(app.notice(), Some("Loading late.png"));
            app.complete(done(&app, outcome));
            assert!(app.modal.is_none());
        }
        app.request_open("early.png".into(), None);
        let early = done(&app, image());
        app.request_open("late.png".into(), None);
        assert!(!app.complete(early));
        assert_eq!(app.notice(), Some("Loading late.png"));
        app.open_dialog("/images".into());
        assert!(app.complete(done(&app, image())));
        assert!(matches!(app.modal, Some(Modal::OpenFile(_))));
    }

    #[test]
    fn results_for_a_closed_dialog_leave_a_reopened_dialog_alone() {
        use rthid_io::dir::Listing;
        let reply = |tag, outcome| Done::new(tag, std::time::Duration::ZERO, outcome);
        let listing = |dir: &str| Listing {
            dir: dir.into(),
            entries: vec![Entry {
                file_name: "a.png".into(),
                name: "a.png".into(),
                dir: false,
            }],
        };
        let mut app = App::empty();
        press(&mut app, KeyCode::Char('o'));
        app.apply(Effect::ListDir("/first".into())).unwrap();
        app.apply(Effect::Open("a.png".into(), "/first".into()))
            .unwrap();
        let tags: Vec<u64> = app.take_jobs().into_iter().map(|(_, tag)| tag).collect();
        let [_, listed, opened] = tags[..] else {
            panic!("list, list and open expected")
        };
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('o'));
        let before = app.dialog();
        assert!(before.is_some_and(|generation| generation != listed));
        assert!(!app.complete(reply(listed, IoOutcome::Listed(Ok(listing("/first"))))));
        assert!(app.complete(reply(
            opened,
            IoOutcome::Opened(Ok(Opened::Image {
                path: "/first/a.png".into(),
                image: RgbaImage::new(2, 3),
            })),
        )));
        assert!(app.open_request.is_none());
        assert_eq!(
            app.source_path(),
            Some(std::path::Path::new("/first/a.png"))
        );
        assert_eq!(app.dialog(), before);
        let Some(Modal::OpenFile(picker)) = &app.modal else {
            panic!("open dialog expected")
        };
        assert_eq!(picker.dir, PathBuf::from("."));
        assert!(picker.entries.is_empty());
    }

    #[test]
    fn pasted_text_respects_field_limits_and_marks_the_path_as_typed() {
        let mut app = App::empty();
        app.open_dialog("/images".into());
        app.handle_paste(&"猫\n".repeat(100_000));
        let Some(Modal::OpenFile(picker)) = &app.modal else {
            panic!("open dialog expected")
        };
        assert!(picker.typed);
        assert_eq!(picker.input.text().chars().count(), PATH_MAX_CHARS);
        app.handle_paste(&format!("/{}.png", "a".repeat(5000)));
        assert_eq!(
            app.modal
                .as_mut()
                .unwrap()
                .text_input_mut()
                .unwrap()
                .text()
                .chars()
                .count(),
            PATH_MAX_CHARS
        );
    }

    #[test]
    fn export_jobs_protect_the_destination_and_retain_errors_until_success() {
        use rthid_io::io::Op;
        let path = PathBuf::from("preview.png");
        let mut app = business_app(RgbaImage::new(4, 2), Some(path.clone()));
        app.request_open("photo.png".into(), Some("/images".into()));
        let mut queued = jobs(&mut app);
        let (output, image) = encoded_export(queued.remove(0));
        assert_eq!(output, path);
        assert_eq!(image.dimensions(), (1000, 1800));
        assert!(
            image
                .pixels()
                .all(|pixel| pixel.0[..3] == app.settings.palette[0].rgb)
        );
        assert!(
            matches!(queued.as_slice(), [Job::Open { path: source, dir: Some(dir), protect: Some(protect) }]
            if protect == &path && source == &PathBuf::from("photo.png") && dir == &PathBuf::from("/images"))
        );
        app.complete(done(&app, IoOutcome::Opened(Err(failure(Op::Decode)))));
        assert!(app.complete(done(&app, IoOutcome::Exported(Err(failure(Op::Encode))))));
        let error = app.export_status().unwrap().to_owned();
        assert_eq!(app.notice(), Some(error.as_str()));
        assert!(app.export_result().is_err());
        app.settings.rotation = Rotation::R90;
        app.finish();
        assert!(matches!(jobs(&mut app).as_slice(), [Job::Export { .. }]));
        assert_eq!(app.export_status(), Some(error.as_str()));
        assert!(!app.complete(done(&app, IoOutcome::Exported(Err(failure(Op::Encode))))));
        let mut timeout = failure(Op::Encode);
        timeout.cause = rthid_io::io::Cause::TimedOut;
        assert!(app.complete(done(&app, IoOutcome::Exported(Err(timeout)))));
        assert!(
            app.export_result()
                .unwrap_err()
                .to_string()
                .contains("did not finish before quitting")
        );
        assert!(app.complete(done(&app, IoOutcome::Exported(Ok(path.clone())))));
        assert!(app.export_status().is_none());
        assert!(app.notice().is_none());
        assert!(app.export_result().is_ok());
        assert!(!app.complete(done(&app, IoOutcome::Exported(Ok(path)))));
    }

    #[test]
    fn sync_reports_only_grid_or_dither_rebuilds() {
        let mut app = business_app(RgbaImage::new(4, 2), None);
        let preview = test_layout().preview;
        assert!(!app.sync(preview));
        app.settings.dot = 10;
        assert!(app.sync(preview));
        assert!(!app.sync(preview));
        app.layer = Layer::Dithered;
        assert!(app.sync(preview));
        assert!(!app.sync(preview));
        app.settings.strength = 0.5;
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
                matches!(jobs(&mut app).as_slice(), [Job::List(path)] if path == &PathBuf::from(dir))
            );
            app.set_listing(
                PathBuf::from(dir),
                vec![Entry {
                    file_name: "bad.png".into(),
                    name: "bad.png".into(),
                    dir: false,
                }],
            );
            press(&mut app, KeyCode::Enter);
            assert!(
                matches!(jobs(&mut app).as_slice(), [Job::Open { path, dir: Some(base), protect: None }]
                if path == &PathBuf::from("bad.png") && base == &PathBuf::from(dir))
            );
            app.complete(done(
                &app,
                IoOutcome::Opened(Err(failure(rthid_io::io::Op::Decode))),
            ));
            assert!(matches!(app.modal, Some(Modal::OpenFile(_))));
            assert!(app.notice().unwrap().starts_with("open:"));
            app.handle_paste("'file:///tmp/My%20Photo.png'");
            assert!(jobs(&mut app).is_empty());
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
                    matches!(jobs(&mut app).as_slice(), [Job::Open { path: entered, dir: Some(base), protect: None }]
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
        assert!(jobs(&mut app).is_empty());
        press(&mut app, KeyCode::Esc);
        app.handle_paste("'/tmp/ignored.jpg'");
        assert!(jobs(&mut app).is_empty());

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
        assert!(jobs(&mut app).is_empty());
        app.handle_paste("'/tmp/My Photo.jpg'");
        assert_eq!(app.notice(), Some("Loading My Photo.jpg"));
        press(&mut app, KeyCode::Char('o'));
        app.apply(Effect::ListDir(PathBuf::from("/other"))).unwrap();
        assert!(matches!(jobs(&mut app).as_slice(), [
            Job::Open { path, dir: None, protect: None }, Job::List(_), Job::List(listed)
        ] if path == &PathBuf::from("/tmp/My Photo.jpg") && listed == &PathBuf::from("/other")));
        assert!(jobs(&mut app).is_empty());
    }

    #[test]
    fn empty_preview_ignores_navigation_but_keeps_panel_edits() {
        let mut app = App::empty();
        app.settings.card = BUSINESS_CARD;
        assert!(app.source_path().is_none());
        assert!(app.displayed().is_none());
        assert_eq!(app.grid_size(), (0, 0));
        assert!(app.grid_cache.source.is_none());
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
                mouse_event(MouseEventKind::Down(MouseButton::Left), 2, 3),
                &app.geometry(test_layout().preview),
                test_layout().screen,
            );
            assert_eq!(app.viewport.zoom, Zoom::Fit);
            assert_eq!((app.viewport.x, app.viewport.y), (0, 0));
            assert!(!app.viewport.hide_minimap);
            assert!(app.last_drag.is_none());
            assert_eq!(app.layer, Layer::Original);
            assert_eq!(versions(&app), (0, 0));
        }
        app.preview_focus = false;
        press(&mut app, KeyCode::Right);
        assert_eq!(
            app.settings.card,
            crate::settings::Card::from_id("business-landscape")
        );
        assert!(app.displayed().is_none());
        app.open_image(PathBuf::from("first.png"), RgbaImage::new(20, 10));
        assert_eq!(app.grid_size(), (450, 250));
        assert!(app.displayed().is_some());
        assert_eq!(app.layer, Layer::Original);
        assert_eq!(versions(&app), (1, 0));
    }

    #[test]
    fn open_image_resets_navigation_and_rewrites_companion() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("preview.png");
        let mut app = business_app(RgbaImage::new(4, 2), Some(output.clone()));
        let (path, image) = encoded_export(jobs(&mut app).pop().unwrap());
        assert_eq!(path, output);
        assert_eq!(image.dimensions(), (1000, 1800));
        app.settings = app
            .settings
            .with_preset(app.library.palettes.get(1).unwrap());
        app.settings.tone.brightness = 0.2;
        app.settings.rotation = Rotation::R90;
        app.settings.zoom = 150;
        app.settings.pan_x = 25;
        app.settings.pan_y = -40;
        app.settings.flip = true;
        app.layer = Layer::Dithered;
        app.refresh(false);
        let settings = app.settings.clone();
        let before = versions(&app);
        app.viewport.zoom = Zoom::In(2);
        app.viewport.x = 1;
        app.viewport.hide_minimap = true;
        app.last_drag = Some(Drag {
            point: (3, 4),
            origin: (1, 0),
            moved: true,
            minimap: false,
        });
        let path = PathBuf::from("replacement.png");
        app.open_image(path.clone(), RgbaImage::new(3, 5));
        assert_eq!(app.source_path(), Some(path.as_path()));
        assert_eq!(app.viewport.zoom, Zoom::Fit);
        assert_eq!((app.viewport.x, app.viewport.y), (0, 0));
        assert!(!app.viewport.hide_minimap);
        assert!(app.last_drag.is_none() && app.layer == Layer::Original);
        assert_eq!(app.settings.rotation, Rotation::R0);
        assert!(!app.settings.flip);
        assert_eq!(
            (app.settings.zoom, app.settings.pan_x, app.settings.pan_y),
            (100, 0, 0)
        );
        assert_eq!(app.settings.options(), settings.options());
        assert_eq!(app.settings.palette_hex, settings.palette_hex);
        assert_eq!(app.settings.palette_name, settings.palette_name);
        assert_eq!(app.grid_size(), (250, 450));
        assert_eq!(versions(&app), (before.0 + 1, before.1 + 1));
        let (path, image) = encoded_export(jobs(&mut app).pop().unwrap());
        assert_eq!(path, output);
        assert_eq!(image.dimensions(), (1000, 1800));
    }

    #[test]
    fn template_manager_saves_and_applies_settings() {
        let mut app = app_at(Setting::Strength);
        app.settings.strength = 0.5;
        let saved = app.settings.clone();
        for code in [
            KeyCode::Char('T'),
            KeyCode::Char('n'),
            KeyCode::Char('Z'),
            KeyCode::Enter,
        ] {
            press(&mut app, code);
        }
        assert!(app.modal.is_none() && !take_changes(&mut app).is_empty());
        assert_eq!(
            app.library.templates.get(0),
            Some(&Template::capture("Z".into(), &saved))
        );
        app.settings.strength = 0.9;
        app.settings.flip = true;
        app.settings.rotation = Rotation::R90;
        for code in [KeyCode::Char('T'), KeyCode::Enter] {
            press(&mut app, code);
        }
        assert_eq!(app.settings.strength, 0.5);
        assert!(app.settings.flip);
        assert_eq!(app.settings.rotation, Rotation::R90);
        assert!(
            app.layer == Layer::Dithered
                && app.modal.is_none()
                && take_changes(&mut app).is_empty()
        );
    }

    #[test]
    fn preset_editor_saves_and_keeps_failed_save_open() {
        let mut app = app_at(Setting::Palette);
        for code in [KeyCode::Char('P'), KeyCode::Char('n'), KeyCode::Char('s')] {
            press(&mut app, code);
        }
        assert!(app.notice().unwrap().contains("must not be empty"));
        assert!(app.modal.is_some() && take_changes(&mut app).is_empty());
        press(&mut app, KeyCode::Down);
        app.handle_key(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            &app.geometry(test_layout().preview),
        );
        assert!(
            matches!(&app.modal, Some(Modal::Editor(editor)) if editor.draft.colors == app.settings.palette_hex && editor.rows.selected(editor.draft.colors.len() + 2) == Some(1) && editor.input.is_none())
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
        assert!(app.library.palettes.find("Z").is_some());
        assert!(
            app.modal.is_none()
                && !take_changes(&mut app).is_empty()
                && app.layer == Layer::Dithered
        );
    }

    // Takes the queued save job with the tag its result echoes.
    fn queued_save(app: &mut App) -> (u64, PathBuf, Box<dyn rthid_io::io::Edit>) {
        app.queue_save(Path::new("config.toml"), Instant::now());
        match app.take_jobs().pop() {
            Some((Job::Update { path, edit, .. }, tag)) => (tag, path, edit),
            _ => panic!("save job expected"),
        }
    }

    fn save_done(tag: u64, result: Result<rthid_io::io::Updated, rthid_io::io::IoError>) -> Done {
        Done::new(tag, std::time::Duration::ZERO, IoOutcome::Updated(result))
    }

    fn saved(app: &mut App, text: &str) {
        app.library
            .commit(Change::RemoveTemplate("Absent".into()))
            .unwrap();
        let (tag, path, _) = queued_save(app);
        app.complete(save_done(
            tag,
            Ok(rthid_io::io::Updated {
                path,
                text: text.into(),
            }),
        ));
    }

    fn save_failed(app: &mut App) {
        app.library
            .commit(Change::RemoveTemplate("Absent".into()))
            .unwrap();
        let (tag, path, _) = queued_save(app);
        let error = rthid_io::io::IoError {
            op: rthid_io::io::Op::Save,
            path,
            cause: rthid_io::io::Cause::Io(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "config.toml: file is busy",
            )),
        };
        app.complete(save_done(tag, Err(error)));
    }

    #[test]
    fn notice_regression_validation_resolved_during_loading() {
        let mut app = App::empty();
        app.handle_paste("/photo.png");
        for code in [KeyCode::Char('P'), KeyCode::Char('n'), KeyCode::Char('s')] {
            press(&mut app, code);
        }
        for code in [
            KeyCode::Enter,
            KeyCode::Char('Z'),
            KeyCode::Enter,
            KeyCode::Char('s'),
        ] {
            press(&mut app, code);
        }
        assert!(app.library.palettes.find("Z").is_some());
        app.complete(done(&app, opened_photo()));
        assert!(app.notice().is_none());
    }

    #[test]
    fn notice_regression_export_resolved_during_loading() {
        let mut app = App::builtin(RgbaImage::new(2, 3), Some("preview.png".into()));
        app.request_open("photo.png".into(), None);
        app.complete(done(
            &app,
            IoOutcome::Exported(Err(failure(rthid_io::io::Op::Encode))),
        ));
        app.complete(done(&app, IoOutcome::Exported(Ok("preview.png".into()))));
        app.complete(done(&app, opened_photo()));
        assert!(app.notice().is_none());
    }

    #[test]
    fn notice_regression_listing_resolved_during_loading() {
        let mut app = App::empty();
        press(&mut app, KeyCode::Char('o'));
        app.request_open("photo.png".into(), None);
        app.complete(done(
            &app,
            IoOutcome::Listed(Err(failure(rthid_io::io::Op::List))),
        ));
        app.complete(done(
            &app,
            IoOutcome::Listed(Ok(rthid_io::dir::Listing {
                dir: "/images".into(),
                entries: vec![],
            })),
        ));
        app.complete(done(&app, opened_photo()));
        assert!(app.notice().is_none());
    }

    #[test]
    fn notice_regression_save_failure_survives_replacement_and_input() {
        let mut app = App::empty();
        app.request_open("photo.png".into(), None);
        save_failed(&mut app);
        app.set_notice("validation".into());
        press(&mut app, KeyCode::Char('x'));
        app.complete(done(&app, opened_photo()));
        assert_eq!(app.notice(), Some("config: file is busy"));
        app.set_notice("validation".into());
        save_failed(&mut app);
        assert_eq!(app.notice(), Some("config: file is busy"));
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.notice(), Some("config: file is busy"));
        saved(&mut app, "");
        assert!(app.notice().is_none());
    }

    #[test]
    fn notice_regression_stale_listing_error_is_dropped() {
        let mut app = App::empty();
        press(&mut app, KeyCode::Char('o'));
        let tag = app.dialog().unwrap();
        press(&mut app, KeyCode::Esc);
        for reopen in [false, true] {
            if reopen {
                press(&mut app, KeyCode::Char('o'));
            }
            app.set_notice("retained".into());
            assert!(!app.complete(Done::new(
                tag,
                std::time::Duration::ZERO,
                IoOutcome::Listed(Err(failure(rthid_io::io::Op::List)))
            )));
            assert_eq!(app.notice(), Some("retained"));
        }
    }

    #[test]
    fn modal_pointer_motion_preserves_notice_until_an_effect() {
        let mut app = App::empty();
        app.open_dialog(".".into());
        app.set_notice("open: failed".into());
        let layout = test_layout();
        app.handle_mouse(
            mouse_event(
                MouseEventKind::Moved,
                layout.screen.width / 2,
                layout.screen.height / 2,
            ),
            &app.geometry(layout.preview),
            layout.screen,
        );
        assert!(matches!(app.modal, Some(Modal::OpenFile(_))));
        assert_eq!(app.notice(), Some("open: failed"));
        app.apply_step(Step {
            next: Next::Stay,
            effect: Some(Effect::Preview(app.settings.clone())),
        });
        assert!(app.notice().is_none());
    }

    #[test]
    fn persistent_notices_follow_fallback_order_until_their_own_success() {
        let text = "version = 1\n[[palette]]\nname = 'Bad'\ncolors = ['oops']";
        let loaded = crate::store::parse(text);
        let warning = loaded.notice().unwrap();
        let mut app = App::new(None, Some("preview.png".into()), None, loaded);
        // A save warning can be replaced before the next draw.
        saved(&mut app, text);
        save_failed(&mut app);
        let error = failure(rthid_io::io::Op::Encode);
        let export_error = error.to_string();
        app.complete(done(&app, IoOutcome::Exported(Err(error))));
        assert_eq!(app.notice(), Some(export_error.as_str()));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.notice(), Some("config: file is busy"));
        saved(&mut app, text);
        assert_eq!(app.notice(), Some(warning.as_str()));
        saved(&mut app, "");
        assert_eq!(app.notice(), Some(export_error.as_str()));
        app.complete(done(&app, IoOutcome::Exported(Ok("preview.png".into()))));
        assert!(app.notice().is_none());
    }

    #[test]
    fn export_success_preserves_other_sources_with_equal_text() {
        let mut app = App::new(None, Some("preview.png".into()), None, Loaded::default());
        let error = failure(rthid_io::io::Op::Encode);
        let text = error.to_string();
        app.complete(done(&app, IoOutcome::Exported(Err(error))));
        app.set_notice(text.clone());
        app.complete(done(&app, IoOutcome::Exported(Ok("preview.png".into()))));
        assert_eq!(app.notice(), Some(text.as_str()));
    }

    #[test]
    fn save_notices_preserve_unrelated_errors() {
        let loaded =
            crate::store::parse("version = 1\n[[palette]]\nname = 'Bad'\ncolors = ['oops']\n");
        let mut app = App::new(None, None, None, loaded);
        assert!(app.notice().unwrap().starts_with("config:"));
        saved(&mut app, "");
        assert!(app.notice().is_none());
        save_failed(&mut app);
        assert_eq!(app.notice(), Some("config: file is busy"));
        app.set_notice("open: failed".into());
        save_failed(&mut app);
        assert_eq!(app.notice(), Some("config: file is busy"));
        app.set_notice("open: failed".into());
        saved(&mut app, "");
        assert_eq!(app.notice(), Some("open: failed"));
        save_failed(&mut app);
        assert_eq!(app.notice(), Some("config: file is busy"));
        saved(&mut app, "");
        assert!(app.notice().is_none());
    }

    #[test]
    fn successful_save_clears_only_config_notices_during_loading() {
        for config in [false, true] {
            let mut app = App::empty();
            app.request_open("photo.png".into(), None);
            save_failed(&mut app);
            if !config {
                app.set_notice("open: failed".into());
            }
            saved(&mut app, "");
            assert_eq!(app.notice(), Some("Loading photo.png"));
            app.complete(done(&app, opened_photo()));
            assert_eq!(
                app.notice(),
                if config { None } else { Some("open: failed") }
            );
        }
    }

    #[test]
    fn loading_masks_notices_until_open_finishes() {
        use rthid_io::{dir::Listing, io::Op};
        for notice_kind in 0..3 {
            for result_kind in 0..3 {
                let mut app = App::empty();
                app.set_notice("Loading is only text".into());
                assert!(app.open_request.is_none());
                app.request_open("photo.png".into(), None);
                assert!(app.open_request.is_some());
                app.set_notice("earlier notice".into());
                let expected = match notice_kind {
                    0 => {
                        save_failed(&mut app);
                        save_failed(&mut app);
                        "config: file is busy".to_string()
                    }
                    1 => {
                        app.open_dialog("/images".into());
                        let error = failure(Op::List);
                        let expected = error.to_string();
                        app.complete(done(&app, IoOutcome::Listed(Err(error))));
                        expected
                    }
                    _ => {
                        let loaded = crate::store::parse("version = 99");
                        let expected = loaded.notice().unwrap();
                        saved(&mut app, "version = 99");
                        expected
                    }
                };
                assert_eq!(app.notice(), Some("Loading photo.png"));
                let outcome = match result_kind {
                    0 => opened_photo(),
                    1 => IoOutcome::Opened(Ok(Opened::Dir(Listing {
                        dir: "/images".into(),
                        entries: vec![],
                    }))),
                    _ => IoOutcome::Opened(Err(failure(Op::Decode))),
                };
                let expected = if result_kind == 2 {
                    failure(Op::Decode).to_string()
                } else {
                    expected
                };
                assert!(app.complete(done(&app, outcome)));
                assert!(app.open_request.is_none());
                assert_eq!(app.notice(), Some(expected.as_str()));
            }
        }
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
        app.apply(Effect::SaveTemplate("Zine".into())).unwrap();
        let mut preset_confirm = Modal::Presets {
            list: rthid_widgets::ListPicker::new(0),
            confirm: Some((
                rthid_widgets::Confirm::new("Delete preset?"),
                crate::modal::OnYes::RemovePreset("Selected".into()),
            )),
        };
        let mut template_confirm = Modal::Templates {
            list: rthid_widgets::ListPicker::new(0),
            prompt: None,
            confirm: Some((
                rthid_widgets::Confirm::new("Delete template?"),
                crate::modal::OnYes::RemoveTemplate("Zine".into()),
            )),
        };
        let index = app.library.palettes.find("Selected").unwrap();
        let text = "version = 1\n[[palette]]\nname = 'Other'\ncolors = ['#fff']\n[[palette]]\nname = 'Selected'\ncolors = ['#123456']\n[[template]]\nname = 'Other'\n[[template]]\nname = 'Zine'\n[[palette]]\nname = 'Bad'\ncolors = ['oops']\n";
        let notice = crate::store::parse(text).notice();
        saved(&mut app, text);
        assert_eq!(app.settings.palette_name, "Selected");
        assert!(app.settings == settings);
        assert_eq!(app.library.palettes.find("Selected"), Some(index + 1));
        assert!(app.library.templates.find("Zine").is_some());
        assert_eq!(app.notice(), notice.as_deref());
        assert!(app.notice().unwrap().contains("1 entries skipped"));
        let yes = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        let effect = preset_confirm
            .handle_key(yes, app.modal_ctx())
            .effect
            .unwrap();
        app.apply(effect).unwrap();
        let effect = template_confirm
            .handle_key(yes, app.modal_ctx())
            .effect
            .unwrap();
        app.apply(effect).unwrap();
        assert!(app.library.palettes.find("Other").is_some());
        assert!(app.library.palettes.find("Selected").is_none());
        assert!(app.library.templates.find("Other").is_some());
        assert!(app.library.templates.find("Zine").is_none());
        assert!(take_changes(&mut app).contains(&Change::RemovePreset("Selected".into())));
        saved(&mut app, "");
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
                None,
                Loaded {
                    templates: templates.clone(),
                    save_blocked: blocked.then(|| "broken file".into()),
                    ..Loaded::default()
                },
            );
            app.set_notice("config warning".into());
            assert_eq!(app.library.templates, templates);
            assert_eq!(app.notice(), Some("config warning"));
            assert!(app.library.unsaved().is_empty());
            app.handle_key(
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
                &app.geometry(Rect::new(0, 0, 20, 10)),
            );
            assert_eq!(
                app.notice(),
                blocked.then_some("config: saving disabled: broken file")
            );
            app.apply(Effect::SaveTemplate("Another".into())).unwrap();
            assert_eq!(app.library.unsaved().is_empty(), blocked);
            app.queue_save(Path::new("config.toml"), Instant::now());
            assert_eq!(
                matches!(jobs(&mut app).as_slice(), [Job::Update { .. }]),
                !blocked
            );
            app.queue_save(Path::new("config.toml"), Instant::now());
            assert!(jobs(&mut app).is_empty());
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

    fn select(app: &mut App, setting: Setting) {
        app.panel
            .select(Setting::ALL.iter().position(|&row| row == setting).unwrap());
    }

    fn app_at(setting: Setting) -> App {
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        select(&mut app, setting);
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
        let mut app = business_app(RgbaImage::new(20, 10), None);
        assert_eq!(app.grid_cache.grid.version(), 1);
        assert_eq!(app.grid_cache.dithered.version(), 0);
        app.layer = Layer::Dithered;
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.dithered.version(), 1);
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.grid.version(), 1);
        assert_eq!(app.grid_cache.dithered.version(), 1);
        app.settings.strength = 0.5;
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.grid.version(), 1);
        assert_eq!(app.grid_cache.dithered.version(), 2);
        app.layer = Layer::Original;
        app.settings.rotation = Rotation::R180;
        app.settings.dot = 19;
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.grid.version(), 2);
        assert_eq!(app.grid_cache.dithered.version(), 2);
        app.layer = Layer::Dithered;
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.dithered.version(), 3);
        assert_eq!(app.grid_size(), (263, 474));
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
        for (screen, dot, rotation, expected) in [
            (Rect::new(0, 0, 41, 9), 100, Rotation::R0, (110, 70)),
            (Rect::new(0, 0, 100, 40), 100, Rotation::R0, (51, 8)),
            (Rect::new(0, 0, 100, 40), 200, Rotation::R0, (0, 0)),
            (Rect::new(0, 0, 100, 40), 100, Rotation::R90, (51, 8)),
        ] {
            app.settings.card = Settings::for_pixels(120, 80).card;
            app.settings.dot = dot;
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
        select(&mut app, Setting::Strength);
        assert_eq!(app.layer, Layer::Original);
        for (focus, rebuilt) in [(false, true), (true, false)] {
            app.preview_focus = focus;
            assert_eq!(press(&mut app, KeyCode::Char(' ')), rebuilt);
            assert_eq!(app.layer, Layer::Dithered);
            assert!(!press(&mut app, KeyCode::Char(' ')));
            assert_eq!(app.layer, Layer::Original);
        }
        app.preview_focus = false;
        assert!(!press(&mut app, KeyCode::Right));
        assert_eq!(app.layer, Layer::Original);
        assert!(press(&mut app, KeyCode::Left));
        assert_eq!(app.layer, Layer::Dithered);
        assert!(!press(&mut app, KeyCode::Char(' ')));
        app.panel = ListPicker::new(0);
        assert!(press(&mut app, KeyCode::Left));
        assert_eq!(app.layer, Layer::Original);
        for code in [KeyCode::Char('r'), KeyCode::Char('d'), KeyCode::Char('D')] {
            assert!(press(&mut app, code));
            assert_eq!(app.layer, Layer::Original);
        }
        app.layer = Layer::Mass;
        select(&mut app, Setting::Strength);
        let strength = app.settings.strength;
        press(&mut app, KeyCode::Left);
        assert_ne!(app.settings.strength, strength);
        assert_eq!(app.layer, Layer::Mass);
        app.apply(Effect::Preview(app.settings.clone())).unwrap();
        assert_eq!(app.layer, Layer::Mass);
    }

    #[test]
    fn mass_toggle_builds_rgb_and_redraws_without_rebuilding_grid() {
        for focus in [false, true] {
            let mut app = App::builtin(RgbaImage::new(20, 10), None);
            app.preview_focus = focus;
            app.settings = Settings::for_pixels(8, 10);
            app.settings.rotation = Rotation::R90;
            let area = test_layout().preview;
            app.sync(area);
            let grid_version = app.grid_cache.grid_version();
            app.handle_key(
                KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
                &app.geometry(area),
            );
            assert_eq!(app.layer, Layer::Mass);
            assert!(app.sync(area));
            assert_eq!(app.grid_cache.grid_version(), grid_version);
            assert_eq!(app.grid_cache.dithered.version(), 0);
            let Some(Pixels::Rgb {
                image,
                version,
                layer: Layer::Mass,
            }) = app.displayed()
            else {
                panic!("expected mass RGB pixels");
            };
            assert_eq!(image.dimensions(), (8, 10));
            assert_eq!(version, app.grid_cache.mass_version());
            assert!(!app.sync(area));
            assert!(!press(&mut app, KeyCode::Char('a')));
            assert_eq!(app.layer, Layer::Original);
            press(&mut app, KeyCode::Char('a'));
            press(&mut app, KeyCode::Char(' '));
            assert_eq!(app.layer, Layer::Dithered);
            press(&mut app, KeyCode::Char('a'));
            assert_eq!(app.layer, Layer::Mass);
            press(&mut app, KeyCode::Char('a'));
            assert_eq!(app.layer, Layer::Dithered);
            press(&mut app, KeyCode::Char('a'));
            app.open_image(PathBuf::from("next.png"), RgbaImage::new(20, 10));
            assert_eq!(
                (app.layer, app.mass_return),
                (Layer::Original, Layer::Original)
            );
        }
    }

    #[test]
    fn image_grid_edits_use_the_loaded_source() {
        for (source, margin, expected_margin, expected_card, dot_margin, expected_dot) in [
            (None, 95, 100, crate::settings::CARDS[0], 400, 8),
            (Some((1, 1)), 95, 100, crate::settings::CARDS[0], 400, 8),
            (Some((16, 16)), 95, 100, crate::settings::CARDS[0], 400, 8),
            (
                Some((1024, 1024)),
                95,
                100,
                crate::settings::CARDS[0],
                400,
                9,
            ),
            (Some((1774, 1774)), 95, 95, BUSINESS_CARD, 0, 8),
        ] {
            let mut app = App::empty();
            if let Some((w, h)) = source {
                app.open_image("source.png".into(), RgbaImage::new(w, h));
            }
            app.settings.dot = 8;
            app.settings.margin = margin;
            select(&mut app, Setting::Margin);
            app.adjust(1);
            assert_eq!(app.settings.margin, expected_margin);

            app.settings.card = BUSINESS_CARD;
            app.settings.margin = 100;
            select(&mut app, Setting::Card);
            app.adjust(-1);
            assert_eq!(app.settings.card, expected_card);

            app.settings.card = crate::settings::CARDS[0];
            app.settings.dot = 9;
            app.settings.margin = dot_margin;
            select(&mut app, Setting::Dot);
            app.adjust(-1);
            assert_eq!(app.settings.dot, expected_dot);
            assert_eq!(app.settings.margin, dot_margin);
        }
    }

    #[test]
    fn mass_error_notice_follows_layer_and_minimap_visibility() {
        let mut app = App::builtin(RgbaImage::new(400, 400), None);
        let area = Rect::new(0, 0, 69, 36);
        app.mass.sigma = -1.0;
        app.layer = Layer::Mass;
        app.sync(area);
        let expected = format!("mass study: {}", app.grid_cache.mass_error().unwrap().0);
        assert_eq!(app.notice(), Some(expected.as_str()));
        assert!(!app.sync(area));
        assert_eq!(app.notice(), Some(expected.as_str()));
        app.set_notice("retained".into());
        assert_eq!(app.notice(), Some("retained"));
        app.notice = None;
        assert_eq!(app.notice(), Some(expected.as_str()));

        app.layer = Layer::Original;
        app.sync(area);
        assert!(app.notice().is_none());
        app.viewport.zoom = Zoom::In(2);
        app.sync(area);
        assert_eq!(app.notice(), Some(expected.as_str()));
        app.mass_minimap = false;
        app.sync(area);
        assert!(app.notice().is_none());
        app.mass_minimap = true;
        app.viewport.hide_minimap = true;
        app.sync(area);
        assert!(app.notice().is_none());

        app.layer = Layer::Mass;
        app.mass = MassOptions::default();
        app.sync(area);
        assert!(app.notice().is_none());
    }

    #[test]
    fn mass_minimap_builds_only_when_visible() {
        let mut app = App::builtin(RgbaImage::new(400, 400), None);
        let area = Rect::new(0, 0, 69, 36);
        assert!(app.mass_minimap);
        assert!(!app.sync(area));
        assert_eq!(app.grid_cache.mass_version(), 0);
        assert!(matches!(
            app.inset(),
            Some(Pixels::Rgb {
                layer: Layer::Original,
                ..
            })
        ));

        app.viewport.hide_minimap = true;
        app.viewport.zoom = Zoom::In(2);
        assert!(!app.sync(area));
        assert_eq!(app.grid_cache.mass_version(), 0);
        app.viewport.hide_minimap = false;
        assert!(app.sync(area));
        assert_eq!(app.grid_cache.mass_version(), 1);
        assert!(matches!(
            app.inset(),
            Some(Pixels::Rgb {
                layer: Layer::Mass,
                ..
            })
        ));
        assert!(matches!(
            app.displayed(),
            Some(Pixels::Rgb {
                layer: Layer::Original,
                ..
            })
        ));

        app.viewport.hide_minimap = true;
        assert!(!app.sync(area));
        assert_eq!(app.grid_cache.mass_version(), 1);
        // The cached mass inset is retained while the hidden minimap is not drawn.
        assert!(app.geometry(area).minimap.is_none());
        assert!(matches!(
            app.inset(),
            Some(Pixels::Rgb {
                layer: Layer::Mass,
                ..
            })
        ));
        app.mass_minimap = false;
        assert!(matches!(
            app.inset(),
            Some(Pixels::Rgb {
                layer: Layer::Original,
                ..
            })
        ));
        app.layer = Layer::Dithered;
        app.sync(area);
        assert!(matches!(app.inset(), Some(Pixels::Indexed { .. })));
        assert!(matches!(app.displayed(), Some(Pixels::Indexed { .. })));
    }

    #[test]
    fn mass_minimap_refreshes_after_card_grid_changes() {
        let mut app = App::builtin(RgbaImage::new(400, 400), None);
        let area = Rect::new(0, 0, 69, 36);
        app.viewport.zoom = Zoom::In(2);
        assert!(app.sync(area));
        app.settings = Settings::for_pixels(200, 200);
        assert!(app.sync(area));
        let Some(Pixels::Rgb {
            image,
            layer: Layer::Mass,
            ..
        }) = app.inset()
        else {
            panic!("expected mass inset");
        };
        assert_eq!(image.dimensions(), (200, 200));
        assert_eq!(image.dimensions(), app.grid_size());
        assert_eq!(app.grid_cache.mass_version(), 2);
    }

    #[test]
    fn mass_minimap_toggle_preserves_grid_and_visibility() {
        for focus in [false, true] {
            let mut app = App::builtin(RgbaImage::new(400, 400), None);
            let area = Rect::new(0, 0, 69, 36);
            app.preview_focus = focus;
            app.viewport.zoom = Zoom::In(2);
            assert!(app.sync(area));
            let before = versions(&app);
            for enabled in [false, true] {
                app.handle_key(
                    KeyEvent::new(KeyCode::Char('M'), KeyModifiers::NONE),
                    &app.geometry(area),
                );
                assert_eq!(app.mass_minimap, enabled);
                assert!(!app.sync(area));
                assert_eq!(versions(&app), before);
                assert_eq!(app.grid_cache.mass_version(), 1);
                assert!(!app.viewport.hide_minimap);
                assert_eq!(app.layer, Layer::Original);
            }
        }
    }

    #[test]
    fn image_template_apply_uses_the_loaded_source() {
        for (source, margin, expected_dot, expected_margin, modal_open, notice) in [
            (None, 100, 8, 100, false, None),
            (Some((16, 16)), 100, 8, 100, false, None),
            (
                Some((1774, 1774)),
                100,
                20,
                0,
                true,
                Some("grid exceeds four million dots"),
            ),
        ] {
            let mut app = App::empty();
            if let Some((w, h)) = source {
                app.open_image("source.png".into(), RgbaImage::new(w, h));
            }
            let settings = Settings {
                dot: 8,
                margin,
                ..Settings::default()
            };
            app.library
                .commit(Change::UpsertTemplate(Template::capture(
                    "Border".into(),
                    &settings,
                )))
                .unwrap();
            press(&mut app, KeyCode::Char('T'));
            press(&mut app, KeyCode::Enter);
            assert!(
                app.settings
                    == Settings {
                        dot: expected_dot,
                        margin: expected_margin,
                        ..Settings::default()
                    }
            );
            assert_eq!(app.modal.is_some(), modal_open);
            assert_eq!(app.notice(), notice);
        }
    }

    #[test]
    fn image_template_validation_preserves_large_margins() {
        for (source, expected_dot, expected_margin, modal_open, notice) in [
            ((1, 1), 8, 400, false, None),
            ((16, 16), 8, 400, false, None),
            (
                (1024, 1024),
                20,
                0,
                true,
                Some("grid exceeds four million dots"),
            ),
        ] {
            let mut app = App::builtin(RgbaImage::new(source.0, source.1), None);
            let template = Template::capture(
                "Border".into(),
                &Settings {
                    dot: 8,
                    margin: 400,
                    ..Settings::default()
                },
            );
            app.modal = Some(Modal::templates());
            app.apply_step(Step {
                next: Next::Close,
                effect: Some(Effect::Preview(template.apply(&app.settings).unwrap())),
            });
            assert!(
                app.settings
                    == Settings {
                        dot: expected_dot,
                        margin: expected_margin,
                        ..Settings::default()
                    }
            );
            assert_eq!(app.modal.is_some(), modal_open);
            assert_eq!(app.notice(), notice);
        }
    }

    #[test]
    fn oversized_loaded_image_reports_the_limit_until_corrected() {
        let mut app = App::empty();
        app.settings.dot = 8;
        app.settings.margin = 400;
        app.open_image("large.png".into(), RgbaImage::new(1024, 1024));
        assert!(app.grid_cache.has_source());
        assert_eq!(app.notice(), Some("grid exceeds four million dots"));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.notice(), Some("grid exceeds four million dots"));
        press(&mut app, KeyCode::Char('D'));
        assert!(app.grid_cache.placement(&app.settings).is_some());
        assert!(app.notice().is_none());
    }

    #[test]
    fn mass_tones_follow_palette_changes() {
        let mut app = App::builtin(RgbaImage::new(20, 10), None);
        assert_eq!(app.mass_tones, None);
        app.layer = Layer::Mass;
        let area = test_layout().preview;
        app.sync(area);
        let version = app.grid_cache.mass_version();
        app.settings.palette = vec![
            PaletteColor { rgb: [10, 20, 30] },
            PaletteColor {
                rgb: [210, 220, 230],
            },
        ];
        assert!(app.sync(area));
        assert_eq!(app.grid_cache.mass_version(), version + 1);
        assert_eq!(
            app.grid_cache.mass().unwrap().get_pixel(0, 0).0,
            [210, 220, 230, 255]
        );
    }

    #[test]
    fn original_preserves_rgb_and_alpha() {
        let source =
            RgbaImage::from_raw(8, 8, [255, 0, 0, 0, 25, 26, 128, 255].repeat(32)).unwrap();
        let expected = source.as_raw().clone();
        let mut app = App::builtin(source, None);
        app.settings = Settings::for_pixels(8, 8);
        app.refresh(false);
        let Some(Pixels::Rgb { image: pixels, .. }) = app.displayed() else {
            panic!("expected original RGB pixels");
        };
        assert_eq!(pixels.as_raw(), &expected);
        assert_eq!(app.grid_cache.grid.get().unwrap(), pixels);
    }

    #[test]
    fn navigation_stays_in_range_and_skips_redither() {
        let mut app = app_at(Setting::Card);
        assert!(!press(&mut app, KeyCode::Up));
        assert_eq!(app.panel.selected(Setting::ALL.len()), Some(0));
        for _ in 0..Setting::ALL.len() + 2 {
            assert!(!press(&mut app, KeyCode::Char('j')));
        }
        assert_eq!(
            Setting::ALL[app.panel.selected(Setting::ALL.len()).unwrap()],
            Setting::Sharpen
        );
        for code in [KeyCode::Char('x'), KeyCode::Enter, KeyCode::Esc] {
            assert!(!press(&mut app, code));
            assert_eq!(
                app.panel.selected(Setting::ALL.len()),
                Some(Setting::ALL.len() - 1)
            );
            assert!(!app.quit);
        }
    }

    #[test]
    fn algorithms_cycle_in_panel_order_and_back() {
        let mut app = app_at(Setting::Dither);
        for expected in Dither::ALL.into_iter().cycle().skip(1).take(9) {
            assert!(press(&mut app, KeyCode::Right));
            assert_eq!(app.settings.dither, expected);
        }
        assert!(press(&mut app, KeyCode::Left));
        assert_eq!(app.settings.dither, Dither::Noise);
    }

    #[test]
    fn bayer_ignores_diffusion_rows_and_retains_their_values() {
        let mut app = app_at(Setting::Strength);
        press(&mut app, KeyCode::Left);
        app.settings.serpentine = false;
        app.settings.dither = Dither::Bayer4;
        app.settings.seed = 42;
        app.sync(test_layout().preview);
        app.layer = Layer::Original;
        for setting in [Setting::Strength, Setting::Serpentine] {
            select(&mut app, setting);
            for key in [
                KeyCode::Left,
                KeyCode::Right,
                KeyCode::Enter,
                KeyCode::Char('d'),
            ] {
                let before = app.settings.clone();
                assert!(!press(&mut app, key));
                assert!(app.settings == before);
                assert_eq!(app.layer, Layer::Original);
            }
        }
        select(&mut app, Setting::Dither);
        app.layer = Layer::Dithered;
        assert!(press(&mut app, KeyCode::Char('d')));
        assert_eq!(app.settings.dither, Dither::Atkinson);
        assert_eq!(app.settings.strength, 0.9);
        assert!(!app.settings.serpentine);
        assert_eq!(app.settings.seed, 42);
    }

    #[test]
    fn seed_steps_and_enter_only_change_noise() {
        let mut app = app_at(Setting::Seed);
        let before = app.settings.clone();
        assert!(!press(&mut app, KeyCode::Enter));
        assert!(app.settings == before);
        assert_eq!(app.layer, Layer::Original);
        app.settings.dither = Dither::Noise;
        app.settings.seed = 0;
        assert!(!press(&mut app, KeyCode::Left));
        assert_eq!(app.settings.seed, 0);
        assert_eq!(app.layer, Layer::Original);
        assert!(press(&mut app, KeyCode::Right));
        assert_eq!(app.settings.seed, 1);
        let seed = app.settings.seed;
        app.layer = Layer::Original;
        assert!(press(&mut app, KeyCode::Enter));
        assert_ne!(app.settings.seed, seed);
        assert_eq!(app.layer, Layer::Dithered);
        assert!(press(&mut app, KeyCode::Char('d')));
        assert_eq!(app.settings.seed, 0);
    }

    #[test]
    fn numeric_settings_clamp_without_redither() {
        type Getter = fn(&Settings) -> f64;
        let cases: [(Setting, Getter); 8] = [
            (Setting::Dot, |p| f64::from(p.dot)),
            (Setting::Strength, |p| p.strength),
            (Setting::Brightness, |p| p.tone.brightness),
            (Setting::Contrast, |p| p.tone.contrast),
            (Setting::Gamma, |p| p.tone.gamma),
            (Setting::Black, |p| p.tone.black),
            (Setting::White, |p| p.tone.white),
            (Setting::Sharpen, |p| p.tone.sharpen),
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
        assert!(matches!(app.settings.mode, Mode::Color));
        let mut app = app_at(Setting::Serpentine);
        press(&mut app, KeyCode::Right);
        let serpentine = app.settings.serpentine;
        assert!(!serpentine);
        let mut app = app_at(Setting::Palette);
        // Wrapping back lands on the last built-in, which also switches to its Color mode.
        press(&mut app, KeyCode::Left);
        assert_eq!(app.settings.palette_name, "Pratt");
        assert_eq!(app.settings.palette.len(), 5);
        assert_eq!(app.settings.mode, Mode::Color);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.settings.palette_name, "Black/white");
        assert_eq!(app.settings.palette.len(), 2);
    }

    #[test]
    fn reset_restores_selected_or_all_defaults() {
        let mut app = app_at(Setting::Card);
        press(&mut app, KeyCode::Char('f'));
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('r'));
        select(&mut app, Setting::Mode);
        press(&mut app, KeyCode::Right);
        assert!(press(&mut app, KeyCode::Char('d')));
        assert!(matches!(app.settings.mode, Mode::Tone));
        assert!(app.settings.flip);
        assert_eq!(
            (app.settings.card, app.settings.rotation),
            (BUSINESS_CARD, Rotation::R90)
        );
        select(&mut app, Setting::Palette);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Char('D'));
        let p = &app.settings;
        assert_eq!(
            (p.card, p.rotation),
            (crate::settings::CARDS[0], Rotation::R0)
        );
        assert!(!p.flip);
        assert_eq!(p.palette_name, "Black/white");
        assert_eq!(p.palette.len(), 2);
        assert_eq!(app.grid_cache.dithered.get().unwrap().len(), 4 * 4);
    }

    #[test]
    fn reset_only_redithers_changed_values_and_preserves_pending_changes() {
        for (setting, dither) in Setting::ALL
            .into_iter()
            .map(|setting| (setting, Dither::Atkinson))
            .chain([(Setting::Seed, Dither::Noise)])
        {
            let mut app = App::builtin(RgbaImage::new(20, 10), None);
            select(&mut app, setting);
            assert!(!press(&mut app, KeyCode::Char('d')), "{setting:?}");
            assert!(!press(&mut app, KeyCode::Char('D')), "{setting:?}");
            app.settings.dither = dither;
            app.settings.seed = 0;
            app.sync(test_layout().preview);
            if !setting.is_active(&app.settings) {
                let before = app.settings.clone();
                app.layer = Layer::Original;
                for key in [
                    KeyCode::Left,
                    KeyCode::Right,
                    KeyCode::Enter,
                    KeyCode::Char('d'),
                ] {
                    assert!(!press(&mut app, key));
                    assert!(app.settings == before);
                    assert_eq!(app.layer, Layer::Original);
                }
                continue;
            }
            let change = if matches!(
                setting,
                Setting::Black
                    | Setting::Sharpen
                    | Setting::Seed
                    | Setting::Margin
                    | Setting::Bleed
            ) {
                KeyCode::Right
            } else {
                KeyCode::Left
            };
            let rebuilds = setting != Setting::Dot;
            assert_eq!(press(&mut app, change), rebuilds, "{setting:?}");
            assert_eq!(press(&mut app, KeyCode::Char('d')), rebuilds, "{setting:?}");
            assert!(!press(&mut app, KeyCode::Char('d')), "{setting:?}");
            assert_eq!(press(&mut app, change), rebuilds, "{setting:?}");
            assert_eq!(press(&mut app, KeyCode::Char('D')), rebuilds, "{setting:?}");
            assert!(!press(&mut app, KeyCode::Char('D')), "{setting:?}");
        }
        let mut app = app_at(Setting::Card);
        press(&mut app, KeyCode::Char('r'));
        assert!(!press(&mut app, KeyCode::Char('d')));
        assert!(press(&mut app, KeyCode::Char('D')));
        app.settings.tone.gamma = 2.0;
        app.layer = Layer::Dithered;
        app.sync(test_layout().preview);
        assert!(press(&mut app, KeyCode::Char('D')));
        assert_eq!(app.settings.tone.gamma, 1.0);
        app.handle_key(
            KeyEvent::new(KeyCode::Right, KeyModifiers::NONE),
            &app.geometry(test_layout().preview),
        );
        app.handle_key(
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            &app.geometry(test_layout().preview),
        );
        let version = versions(&app);
        app.refresh(false);
        assert_eq!(versions(&app), version);
    }

    #[test]
    fn quit_keys_and_ctrl_chords() {
        let mut app = app_at(Setting::Card);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Esc);
        assert!(!app.preview_focus);
        press(&mut app, KeyCode::Esc);
        assert!(!app.quit);
        press(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
        let mut app = app_at(Setting::Card);
        let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        app.handle_key(release, &app.geometry(test_layout().preview));
        assert!(!app.quit);
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let version = versions(&app);
        app.handle_key(ctrl('r'), &app.geometry(test_layout().preview));
        app.refresh(false);
        assert_eq!(versions(&app), version);
        app.handle_key(ctrl('c'), &app.geometry(test_layout().preview));
        assert!(app.quit);
    }

    #[test]
    fn flip_toggles_in_both_focuses_and_layers_and_rebuilds_once() {
        for focus in [false, true] {
            for layer in [Layer::Original, Layer::Dithered, Layer::Mass] {
                let mut app = App::builtin(RgbaImage::new(16, 12), None);
                app.settings = Settings::for_pixels(16, 12);
                app.preview_focus = focus;
                app.layer = layer;
                app.refresh(false);
                assert!(!app.settings.flip);
                for expected in [true, false] {
                    let before = versions(&app);
                    assert!(press(&mut app, KeyCode::Char('f')));
                    assert_eq!(app.settings.flip, expected);
                    assert_eq!(app.layer, layer);
                    let after = (before.0 + 1, before.1 + u64::from(layer == Layer::Dithered));
                    assert_eq!(versions(&app), after);
                    app.refresh(false);
                    assert_eq!(versions(&app), after);
                }
                press(&mut app, KeyCode::Char('o'));
                press(&mut app, KeyCode::Char('f'));
                assert!(!app.settings.flip);
            }
        }
    }

    #[test]
    fn rotation_keeps_the_actual_dither_grid() {
        let mut app = business_app(RgbaImage::new(256, 128), None);
        app.layer = Layer::Dithered;
        let size = app.grid_size();
        for (key, expected) in [
            ('r', Rotation::R90),
            ('r', Rotation::R180),
            ('R', Rotation::R90),
            ('R', Rotation::R0),
            ('R', Rotation::R270),
        ] {
            assert!(press(&mut app, KeyCode::Char(key)));
            assert_eq!(app.settings.rotation, expected);
            assert_eq!(app.grid_size(), size);
            assert_eq!(
                app.grid_cache.dithered().unwrap().len(),
                (size.0 * size.1) as usize
            );
        }
    }
    #[test]
    fn global_zoom_and_preview_pans_skip_redithering() {
        let mut app = App::builtin(RgbaImage::new(400, 400), None);
        app.settings = Settings::for_pixels(400, 400);
        app.refresh(false);
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
        assert_eq!(app.settings.card, BUSINESS_CARD);
    }

    #[test]
    fn directory_completion_opens_modal_and_ends_drag() {
        let mut app = App::drag_fixture();
        let layout = test_layout();
        for (kind, column) in [
            (MouseEventKind::Down(MouseButton::Left), 10),
            (MouseEventKind::Drag(MouseButton::Left), 9),
        ] {
            app.mouse_at(
                kind,
                column,
                layout.preview.y + 1,
                &app.geometry(layout.preview),
                layout.screen,
            );
        }
        assert!(app.dragging());
        assert!(app.modal.is_none());
        app.request_open("/images".into(), None);
        let outcome = IoOutcome::Opened(Ok(Opened::Dir(rthid_io::dir::Listing {
            dir: "/images".into(),
            entries: vec![],
        })));
        assert!(app.complete(done(&app, outcome)));
        assert!(matches!(app.modal, Some(Modal::OpenFile(_))));
        assert!(!app.dragging());
    }

    #[test]
    fn mouse_centers_pans_and_scrolls_without_focus_or_redithering() {
        let mut app = App::builtin(RgbaImage::new(120, 120), None);
        app.settings = Settings::for_pixels(120, 120);
        app.refresh(false);
        let area = Rect::new(2, 3, 48, 18);
        app.viewport.zoom = Zoom::In(1);
        let map = app.geometry(area).minimap.unwrap().map;
        let mouse = |app: &mut App, kind, column, row| {
            let version = versions(app);
            app.handle_mouse(
                mouse_event(kind, column, row),
                &app.geometry(area),
                test_layout().screen,
            );
            app.refresh(false);
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
        assert!(!app.dragging());
        mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 13, 12);
        assert!(app.dragging());
        assert_eq!((app.viewport.x, app.viewport.y), (69, 80));
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 13, 12);
        assert!(!app.dragging());
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
    fn minimap_drag_centers_after_leaving_map() {
        let mut app = App::drag_fixture();
        app.viewport.hide_minimap = false;
        let area = Rect::new(2, 3, 48, 18);
        let minimap = app.geometry(area).minimap.unwrap();
        let map = minimap.map;
        for outside in [
            (minimap.frame.x, map.y + 1),
            (minimap.frame.x - 1, map.y + 1),
            (0, 0),
        ] {
            let geometry = app.geometry(area);
            assert!(app.mouse_at(
                MouseEventKind::Down(MouseButton::Left),
                map.x + 4,
                map.y + 1,
                &geometry,
                test_layout().screen,
            ));
            let centered = (app.viewport.x, app.viewport.y);
            assert!(!app.mouse_at(
                MouseEventKind::Drag(MouseButton::Left),
                outside.0,
                outside.1,
                &app.geometry(area),
                test_layout().screen,
            ));
            assert_eq!((app.viewport.x, app.viewport.y), centered);
            let point = (map.right() - 1, map.bottom() - 1);
            let geometry = app.geometry(area);
            let mut expected = app.viewport;
            expected.center_on_minimap(point, &geometry);
            assert!(app.mouse_at(
                MouseEventKind::Drag(MouseButton::Left),
                point.0,
                point.1,
                &geometry,
                test_layout().screen,
            ));
            assert_eq!((app.viewport.x, app.viewport.y), (expected.x, expected.y));
            assert!(!app.dragging());
            assert!(!app.mouse_at(
                MouseEventKind::Up(MouseButton::Left),
                point.0,
                point.1,
                &app.geometry(area),
                test_layout().screen,
            ));
            assert!(app.last_drag.is_none());
        }
    }

    #[test]
    fn preview_drag_continues_over_minimap() {
        let mut app = App::drag_fixture();
        app.viewport.hide_minimap = false;
        app.viewport.x = 60;
        app.viewport.y = 60;
        let area = Rect::new(2, 3, 48, 18);
        let geometry = app.geometry(area);
        let map = geometry.minimap.unwrap().map;
        let start = (map.x - 2, map.y + 2);
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            start.0,
            start.1,
            &geometry,
            test_layout().screen,
        );
        for (point, expected) in [
            ((map.x + 2, map.y + 1), (56, 62)),
            ((map.x + 3, map.y + 2), (55, 60)),
        ] {
            assert!(app.mouse_at(
                MouseEventKind::Drag(MouseButton::Left),
                point.0,
                point.1,
                &app.geometry(area),
                test_layout().screen,
            ));
            assert_eq!((app.viewport.x, app.viewport.y), expected);
            let anchor = app.last_drag.as_ref().unwrap();
            assert_eq!(anchor.point, start);
            assert_eq!(anchor.origin, geometry.origin);
            assert!(app.dragging());
        }
    }

    #[test]
    fn minimap_press_updates_viewport_and_replaces_preview_drag() {
        let mut app = App::drag_fixture();
        app.viewport.hide_minimap = false;
        app.viewport.x = 60;
        app.viewport.y = 60;
        let area = Rect::new(2, 3, 48, 18);
        let geometry = app.geometry(area);
        let minimap = geometry.minimap.unwrap();
        let map = minimap.map;
        for (name, point, expected_return, expected_viewport, expected_last_drag) in [
            (
                "border",
                (minimap.frame.x, minimap.frame.y),
                false,
                geometry.origin,
                None,
            ),
            ("map", (map.x + 4, map.y + 1), true, (36, 27), Some(true)),
        ] {
            app.viewport.x = geometry.origin.0;
            app.viewport.y = geometry.origin.1;
            app.mouse_at(
                MouseEventKind::Down(MouseButton::Left),
                map.x - 2,
                map.y + 1,
                &geometry,
                test_layout().screen,
            );
            assert!(app.last_drag.is_some(), "{name}");
            assert_eq!(
                app.mouse_at(
                    MouseEventKind::Down(MouseButton::Left),
                    point.0,
                    point.1,
                    &geometry,
                    test_layout().screen,
                ),
                expected_return,
                "{name}",
            );
            assert_eq!(
                (app.viewport.x, app.viewport.y),
                expected_viewport,
                "{name}",
            );
            assert_eq!(
                app.last_drag.as_ref().map(|drag| drag.minimap),
                expected_last_drag,
                "{name}",
            );
            assert!(!app.dragging(), "{name}");
        }
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
                    mouse_event(kind, column, row),
                    &app.geometry(area),
                    test_layout().screen,
                );
            }
            assert_eq!((app.viewport.x, app.viewport.y), dragged, "{zoom:?}");
            for kind in [MouseEventKind::ScrollRight, MouseEventKind::ScrollDown] {
                app.handle_mouse(
                    mouse_event(kind, 7, 8),
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
        let mut app = business_app(RgbaImage::new(4, 2), Some(path.clone()));
        for code in [KeyCode::Char('r'), KeyCode::Char('q')] {
            app.handle_key(
                KeyEvent::new(code, KeyModifiers::NONE),
                &app.geometry(test_layout().preview),
            );
        }
        assert!(app.quit);
        assert_eq!(app.grid_cache.grid.version(), 1);
        jobs(&mut app);
        app.finish();
        let (output, image) = encoded_export(jobs(&mut app).pop().unwrap());
        assert_eq!(output, path);
        assert_eq!(image.dimensions(), (1000, 1800));
        assert!(jobs(&mut app).is_empty());
    }
    #[test]
    fn print_controls_rebuild_once_and_preserve_the_layer() {
        let mut app = business_app(RgbaImage::new(20, 10), None);
        select(&mut app, Setting::Dot);
        let before = versions(&app);
        assert!(press(&mut app, KeyCode::Right));
        assert_eq!(app.grid_size(), (238, 429));
        assert_eq!(versions(&app), (before.0 + 1, before.1));
        assert_eq!(app.layer, Layer::Original);
        assert!(!app.sync(test_layout().preview));
        app.layer = Layer::Dithered;
        app.refresh(false);
        for (setting, key) in [
            (Setting::Zoom, KeyCode::Right),
            (Setting::PanX, KeyCode::Right),
            (Setting::PanY, KeyCode::Left),
        ] {
            select(&mut app, setting);
            let before = versions(&app);
            assert!(press(&mut app, key));
            assert_eq!(versions(&app), (before.0 + 1, before.1 + 1));
            assert_eq!(app.layer, Layer::Dithered);
            assert!(!app.sync(test_layout().preview));
        }
        for setting in [Setting::Zoom, Setting::PanX, Setting::PanY] {
            app.settings.zoom = 400;
            app.settings.pan_x = 100;
            app.settings.pan_y = 100;
            app.refresh(false);
            select(&mut app, setting);
            let before = versions(&app);
            assert!(!press(&mut app, KeyCode::Right));
            assert_eq!(versions(&app), before);
        }
    }

    #[test]
    fn margin_consuming_card_builds_only_paper() {
        let mut app = App::builtin(RgbaImage::new(8, 8), None);
        app.settings = Settings {
            margin: 100,
            ..Settings::for_pixels(8, 8)
        };
        app.settings.tone.brightness = -0.5;
        app.layer = Layer::Dithered;
        app.refresh(false);
        assert_eq!(app.grid_size(), (8, 8));
        assert_eq!(app.grid_cache.dithered().unwrap(), &vec![1; 64]);
        assert!(
            app.grid_cache
                .grid()
                .unwrap()
                .pixels()
                .all(|p| p.0 == [255; 4])
        );
    }
}
