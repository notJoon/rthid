//! Interactive state and the key and mouse handling that changes it.

use crate::{
    brush::{Brush, BrushRow, Stroke},
    cells::{Glyphs, Pixels, View},
    document::{Document, History},
    export::{Export, MassExport},
    grid::GridCache,
    modal::{Ctx, Effect, Modal, Next, Step},
    session::{self, Session, ViewState},
    settings::{GROUP_LENS, Setting, Settings, Template},
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
use rthid_widgets::{Confirm, Outcome, Outline, Row};
use std::{
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, PartialEq)]
struct SavedState {
    revision: u64,
    settings: Settings,
    view: ViewState,
    selected: Option<usize>,
}

struct SessionSave {
    token: u64,
    epoch: u64,
    dir: PathBuf,
    state: SavedState,
    superseded: bool,
}

struct OpenRequest {
    state: SavedState,
    epoch: u64,
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

/// Meaning of a flat parameter panel item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PanelItem {
    /// An image setting in its existing order.
    Setting(Setting),
    /// An independent brush control.
    Brush(BrushRow),
    /// A document layer indexed from the bottom.
    Layer(usize),
}

#[derive(Clone, Copy)]
struct LayerDrag {
    index: usize,
    moved: bool,
    toggle: bool,
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
/// optional companion exports.
pub struct App {
    export: Option<Export>,
    mass_export: Option<MassExport>,
    source_path: Option<PathBuf>,
    /// Directory used when saving the current editing session.
    pub(crate) session: Option<PathBuf>,
    saved: Option<SavedState>,
    saved_token: u64,
    save_uncertain: bool,
    saving: Vec<SessionSave>,
    document_epoch: u64,
    notice: Option<Notice>,
    outbox: Vec<(Job, u64)>,
    open_request: Option<OpenRequest>,
    // Last value handed out as a dialog generation or job token.
    // Zero tags requests without a dialog.
    last_token: u64,
    /// Active dialog, which receives input before global bindings.
    pub modal: Option<Modal>,
    /// Whether the full key reference is capturing input.
    pub(crate) keys_open: bool,
    /// Retained row offset in the full key reference.
    pub(crate) keys_scroll: u16,
    /// Glyph set for sampled Dithered cells and the minimap.
    pub glyphs: Glyphs,
    /// Preview zoom, minimap and dot origin.
    pub viewport: Viewport,
    /// Whether keys go to the preview first (toggled by Tab, left with Esc).
    pub preview_focus: bool,
    last_drag: Option<Drag>,
    layer_drag: Option<LayerDrag>,
    stroke: Option<Stroke>,
    /// Current source pixel brush controls, independent of history.
    pub(crate) brush: Brush,
    /// Whether left preview drags paint exposure.
    pub(crate) brush_on: bool,
    /// Whether the last synced preview has a visible minimap.
    minimap_visible: bool,
    /// Image processing caches.
    pub grid_cache: GridCache,
    /// Source exposure layers and their undo history.
    pub edits: History,
    /// Named configuration lists and their pending saves.
    pub library: Library,
    /// Current image settings.
    pub settings: Settings,
    /// Parameter selection and retained scroll offset.
    pub panel: Outline,
    /// Selected view, initially Original.
    pub view: View,
    /// View that `a` returns to from the mass study.
    pub mass_return: View,
    /// Whether the minimap shows the mass study instead of the main view.
    pub mass_minimap: bool,
    /// Geometry controls for the mass study.
    pub mass: MassOptions,
    /// Fixed mass study tones, or None to follow the dither palette.
    pub mass_tones: Option<[PaletteColor; 4]>,
    /// Whether the event loop should exit.
    pub quit: bool,
}

/// Outline group containing the brush controls.
pub(crate) const BRUSH_GROUP: usize = GROUP_LENS.len();
/// Outline group containing document layers.
pub(crate) const LAYER_GROUP: usize = BRUSH_GROUP + 1;

impl App {
    /// Counts settings, brush controls and current layers for the outline.
    pub(crate) fn panel_lens(&self) -> [usize; 5] {
        [
            GROUP_LENS[0],
            GROUP_LENS[1],
            GROUP_LENS[2],
            BrushRow::ALL.len(),
            self.edits.current().layers().len(),
        ]
    }

    /// Maps an item from the current panel lens, with the top layer first.
    pub(crate) fn panel_row(&self, item: usize) -> PanelItem {
        debug_assert!(item < self.panel_lens().iter().sum());
        if let Some(&setting) = Setting::ALL.get(item) {
            PanelItem::Setting(setting)
        } else if let Some(&row) = BrushRow::ALL.get(item - Setting::ALL.len()) {
            PanelItem::Brush(row)
        } else {
            PanelItem::Layer(
                self.edits.current().layers().len()
                    - 1
                    - (item - Setting::ALL.len() - BrushRow::ALL.len()),
            )
        }
    }

    /// Returns a setting only when the cursor is on a settings row.
    pub(crate) fn selected_setting(&self) -> Option<Setting> {
        match self.panel.selected(&self.panel_lens()) {
            Some(Row::Item(i)) => match self.panel_row(i) {
                PanelItem::Setting(setting) => Some(setting),
                PanelItem::Brush(_) | PanelItem::Layer(_) => None,
            },
            _ => None,
        }
    }

    fn layer_row(&self, index: usize) -> Row {
        Row::Item(
            Setting::ALL.len() + BrushRow::ALL.len() + self.edits.current().layers().len()
                - 1
                - index,
        )
    }

    fn retain_panel(&mut self, row: Row) {
        let len = self.edits.current().layers().len();
        let row = match row {
            Row::Item(i) if i >= Setting::ALL.len() + BrushRow::ALL.len() + len => {
                if len == 0 {
                    Row::Header(LAYER_GROUP)
                } else {
                    Row::Item(Setting::ALL.len() + BrushRow::ALL.len() + len - 1)
                }
            }
            row => row,
        };
        self.panel.select(row, &self.panel_lens());
    }

    fn select_layer(&mut self, index: usize, toggle: bool) {
        let selected = if toggle && self.edits.current().selected() == Some(index) {
            None
        } else {
            Some(index)
        };
        self.edits.select(selected);
        self.retain_panel(self.layer_row(index));
    }

    fn toggle_layer(&mut self, index: usize) {
        let visible = !self.edits.current().layers()[index].visible;
        self.edits.edit(|doc| doc.set_visible(index, visible));
        self.retain_panel(self.layer_row(index));
    }

    fn handle_layer_key(&mut self, code: KeyCode) -> bool {
        if self.source_path().is_none() {
            return false;
        }
        let row = self
            .panel
            .selected(&self.panel_lens())
            .unwrap_or(Row::Header(LAYER_GROUP));
        match code {
            KeyCode::Char('n') => {
                self.edits.edit(|doc| {
                    doc.add_layer(doc.next_layer_name());
                });
                self.panel.invalidate();
                self.retain_panel(self.layer_row(self.edits.current().layers().len() - 1));
            }
            KeyCode::Char('u' | 'U') => {
                let undo = code == KeyCode::Char('u');
                let changed = if undo {
                    self.edits.undo()
                } else {
                    self.edits.redo()
                };
                // Undo and redo can reorder layers without changing the lens.
                self.panel.invalidate();
                if !changed {
                    self.set_notice(
                        if undo {
                            "nothing to undo"
                        } else {
                            "nothing to redo"
                        }
                        .into(),
                    );
                }
                self.retain_panel(row);
            }
            _ => {
                let Row::Item(item) = row else {
                    return false;
                };
                let PanelItem::Layer(index) = self.panel_row(item) else {
                    return false;
                };
                match code {
                    KeyCode::Enter => self.select_layer(index, true),
                    KeyCode::Char('v') => self.toggle_layer(index),
                    KeyCode::Char('x') => {
                        self.edits.edit(|doc| doc.remove_layer(index));
                        self.panel.invalidate();
                        self.retain_panel(row);
                    }
                    KeyCode::Char('[' | ']') => {
                        let to = if code == KeyCode::Char('[') {
                            index.saturating_sub(1)
                        } else {
                            (index + 1).min(self.edits.current().layers().len() - 1)
                        };
                        if to != index {
                            self.edits.edit(|doc| doc.move_layer(index, to));
                            self.panel.invalidate();
                            self.retain_panel(self.layer_row(to));
                        }
                    }
                    _ => return false,
                }
            }
        }
        true
    }

    pub(crate) fn mass_inset_shown(&self, geometry: &PreviewGeometry) -> bool {
        self.mass_minimap && self.mass_map_available(geometry)
    }

    pub(crate) fn mass_map_available(&self, geometry: &PreviewGeometry) -> bool {
        geometry.minimap.is_some() && self.view != View::Mass
    }

    /// Builds the source grid and queues requested companion exports.
    pub fn new(
        source: Option<(PathBuf, RgbaImage)>,
        preview_png: Option<PathBuf>,
        mass_svg: Option<PathBuf>,
        dpi: Option<u32>,
        loaded: Loaded,
    ) -> Self {
        let mut app = Self {
            export: preview_png.map(|path| Export::new(path, dpi)),
            mass_export: mass_svg.map(MassExport::new),
            source_path: None,
            session: None,
            saved: None,
            saved_token: 0,
            save_uncertain: false,
            saving: Vec::new(),
            document_epoch: 0,
            glyphs: Glyphs::default(),
            viewport: Viewport::default(),
            preview_focus: false,
            last_drag: None,
            layer_drag: None,
            stroke: None,
            brush: Brush::default(),
            brush_on: false,
            minimap_visible: false,
            grid_cache: GridCache::default(),
            edits: History::new(Document::new(0, 0)),
            library: Library::new(loaded),
            notice: None,
            outbox: Vec::new(),
            open_request: None,
            last_token: 0,
            modal: None,
            keys_open: false,
            keys_scroll: 0,
            settings: Settings::default(),
            panel: Outline::new(&[
                GROUP_LENS[0],
                GROUP_LENS[1],
                GROUP_LENS[2],
                BrushRow::ALL.len(),
                0,
            ]),
            view: View::Original,
            mass_return: View::Original,
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
        Self::new(None, None, None, None, Loaded::default())
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
        self.set_image(path, image);
        self.refresh(false);
        self.record_saved();
    }

    fn set_image(&mut self, path: PathBuf, image: RgbaImage) {
        let row = self
            .panel
            .selected(&self.panel_lens())
            .unwrap_or(Row::Header(LAYER_GROUP));
        self.layer_drag = None;
        self.stroke = None;
        self.document_epoch += 1;
        self.saved_token = 0;
        self.save_uncertain = false;
        self.source_path = Some(path);
        self.session = None;
        self.edits
            .reset(Document::new(image.width(), image.height()));
        self.retain_panel(row);
        self.grid_cache.set_source(image);
        self.viewport = Viewport::default();
        self.last_drag = None;
        self.settings.reset_composition();
        self.view = View::Original;
        self.mass_return = View::Original;
    }

    fn current_state(&self) -> SavedState {
        SavedState {
            revision: self.edits.revision(),
            settings: self.settings.clone(),
            view: ViewState::capture_unsaved(self),
            selected: self.edits.current().selected(),
        }
    }

    /// Records the current document as the initial saved state.
    pub(crate) fn record_saved(&mut self) {
        self.saved = Some(self.current_state());
    }

    /// Whether document, processing or preview state differs from the last saved snapshot.
    pub(crate) fn modified(&self) -> bool {
        self.source_path.is_some()
            && (self.save_uncertain
                || self
                    .saved
                    .as_ref()
                    .is_some_and(|saved| *saved != self.current_state()))
    }

    /// Replaces source, settings and layers with a validated session and fresh history.
    pub(crate) fn open_session(&mut self, dir: PathBuf, session: Session) {
        let row = self
            .panel
            .selected(&self.panel_lens())
            .unwrap_or(Row::Header(LAYER_GROUP));
        self.set_image(session.source_path, session.source);
        self.edits.reset(session.document);
        self.retain_panel(row);
        self.settings = session.settings;
        session.view.restore(self);
        self.session = Some(dir);
        self.refresh(false);
        self.record_saved();
    }

    fn confirm(&mut self, question: Confirm, effect: Effect) {
        let back = self.modal.take().map(Box::new);
        self.show_modal(Modal::Confirm {
            question,
            action: Some(Box::new(effect)),
            back,
        });
    }

    fn request_session_save(&mut self) {
        let Some(source) = self.source_path() else {
            return;
        };
        let dir = self
            .session
            .clone()
            .unwrap_or_else(|| session::default_path(source));
        let own_pending = self
            .saving
            .iter()
            .any(|save| save.dir == dir && save.epoch == self.document_epoch);
        if !own_pending && dir.exists() && !session::is_saveable(&dir) {
            self.set_notice(format!("{}: not a session directory", dir.display()));
        } else if !own_pending && self.session.is_none() && dir.exists() {
            self.confirm(
                Confirm::new(format!(
                    "Overwrite {}?",
                    dir.file_name().unwrap_or(dir.as_os_str()).to_string_lossy()
                ))
                .detail("The existing session will be replaced.")
                .yes_label("overwrite"),
                Effect::SaveSession(dir),
            );
        } else {
            self.save_session(dir);
        }
    }

    fn save_session(&mut self, dir: PathBuf) {
        let Some(source) = self.grid_cache.source.clone() else {
            return;
        };
        let token = self.next_token();
        // IO reports only the newest save per directory, so older entries never complete.
        let superseded = self.saving.iter().any(|save| save.dir == dir);
        self.saving.retain(|save| save.dir != dir);
        self.saving.push(SessionSave {
            token,
            epoch: self.document_epoch,
            dir: dir.clone(),
            state: self.current_state(),
            superseded,
        });
        self.set_notice(format!(
            "saving {}",
            dir.file_name().unwrap_or(dir.as_os_str()).to_string_lossy()
        ));
        let snapshot = Session {
            source_path: self.source_path.clone().unwrap(),
            source,
            document: self.edits.current().clone(),
            settings: self.settings.clone(),
            view: ViewState::capture(self),
        };
        self.outbox.push((
            Job::Save {
                dir,
                write: Box::new(move |dir| session::write(dir, snapshot)),
            },
            token,
        ));
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
                (self.view == View::Mass || (self.mass_minimap && self.minimap_visible))
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
        let mut edited_during_open = false;
        // Dialog that asked for the result. Listings always come from one.
        let mut requester = Some(done.tag);
        let opened = matches!(done.outcome, IoOutcome::Opened(_) | IoOutcome::Loaded(_));
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
            edited_during_open =
                request.epoch != self.document_epoch || request.state != self.current_state();
            requester = request.dialog;
            changed = true;
        }
        match done.outcome {
            IoOutcome::Saved(result) => {
                let Some(index) = self.saving.iter().position(|save| save.token == done.tag) else {
                    return false;
                };
                let save = self.saving.remove(index);
                match result {
                    Ok(dir)
                        if self.document_epoch == save.epoch && save.token > self.saved_token =>
                    {
                        self.saved_token = save.token;
                        self.save_uncertain = false;
                        self.session = Some(dir.clone());
                        self.saved = Some(save.state);
                        self.set_notice(format!(
                            "saved {}",
                            dir.file_name().unwrap_or(dir.as_os_str()).to_string_lossy()
                        ));
                    }
                    Ok(_) => return false,
                    Err(error) => {
                        if save.superseded && self.document_epoch == save.epoch {
                            self.save_uncertain = true;
                        }
                        self.set_notice(error.to_string());
                    }
                }
                changed = true;
            }
            outcome @ (IoOutcome::Loaded(Ok(_)) | IoOutcome::Opened(Ok(Opened::Image { .. }))) => {
                if requester.is_some() && requester == self.dialog() {
                    self.modal = None;
                }
                if edited_during_open {
                    self.confirm(
                        Confirm::new("Discard unsaved changes?")
                            .detail("Edits made while loading will be lost.")
                            .yes_label("discard changes"),
                        Effect::OpenLoaded(Box::new(outcome)),
                    );
                } else {
                    self.replace_open(outcome);
                }
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
                let path = result
                    .as_ref()
                    .map_or_else(|error| &error.path, |path| path);
                let mut export_error = None;
                let exports = self.export.as_mut().map(Export::state_mut);
                let mass = self.mass_export.as_mut().map(MassExport::state_mut);
                for export in exports.into_iter().chain(mass).filter(|e| e.path() == path) {
                    let clear_notice = result.is_ok()
                        && export.status().is_some_and(|status| {
                            self.notice.as_ref().is_some_and(|notice| {
                                matches!(notice.source, Source::Export) && notice.text == status
                            })
                        });
                    changed |= export.complete(result.as_ref().map(|_| ()));
                    if clear_notice {
                        self.notice = None;
                        changed = true;
                    }
                    export_error = export.status().map(ToOwned::to_owned);
                }
                if let Err(error) = result {
                    changed |= self.report_notice(Notice {
                        source: Source::Export,
                        text: export_error.unwrap_or_else(|| error.to_string()),
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
            IoOutcome::Opened(Err(error))
            | IoOutcome::Listed(Err(error))
            | IoOutcome::Loaded(Err(error)) => {
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
        self.keys_open = false;
        self.modal = Some(modal);
        self.last_drag = None;
    }

    /// Queues an image load and shows its file name while loading.
    pub fn request_open(&mut self, path: PathBuf, dir: Option<PathBuf>) {
        let resolved = Self::resolve_open(&path, dir.as_deref());
        if self.modified() && !(resolved.is_dir() && !session::is_session(&resolved)) {
            self.confirm(
                Confirm::new("Discard unsaved changes?")
                    .detail("Edits since the last Ctrl+S will be lost.")
                    .yes_label("discard changes"),
                Effect::OpenConfirmed(path, dir),
            );
        } else {
            self.start_open(path, dir);
        }
    }

    fn resolve_open(path: &Path, dir: Option<&Path>) -> PathBuf {
        let expanded = path.to_str().map_or_else(
            || path.to_path_buf(),
            |text| {
                rthid_io::path::expand_tilde(
                    text,
                    std::env::var_os("HOME").as_deref().map(Path::new),
                )
            },
        );
        if expanded.is_relative() {
            dir.map_or_else(|| expanded.clone(), |dir| dir.join(&expanded))
        } else {
            expanded
        }
    }

    fn replace_open(&mut self, outcome: IoOutcome) {
        match outcome {
            IoOutcome::Loaded(Ok(output)) => match output.downcast::<Session>() {
                Ok((path, session)) => self.open_session(path, session),
                Err(error) => {
                    self.set_notice(error.to_string());
                }
            },
            IoOutcome::Opened(Ok(Opened::Image { path, image })) => self.open_image(path, image),
            _ => unreachable!("only successful document loads can be accepted"),
        }
    }

    fn start_open(&mut self, path: PathBuf, dir: Option<PathBuf>) {
        let token = self.next_token();
        self.open_request = Some(OpenRequest {
            state: self.current_state(),
            epoch: self.document_epoch,
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
        let resolved = Self::resolve_open(&path, dir.as_deref());
        let resolved = session::containing(&resolved).unwrap_or(resolved);
        let protect: Vec<_> = self
            .export
            .as_ref()
            .map(|export| export.path().to_path_buf())
            .into_iter()
            .chain(
                self.mass_export
                    .as_ref()
                    .map(|export| export.path().to_path_buf()),
            )
            .collect();
        let job = if session::is_session(&resolved)
            || (resolved.is_dir() && resolved.extension().is_some_and(|ext| ext == "rthid"))
        {
            Job::load(resolved, move |dir| {
                let session = session::read(dir)?;
                session::check_exports(dir, &protect)?;
                for source in [&session.source_path, &dir.join("manifest.toml")] {
                    rthid_io::image::check_not_source(source, &protect)
                        .map_err(|error| std::io::Error::other(error.to_string()))?;
                }
                Ok(session)
            })
        } else {
            Job::Open { path, dir, protect }
        };
        self.outbox.push((job, token));
    }

    fn request_list(&mut self, path: PathBuf) {
        let tag = self.dialog().unwrap_or(0);
        self.outbox.push((Job::List(path), tag));
    }

    /// Inserts pasted text into the active field or requests a dropped image.
    pub fn handle_paste(&mut self, text: &str) {
        self.end_layer_drag();
        self.end_stroke();
        self.keys_open = false;
        if let Some(modal) = &mut self.modal {
            modal.paste(text);
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

    /// Rebuilds the grid and requested preview pixels, sends a new dither to the
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
        let need_dither = self.view == View::Dithered || self.has_export();
        let tones = self.mass_tones.unwrap_or_else(|| {
            mass::tones_from_palette(self.settings.palette.colors(), &self.mass)
        });
        let changed = self.grid_cache.refresh(
            &self.edits,
            &self.settings,
            need_dither,
            self.view == View::Mass || inset_mass || self.mass_export.is_some(),
            &self.mass,
            &tones,
        );
        let source = self.source_size();
        if self.grid_cache.placement(&self.settings).is_some()
            && let Some(export) = &mut self.export
        {
            let (w, h) = self.grid_cache.grid().expect("processed grid").dimensions();
            if let Some(job) = export.sync(
                self.grid_cache.dithered().expect("dithered grid"),
                w,
                h,
                &self.settings,
                source,
                changed,
            ) {
                self.outbox.push((job, 0));
            }
        }
        if let Some(export) = &mut self.mass_export {
            let version = self.grid_cache.mass_version();
            if let (Some(preview), Some(study)) =
                (self.grid_cache.grid(), self.grid_cache.mass_study())
            {
                if let Some(job) = export.sync(
                    version,
                    preview,
                    study,
                    tones.map(|tone| tone.rgb),
                    &self.mass,
                ) {
                    self.outbox.push((job, 0));
                }
            } else if let Some((_, error)) = self.grid_cache.mass_error() {
                export.fail(version, error);
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

    /// Latest companion export error for the status line, with PNG first.
    pub fn export_status(&self) -> Option<&str> {
        self.export
            .as_ref()
            .and_then(Export::status)
            .or_else(|| self.mass_export.as_ref().and_then(MassExport::status))
    }

    /// Selected preview pixels after sync, or None without a source.
    ///
    /// Panics if the selected processed view was never built.
    pub fn displayed(&self) -> Option<Pixels<'_>> {
        let grid = self.grid_cache.grid()?;
        Some(match self.view {
            View::Original => Pixels::Rgb {
                image: grid,
                version: self.grid_cache.grid_version(),
                view: View::Original,
            },
            View::Dithered => Pixels::Indexed {
                indices: self.grid_cache.dithered().expect("dithered grid"),
                palette: self.settings.palette.colors(),
            },
            View::Mass => Pixels::Rgb {
                image: self.grid_cache.mass().expect("mass grid"),
                version: self.grid_cache.mass_version(),
                view: View::Mass,
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
                view: View::Mass,
            });
        }
        self.displayed()
    }

    /// Queues the final export including changes received with quit.
    pub fn finish(&mut self) {
        self.refresh(false);
    }

    /// Returns the last companion export result, with PNG first.
    pub fn export_result(&self) -> std::io::Result<()> {
        self.export.as_ref().map_or(Ok(()), Export::result)?;
        self.mass_export.as_ref().map_or(Ok(()), MassExport::result)
    }

    /// Whether a left mouse drag has panned the preview.
    pub fn dragging(&self) -> bool {
        self.last_drag.is_some_and(|drag| drag.moved)
    }

    /// Ends a layer reorder when an unrelated terminal event arrives.
    ///
    /// Returns whether the drag moved a layer, which needs a redraw before the
    /// panel accepts clicks again.
    pub(crate) fn end_layer_drag(&mut self) -> bool {
        if !self.layer_drag.take().is_some_and(|drag| drag.moved) {
            return false;
        }
        self.edits.discard_if_unchanged();
        // Drag hits follow row positions, so only later clicks see a stale layout.
        self.panel.invalidate();
        true
    }

    /// Ends the current exposure stroke before unrelated input changes its context.
    pub(crate) fn end_stroke(&mut self) {
        if let Some(stroke) = self.stroke.take() {
            if stroke.created_layer && self.edits.current().layers()[0].exposure.is_empty() {
                self.edits.amend(|doc| doc.remove_layer(0));
            }
            self.edits.discard_if_unchanged();
        }
    }

    fn source_point(
        &self,
        geometry: &PreviewGeometry,
        column: u16,
        row: u16,
    ) -> Option<(f64, f64)> {
        if geometry
            .minimap
            .is_some_and(|map| map.frame.contains((column, row).into()))
        {
            return None;
        }
        self.grid_cache
            .placement(&self.settings)?
            .source_point(geometry.dot_at(column, row)?, self.source_size())
    }

    fn start_stroke(&mut self, point: (f64, f64)) -> bool {
        let doc = self.edits.current();
        if doc.target().is_some_and(|i| !doc.layers()[i].visible) {
            self.set_notice("target layer is hidden".into());
            return true;
        }
        self.notice = None;
        let brush = self.brush;
        let created_layer = doc.layers().is_empty();
        self.edits.edit(|doc| {
            if doc.layers().is_empty() {
                doc.add_layer(doc.next_layer_name());
            }
            brush.dab(
                &mut doc.target_mut().expect("stroke target").exposure,
                point,
            );
        });
        let mut stroke = Stroke::new(point, brush);
        stroke.created_layer = created_layer;
        self.stroke = Some(stroke);
        self.last_drag = None;
        true
    }

    /// Routes mouse input and reports whether the display needs redrawing.
    pub fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        geometry: &PreviewGeometry,
        screen: Rect,
    ) -> bool {
        let mut ended = false;
        if mouse.kind != MouseEventKind::Drag(MouseButton::Left) {
            if mouse.kind == MouseEventKind::Up(MouseButton::Left)
                && let Some(drag) = self.layer_drag
                && !drag.moved
                && drag.toggle
            {
                self.select_layer(drag.index, true);
                ended = true;
            }
            self.end_stroke();
            let moved = self.end_layer_drag();
            if moved && mouse.kind != MouseEventKind::Up(MouseButton::Left) {
                // A lost release: handle this event too, and redraw for the reorder.
                self.handle_mouse(mouse, geometry, screen);
                return true;
            }
            ended |= moved;
        }
        if ended {
            return true;
        }
        let active_drag = self.layer_drag.take();
        let was_dragging = self.dragging();
        if self.keys_open {
            self.last_drag = None;
            return was_dragging;
        }
        if let Some(mut modal) = self.modal.take() {
            self.last_drag = None;
            let ctx = self.modal_ctx();
            let before = modal.selection(&ctx);
            let step = modal.handle_mouse(mouse, ctx);
            let changed = before != modal.selection(&self.modal_ctx())
                || step.effect.is_some()
                || !matches!(step.next, Next::Stay);
            self.modal = Some(modal);
            self.apply_step(step);
            return was_dragging || changed;
        }
        if let Some(mut stroke) = self.stroke.take() {
            let point = self.source_point(geometry, mouse.column, mouse.row);
            let brush = self.brush;
            let mut centers = Vec::new();
            stroke.advance(point, brush, |point| centers.push(point));
            let painted = !centers.is_empty();
            if painted {
                self.edits.amend(|doc| {
                    let map = &mut doc.target_mut().expect("stroke target").exposure;
                    for point in centers {
                        brush.dab(map, point);
                    }
                });
            }
            self.stroke = Some(stroke);
            return painted;
        }
        let panel = crate::ui::layout(screen).panel;
        let lens = self.panel_lens();
        let hit = self.panel.row_at((mouse.column, mouse.row).into(), &lens);
        if let Some(mut drag) = active_drag {
            if let Some(Row::Item(item)) = hit
                && let PanelItem::Layer(to) = self.panel_row(item)
                && to != drag.index
            {
                if drag.moved {
                    self.edits.amend(|doc| doc.move_layer(drag.index, to));
                } else {
                    self.edits.edit(|doc| doc.move_layer(drag.index, to));
                }
                drag.index = to;
                drag.moved = true;
                self.retain_panel(self.layer_row(to));
                self.layer_drag = Some(drag);
                return true;
            }
            self.layer_drag = Some(drag);
            return false;
        }
        if panel.contains((mouse.column, mouse.row).into())
            && !geometry.area.contains((mouse.column, mouse.row).into())
            && !matches!(mouse.kind, MouseEventKind::Drag(_))
        {
            self.last_drag = None;
            let before = self.panel.selected(&lens);
            let outcome = self.panel.handle_mouse(mouse, &lens);
            let clicked = outcome != Outcome::Ignored
                && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left));
            if clicked {
                self.preview_focus = false;
                if self.source_path().is_some()
                    && let Some(Row::Item(item)) = hit
                    && let PanelItem::Layer(index) = self.panel_row(item)
                {
                    if mouse.column == panel.x + 3 {
                        self.toggle_layer(index);
                    } else {
                        if before != hit {
                            self.select_layer(index, false);
                        }
                        self.layer_drag = Some(LayerDrag {
                            index,
                            moved: false,
                            toggle: before == hit,
                        });
                    }
                }
            }
            return was_dragging || outcome != Outcome::Ignored;
        }
        if self.source_path().is_none() {
            self.last_drag = None;
            return was_dragging;
        }
        if self.brush_on {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left)
                && let Some(point) = self.source_point(geometry, mouse.column, mouse.row)
            {
                return self.start_stroke(point);
            }
            let minimap = geometry
                .minimap
                .is_some_and(|map| map.frame.contains((mouse.column, mouse.row).into()));
            if matches!(
                mouse.kind,
                MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
            ) && !minimap
                && !self.last_drag.is_some_and(|drag| drag.minimap)
            {
                self.last_drag = None;
                return false;
            }
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
            view: self.view,
        }
    }

    fn apply_step(&mut self, mut step: Step) {
        if matches!(self.modal, Some(Modal::Confirm { .. })) && step.effect.is_some() {
            match std::mem::replace(&mut step.next, Next::Stay) {
                Next::Stay => {}
                Next::Close => self.modal = None,
                Next::Replace(modal) => self.show_modal(modal),
            }
        }
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
        self.end_layer_drag();
        self.end_stroke();
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        self.notice = None;
        if self.keys_open {
            let max = crate::ui::key_help_scroll_max(geometry.area.height);
            self.keys_scroll = self.keys_scroll.min(max);
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                match key.code {
                    KeyCode::Esc | KeyCode::Char('?') => self.keys_open = false,
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.keys_scroll = self.keys_scroll.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.keys_scroll = self.keys_scroll.saturating_add(1).min(max);
                    }
                    _ => {}
                }
            }
            return;
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('s') {
            if self.modal.is_none() {
                self.request_session_save();
            }
            return;
        }
        if (key.modifiers == KeyModifiers::CONTROL && matches!(key.code, KeyCode::Char('z' | 'y')))
            || (key.modifiers == (KeyModifiers::CONTROL | KeyModifiers::SHIFT)
                && matches!(key.code, KeyCode::Char('z' | 'Z')))
        {
            if self.modal.is_none() {
                let undo =
                    key.code == KeyCode::Char('z') && !key.modifiers.contains(KeyModifiers::SHIFT);
                self.handle_layer_key(KeyCode::Char(if undo { 'u' } else { 'U' }));
            }
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
        if key.code == KeyCode::Char('?') {
            self.keys_open = true;
            self.keys_scroll = 0;
            self.last_drag = None;
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
        if key.code == KeyCode::Char('b') {
            if self.source_path().is_some() {
                self.brush_on = !self.brush_on;
                self.last_drag = None;
            }
            return;
        }
        if key.code == KeyCode::Char('B') {
            if self.source_path().is_some() {
                BrushRow::Mode.adjust(&mut self.brush, 1);
            }
            return;
        }
        if self.handle_layer_key(key.code) {
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
                    self.view = if self.view == View::Dithered {
                        View::Original
                    } else {
                        View::Dithered
                    };
                }
                PreviewKey::ToggleMass => {
                    if self.view == View::Mass {
                        self.view = self.mass_return;
                    } else {
                        self.mass_return = self.view;
                        self.view = View::Mass;
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
        if self.preview_focus
            && key.code == KeyCode::Enter
            && matches!(
                self.panel.selected(&self.panel_lens()),
                Some(Row::Header(_))
            )
        {
            return;
        }
        match self.handle_panel_key(key) {
            Outcome::Ignored => {}
            Outcome::Submit(i) => {
                let PanelItem::Setting(setting) = self.panel_row(i) else {
                    return;
                };
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
        if let Some(Row::Item(i)) = self.panel.selected(&self.panel_lens())
            && let PanelItem::Brush(row) = self.panel_row(i)
        {
            let direction = match key.code {
                KeyCode::Char('d') => Some(0),
                KeyCode::Left | KeyCode::Char('h') => Some(-1),
                KeyCode::Right | KeyCode::Char('l') => Some(1),
                _ => None,
            };
            if let Some(direction) = direction {
                row.adjust(&mut self.brush, direction);
                return;
            }
        }
        match key.code {
            KeyCode::Char('q') => {
                if self.modified() {
                    self.confirm(
                        Confirm::new("Discard unsaved changes?")
                            .detail("Edits since the last Ctrl+S will be lost.")
                            .yes_label("discard changes"),
                        Effect::Quit,
                    );
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('f') => self.settings.flip = !self.settings.flip,
            KeyCode::Char('r') => self.settings.rotation = self.settings.rotation.cw(),
            KeyCode::Char('R') => self.settings.rotation = self.settings.rotation.ccw(),
            KeyCode::Char('d') => {
                let Some(setting) = self.selected_setting() else {
                    return;
                };
                let next = self.settings.with_default(setting, &self.library.palettes);
                let _ = self.set_settings(next, self.view);
            }
            KeyCode::Left | KeyCode::Char('h') => self.adjust(-1),
            KeyCode::Right | KeyCode::Char('l') => self.adjust(1),
            _ => {}
        }
    }

    fn handle_panel_key(&mut self, key: KeyEvent) -> Outcome<usize> {
        let lens = self.panel_lens();
        if !matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::Char('k' | 'j')
        ) {
            return self.panel.handle_key(key, &lens);
        }
        // Inactive rows are skipped here, not in Outline. Move this into the widget if another list needs disabled rows.
        let start = self.panel.selected(&lens);
        loop {
            let before = self.panel.selected(&lens);
            let outcome = self.panel.handle_key(key, &lens);
            let after = self.panel.selected(&lens);
            if after == before {
                if let Some(row) = start {
                    self.panel.select(row, &lens);
                }
                return outcome;
            }
            if !matches!(after, Some(Row::Item(i)) if matches!(self.panel_row(i), PanelItem::Setting(setting) if !setting.is_active(&self.settings)))
            {
                return outcome;
            }
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
            Effect::Quit => self.quit = true,
            Effect::OpenConfirmed(path, dir) => self.start_open(path, dir),
            Effect::OpenLoaded(outcome) => self.replace_open(*outcome),
            Effect::SaveSession(dir) => self.save_session(dir),
            Effect::Open(path, dir) => self.request_open(path, Some(dir)),
            Effect::ListDir(path) => self.request_list(path),
            Effect::Notice(message) => {
                self.set_notice(message);
            }
            Effect::Preview(settings) => {
                self.set_settings(settings, self.edit_view())?;
            }
            Effect::Restore { settings, view } => {
                self.set_settings(settings, view)?;
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
        let Some(setting) = self.selected_setting() else {
            return;
        };
        let next = self
            .settings
            .adjusted(setting, direction, &self.library.palettes);
        self.apply_setting(setting, next);
    }

    fn set_settings(&mut self, settings: Settings, view: View) -> Result<(), String> {
        settings.dimensions(self.source_size())?;
        self.settings = settings;
        self.view = view;
        Ok(())
    }

    fn edit_view(&self) -> View {
        if self.view == View::Mass {
            View::Mass
        } else {
            View::Dithered
        }
    }

    // Enter and arrow keys share the preview rule and preserve it for rejected edits.
    // The mass study stays selected because palette edits recolor its tones.
    fn apply_setting(&mut self, setting: Setting, next: Settings) {
        let view = if next != self.settings && setting.shows_dithered() {
            self.edit_view()
        } else {
            self.view
        };
        let _ = self.set_settings(next, view);
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
        modal::PATH_MAX_CHARS,
        settings::{BUSINESS_CARD, Dither, Rotation, Templates},
        ui::Layout,
    };
    use ratatui::layout::Rect;
    use rthid_dither::dither::Mode;

    fn business_app(source: RgbaImage, preview_png: Option<PathBuf>) -> App {
        let mut app = App::new(None, preview_png, None, None, Loaded::default());
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

    fn ctrl(app: &mut App, c: char) {
        app.handle_key(
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
            &app.geometry(test_layout().preview),
        );
    }

    fn run_session_job(app: &mut App) {
        let jobs = app.take_jobs();
        assert_eq!(jobs.len(), 1);
        let (job, tag) = jobs.into_iter().next().unwrap();
        let outcome = match job {
            Job::Save { dir, write } => {
                let result =
                    write(&dir)
                        .map(|()| dir.clone())
                        .map_err(|error| rthid_io::io::IoError {
                            op: rthid_io::io::Op::Save,
                            path: dir,
                            cause: rthid_io::io::Cause::Io(error),
                        });
                IoOutcome::Saved(result)
            }
            Job::Load { path, read } => {
                let value = read(&path).unwrap();
                IoOutcome::Loaded(Ok(rthid_io::io::Output { path, value }))
            }
            _ => panic!("expected session job"),
        };
        app.complete(Done::new(tag, std::time::Duration::ZERO, outcome));
    }

    fn snapshot(app: &App) -> Session {
        Session {
            source: app.grid_cache.source.clone().unwrap(),
            source_path: app.source_path.clone().unwrap(),
            document: app.edits.current().clone(),
            settings: app.settings.clone(),
            view: ViewState::capture(app),
        }
    }

    fn session_completion(tag: u64, dir: &Path, succeeds: bool) -> Done {
        let result = if succeeds {
            Ok(dir.to_path_buf())
        } else {
            Err(rthid_io::io::IoError {
                op: rthid_io::io::Op::Save,
                path: dir.to_path_buf(),
                cause: rthid_io::io::Cause::Io(std::io::Error::other("injected save failure")),
            })
        };
        Done::new(tag, std::time::Duration::ZERO, IoOutcome::Saved(result))
    }

    // IO reports only the newest save per directory, so the first completion
    // is ignored even when it arrives.
    #[test]
    fn overlapping_session_saves_keep_the_newest_successful_baseline() {
        for second_succeeds in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let dir = temp.path().join("photo.rthid");
            let mut app = App::builtin(RgbaImage::new(2, 3), None);
            app.open_image(temp.path().join("photo.png"), RgbaImage::new(2, 3));
            app.settings.flip = true;
            ctrl(&mut app, 's');
            let first = app.take_jobs().pop().unwrap().1;
            app.settings.flip = false;
            ctrl(&mut app, 's');
            assert!(app.modal.is_none());
            assert_eq!(app.saving.len(), 1);
            let second = app.take_jobs().pop().unwrap().1;
            assert!(!app.complete(session_completion(first, &dir, true)));
            app.complete(session_completion(second, &dir, second_succeeds));
            assert_eq!(app.session.as_ref(), second_succeeds.then_some(&dir));
            assert_eq!(app.modified(), !second_succeeds);
            if !second_succeeds {
                assert!(app.notice().unwrap().contains("injected save failure"));
            }
        }
    }

    #[test]
    fn superseded_save_failure_stays_modified_until_success() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("photo.rthid");
        let mut app = App::builtin(RgbaImage::new(2, 3), None);
        app.open_image(temp.path().join("photo.png"), RgbaImage::new(2, 3));
        app.save_session(dir.clone());
        let first = app.take_jobs().pop().unwrap().1;
        app.complete(session_completion(first, &dir, false));
        assert!(!app.modified());
        app.settings.flip = true;
        app.save_session(dir.clone());
        run_session_job(&mut app);
        app.settings.flip = false;
        app.save_session(dir.clone());
        run_session_job(&mut app);
        assert!(!app.modified());
        app.settings.flip = true;
        app.save_session(dir.clone());
        run_session_job_without_completion(&mut app);
        app.settings.flip = false;
        app.save_session(dir.clone());
        let latest = app.take_jobs().pop().unwrap().1;
        assert_eq!(app.saving.len(), 1);
        app.complete(session_completion(latest, &dir, false));
        assert!(app.modified());
        press(&mut app, KeyCode::Char('q'));
        assert!(matches!(app.modal, Some(Modal::Confirm { .. })));
        press(&mut app, KeyCode::Char('n'));
        app.request_open("next.png".into(), None);
        assert!(matches!(app.modal, Some(Modal::Confirm { .. })));
        press(&mut app, KeyCode::Char('n'));
        app.save_session(dir.clone());
        run_session_job(&mut app);
        assert!(!app.modified());
    }

    fn run_session_job_without_completion(app: &mut App) {
        let (job, _) = app.take_jobs().pop().unwrap();
        let Job::Save { dir, write } = job else {
            panic!("expected save")
        };
        write(&dir).unwrap();
    }

    #[test]
    fn edits_during_load_require_confirmation_for_sessions_and_images() {
        for session_load in [false, true] {
            for accept in [false, true] {
                let mut app = App::builtin(RgbaImage::new(2, 3), None);
                let loaded = snapshot(&app);
                app.request_open("next.png".into(), None);
                let tag = app.open_request.as_ref().unwrap().token;
                app.edits.edit(|doc| {
                    doc.add_layer("Keep".into());
                });
                let outcome = if session_load {
                    IoOutcome::Loaded(Ok(rthid_io::io::Output {
                        path: "next.rthid".into(),
                        value: Box::new(loaded),
                    }))
                } else {
                    IoOutcome::Opened(Ok(Opened::Image {
                        path: "next.png".into(),
                        image: RgbaImage::new(2, 3),
                    }))
                };
                app.complete(Done::new(tag, std::time::Duration::ZERO, outcome));
                assert!(matches!(app.modal, Some(Modal::Confirm { .. })));
                assert_eq!(app.edits.current().layers().len(), 1);
                press(&mut app, KeyCode::Char(if accept { 'y' } else { 'n' }));
                assert_eq!(app.edits.current().layers().len(), usize::from(!accept));
                assert!(app.open_request.is_none());
            }
        }
    }

    #[test]
    fn session_load_rejects_exports_anywhere_inside_the_directory() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("photo.rthid");
        let app = App::builtin(RgbaImage::new(2, 3), None);
        session::write(&dir, snapshot(&app)).unwrap();
        for name in [
            "source-1.png",
            "exposure-1-0.png",
            "new.png",
            "nested/new.png",
        ] {
            let destination = dir.join(name);
            assert!(session::check_exports(&dir, &[&destination]).is_err());
            let mut app = App::builtin(RgbaImage::new(2, 3), Some(destination));
            app.take_jobs();
            app.request_open(dir.clone(), None);
            let (job, _) = app.take_jobs().pop().unwrap();
            let Job::Load { path, read } = job else {
                panic!("expected load")
            };
            assert!(read(&path).is_err());
        }
        assert!(session::check_exports(&dir, &[temp.path().join("preview.png")]).is_ok());
        #[cfg(unix)]
        {
            let alias = temp.path().join("alias");
            std::os::unix::fs::symlink(&dir, &alias).unwrap();
            assert!(session::check_exports(&dir, &[alias.join("new.png")]).is_err());
        }
    }

    #[test]
    fn late_save_failure_is_reported_for_a_different_document() {
        let mut app = App::builtin(RgbaImage::new(2, 3), None);
        ctrl(&mut app, 's');
        let tag = app.take_jobs().pop().unwrap().1;
        app.open_image("other.png".into(), RgbaImage::new(1, 1));
        assert!(app.complete(session_completion(tag, Path::new("photo.rthid"), false)));
        assert!(app.notice().unwrap().contains("injected save failure"));
        assert!(app.session.is_none());
        assert!(!app.modified());
    }

    #[test]
    fn confirmed_open_failure_restores_the_dialog() {
        let mut app = App::builtin(RgbaImage::new(2, 3), None);
        app.settings.flip = true;
        app.open_dialog(".".into());
        let dialog = app.dialog();
        app.request_open("missing.png".into(), Some(".".into()));
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.dialog(), dialog);
        assert!(matches!(app.modal, Some(Modal::OpenFile(_))));
        app.take_jobs();
        app.complete(done(
            &app,
            IoOutcome::Opened(Err(failure(rthid_io::io::Op::Decode))),
        ));
        assert!(matches!(app.modal, Some(Modal::OpenFile(_))));
        assert!(app.notice().is_some());
        app.request_open("photo.png".into(), Some(".".into()));
        press(&mut app, KeyCode::Char('y'));
        app.take_jobs();
        app.complete(done(&app, opened_photo()));
        assert!(app.modal.is_none());
    }

    #[test]
    fn session_source_paths_route_to_sessions_and_protect_the_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().canonicalize().unwrap().join("photo.rthid");
        let mut app = App::builtin(RgbaImage::new(2, 3), None);
        app.edits.edit(|doc| {
            doc.add_layer("Stored".into());
        });
        session::write(&dir, snapshot(&app)).unwrap();
        let source = session::read(&dir).unwrap().source_path;
        app.open_image("other.png".into(), RgbaImage::new(1, 1));
        app.request_open(source.clone(), None);
        run_session_job(&mut app);
        assert_eq!(app.session.as_ref(), Some(&dir));
        assert_eq!(app.edits.current().layers()[0].name, "Stored");
        app.open_dialog(dir.clone());
        app.request_open(
            PathBuf::from(source.file_name().unwrap()),
            Some(dir.clone()),
        );
        run_session_job(&mut app);
        assert_eq!(app.session.as_ref(), Some(&dir));
        let before = std::fs::read(dir.join("manifest.toml")).unwrap();
        let mut protected = App::builtin(RgbaImage::new(1, 1), Some(dir.join("manifest.toml")));
        protected.take_jobs();
        protected.request_open(dir.clone(), None);
        let (job, tag) = protected.take_jobs().pop().unwrap();
        let Job::Load { path, read } = job else {
            panic!("expected session load")
        };
        let error = read(&path).err().unwrap();
        protected.complete(Done::new(
            tag,
            std::time::Duration::ZERO,
            IoOutcome::Loaded(Err(rthid_io::io::IoError {
                op: rthid_io::io::Op::Decode,
                path,
                cause: rthid_io::io::Cause::Io(error),
            })),
        ));
        assert!(protected.session.is_none());
        assert!(protected.notice().unwrap().contains("manifest.toml"));
        assert_eq!(std::fs::read(dir.join("manifest.toml")).unwrap(), before);
    }

    #[test]
    fn session_open_refreshes_once_with_restored_settings() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("photo.rthid");
        let mut stored = App::builtin(RgbaImage::new(2, 3), None);
        stored.settings.flip = true;
        stored.settings.rotation = Rotation::R90;
        session::write(&dir, snapshot(&stored)).unwrap();
        let mut app = App::builtin(RgbaImage::new(1, 1), Some(temp.path().join("preview.png")));
        app.take_jobs();
        let before = app.grid_cache.grid_version();
        app.open_session(dir.clone(), session::read(&dir).unwrap());
        assert_eq!(app.grid_cache.grid_version(), before + 1);
        assert!(app.settings == stored.settings);
        assert!(matches!(
            app.take_jobs().as_slice(),
            [(Job::Export { .. }, _)]
        ));
        app.sync(Rect::new(0, 0, 1, 1));
        assert!(!app.modified());
    }

    #[test]
    fn session_save_default_path_tracks_submitted_snapshot_and_writes_back() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("photo.png");
        RgbaImage::new(2, 3).save(&source).unwrap();
        let original = std::fs::read(&source).unwrap();
        let mut app = App::builtin(RgbaImage::new(2, 3), None);
        app.open_image(source.clone(), RgbaImage::new(2, 3));
        assert!(!app.modified());
        press(&mut app, KeyCode::Char('n'));
        assert!(app.modified());
        ctrl(&mut app, 's');
        app.settings.flip = true;
        app.edits.edit(|doc| {
            doc.add_layer("Later".into());
        });
        run_session_job(&mut app);
        let dir = temp.path().join("photo.rthid");
        assert_eq!(app.session.as_ref(), Some(&dir));
        assert_eq!(app.notice(), Some("saved photo.rthid"));
        assert_eq!(session::read(&dir).unwrap().document.layers().len(), 1);
        assert!(app.modified());
        ctrl(&mut app, 's');
        run_session_job(&mut app);
        assert!(!app.modified());
        assert_eq!(std::fs::read(&source).unwrap(), original);
        let document = app.edits.current().clone();
        app.open_image("other.png".into(), RgbaImage::new(1, 1));
        assert!(app.session.is_none());
        assert!(!app.modified());
        app.request_open(dir.clone(), None);
        run_session_job(&mut app);
        assert_eq!(app.session.as_ref(), Some(&dir));
        assert_eq!(
            app.source_path(),
            Some(session::read(&dir).unwrap().source_path.as_path())
        );
        assert_eq!(app.edits.current(), &document);
        assert!(app.settings.flip);
        assert!(!app.edits.can_undo());
        assert!(!app.modified());
        ctrl(&mut app, 's');
        assert!(matches!(&app.outbox[0].0, Job::Save { dir: target, .. } if target == &dir));
        run_session_job(&mut app);
        assert!(!dir.join("source.rthid").exists());
    }

    #[test]
    fn session_overwrite_confirmation_refuses_other_targets_and_can_cancel() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("photo.png");
        let target = temp.path().join("photo.rthid");
        let mut app = App::builtin(RgbaImage::new(2, 3), None);
        app.open_image(source.clone(), RgbaImage::new(2, 3));
        std::fs::write(&target, b"unrelated").unwrap();
        ctrl(&mut app, 's');
        assert!(app.notice().unwrap().contains("not a session directory"));
        assert!(app.take_jobs().is_empty());
        std::fs::remove_file(&target).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("notes.txt"), b"unrelated").unwrap();
        ctrl(&mut app, 's');
        assert!(app.notice().unwrap().contains("not a session directory"));
        std::fs::remove_file(target.join("notes.txt")).unwrap();
        std::fs::remove_dir(&target).unwrap();
        ctrl(&mut app, 's');
        run_session_job(&mut app);
        app.open_image(source, RgbaImage::new(2, 3));
        ctrl(&mut app, 's');
        assert!(
            matches!(&app.modal, Some(Modal::Confirm { question, .. }) if question.message() == "Overwrite photo.rthid?")
        );
        press(&mut app, KeyCode::Char('n'));
        assert!(app.modal.is_none());
        assert!(app.take_jobs().is_empty());
        ctrl(&mut app, 's');
        press(&mut app, KeyCode::Char('y'));
        run_session_job(&mut app);
        assert_eq!(app.session.as_ref(), Some(&target));
    }

    #[test]
    fn session_unsaved_quit_open_and_cancel_restore_the_dialog() {
        let mut app = App::builtin(RgbaImage::new(2, 3), None);
        app.settings.flip = true;
        assert!(app.modified());
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.quit);
        press(&mut app, KeyCode::Char('n'));
        assert!(!app.quit);
        press(&mut app, KeyCode::Char('q'));
        press(&mut app, KeyCode::Char('y'));
        assert!(app.quit);
        app.quit = false;
        app.request_open("next.png".into(), None);
        assert!(app.take_jobs().is_empty());
        press(&mut app, KeyCode::Char('n'));
        assert!(app.take_jobs().is_empty());
        app.open_dialog(".".into());
        app.apply(Effect::Open("next.png".into(), ".".into()))
            .unwrap();
        assert!(matches!(app.modal, Some(Modal::Confirm { .. })));
        press(&mut app, KeyCode::Char('n'));
        assert!(matches!(app.modal, Some(Modal::OpenFile(_))));
        app.apply(Effect::Open("next.png".into(), ".".into()))
            .unwrap();
        press(&mut app, KeyCode::Char('y'));
        assert!(matches!(
            app.take_jobs().as_slice(),
            [(Job::Open { .. }, _)]
        ));
        app.complete(done(&app, opened_photo()));
        assert!(!app.modified());
        app.settings.flip = true;
        ctrl(&mut app, 'c');
        assert!(app.quit);
    }

    #[test]
    fn session_modified_tracks_view_and_selection_but_excludes_brush_controls() {
        let mut app = App::builtin(RgbaImage::new(2, 3), None);
        app.brush_on = true;
        app.brush.radius += 1;
        assert!(!app.modified());
        app.mass_minimap = !app.mass_minimap;
        assert!(app.modified());
        app.mass_minimap = !app.mass_minimap;
        assert!(!app.modified());
        app.viewport.hide_minimap = true;
        app.viewport.zoom = Zoom::In(2);
        app.viewport.x = 200;
        app.viewport.y = 300;
        app.sync(Rect::new(0, 0, 8, 4));
        assert!(!app.modified());
        app.record_saved();
        app.edits.edit(|doc| {
            doc.add_layer("One".into());
        });
        app.record_saved();
        app.edits.select(None);
        assert!(app.modified());
        app.record_saved();
        app.edits.edit(|doc| doc.set_visible(0, false));
        assert!(app.edits.undo());
        assert!(app.modified());
    }

    #[test]
    fn session_save_ignores_modal_overlay_and_absent_source() {
        let mut app = App::empty();
        ctrl(&mut app, 's');
        assert!(app.take_jobs().is_empty());
        app.open_image("photo.png".into(), RgbaImage::new(2, 3));
        app.keys_open = true;
        ctrl(&mut app, 's');
        assert!(app.take_jobs().is_empty());
        app.keys_open = false;
        app.open_dialog(".".into());
        ctrl(&mut app, 's');
        assert!(app.take_jobs().is_empty());
        assert!(app.modal.is_some());
        ctrl(&mut app, 'c');
        assert!(app.quit);
    }

    #[test]
    fn session_failed_save_and_late_completion_never_mark_new_document_saved() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = App::builtin(RgbaImage::new(2, 3), None);
        let source = temp.path().join("photo.png");
        app.open_image(source.clone(), RgbaImage::new(2, 3));
        app.settings.flip = true;
        ctrl(&mut app, 's');
        let (job, tag) = app.take_jobs().pop().unwrap();
        let Job::Save { dir, .. } = job else {
            panic!("expected save")
        };
        app.complete(Done::new(
            tag,
            std::time::Duration::ZERO,
            IoOutcome::Saved(Err(rthid_io::io::IoError {
                op: rthid_io::io::Op::Save,
                path: dir,
                cause: rthid_io::io::Cause::Io(std::io::Error::other("injected failure")),
            })),
        ));
        assert!(app.modified());
        assert!(app.session.is_none());
        assert!(app.notice().unwrap().contains("injected failure"));
        ctrl(&mut app, 's');
        app.open_image(source, RgbaImage::new(2, 3));
        app.settings.flip = true;
        run_session_job(&mut app);
        assert!(app.session.is_none());
        assert!(app.modified());
    }

    #[test]
    fn failed_first_save_leftovers_ask_to_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = App::builtin(RgbaImage::new(2, 3), None);
        let source = temp.path().join("photo.png");
        app.open_image(source.clone(), RgbaImage::new(2, 3));
        let dir = session::default_path(&source);
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("source-1.png"), b"partial").unwrap();
        ctrl(&mut app, 's');
        assert!(
            matches!(&app.modal, Some(Modal::Confirm { question, .. }) if question.message() == "Overwrite photo.rthid?")
        );
    }

    #[test]
    fn brush_rows_adjust_reset_and_leave_layers_addressable() {
        let mut app = App::empty();
        press(&mut app, KeyCode::Char('b'));
        assert!(!app.brush_on);
        for (index, row) in BrushRow::ALL.into_iter().enumerate() {
            let item = Setting::ALL.len() + index;
            assert_eq!(app.panel_row(item), PanelItem::Brush(row));
            app.panel.select(Row::Item(item), &app.panel_lens());
            assert_eq!(app.selected_setting(), None);
            press(&mut app, KeyCode::Char('h'));
            assert!(row.changed(app.brush));
            press(&mut app, KeyCode::Char('l'));
            if row != BrushRow::Mode {
                assert!(!row.changed(app.brush));
            }
            press(&mut app, KeyCode::Right);
            assert!(row.changed(app.brush));
            press(&mut app, KeyCode::Char('d'));
            assert!(!row.changed(app.brush));
            press(&mut app, KeyCode::Left);
            press(&mut app, KeyCode::Char('d'));
        }
        assert_eq!(app.brush, Brush::default());
        assert!(!app.edits.can_undo());
        app.open_image("source.png".into(), RgbaImage::new(10, 10));
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.panel_row(Setting::ALL.len() + 3), PanelItem::Layer(0));
    }

    #[test]
    fn brush_stroke_is_one_undo_and_rebuilds_brighter_grid() {
        let mut app = App::builtin(
            RgbaImage::from_pixel(128, 128, image::Rgba([100, 100, 100, 255])),
            None,
        );
        let screen = Rect::new(0, 0, 100, 40);
        let layout = crate::ui::layout(screen);
        app.viewport.zoom = Zoom::In(1);
        app.sync(layout.preview);
        press(&mut app, KeyCode::Char('b'));
        assert!(app.brush_on);
        let geometry = app.geometry(layout.preview);
        let (x, y) = (geometry.target.x + 4, geometry.target.y + 4);
        let center = app.source_point(&geometry, x, y).unwrap();
        let before = app.edits.current().clone();
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            x,
            y,
            &geometry,
            screen,
        );
        assert!(app.stroke.is_some());
        assert_eq!(app.edits.current().layers()[0].name, "Layer 1");
        let version = app.grid_cache.grid_version();
        app.sync(layout.preview);
        assert!(app.grid_cache.grid_version() > version);
        let grid = app.grid_cache.grid().unwrap();
        assert!(grid.get_pixel(center.0.round() as u32, center.1.round() as u32)[0] > 100);
        for offset in [3, 7, 15] {
            app.mouse_at(
                MouseEventKind::Drag(MouseButton::Left),
                x + offset,
                y,
                &geometry,
                screen,
            );
            app.sync(layout.preview);
        }
        app.mouse_at(
            MouseEventKind::Up(MouseButton::Left),
            x + 15,
            y,
            &geometry,
            screen,
        );
        assert!(app.stroke.is_none());
        assert_eq!((app.viewport.x, app.viewport.y), (0, 0));
        let painted = app.edits.current().clone();
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.edits.current(), &before);
        assert!(!app.edits.can_undo());
        press(&mut app, KeyCode::Char('U'));
        assert_eq!(app.edits.current(), &painted);
        assert!(app.brush_on);
        app.sync(layout.preview);
        let before = app.edits.current().clone();
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            x,
            y,
            &geometry,
            screen,
        );
        app.mouse_at(
            MouseEventKind::Drag(MouseButton::Left),
            x + 5,
            y + 2,
            &geometry,
            screen,
        );
        app.mouse_at(
            MouseEventKind::Up(MouseButton::Left),
            x + 5,
            y + 2,
            &geometry,
            screen,
        );
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.edits.current(), &before);
    }

    #[test]
    fn brush_drag_without_a_dab_keeps_revision() {
        let mut app = App::builtin(RgbaImage::new(128, 128), None);
        let screen = Rect::new(0, 0, 100, 40);
        let area = crate::ui::layout(screen).preview;
        app.viewport.zoom = Zoom::In(1);
        app.brush_on = true;
        app.brush.radius = 256;
        let geometry = app.geometry(area);
        let (x, y) = (geometry.target.x + 4, geometry.target.y + 4);
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            x,
            y,
            &geometry,
            screen,
        );
        let revision = app.edits.revision();
        assert!(!app.mouse_at(
            MouseEventKind::Drag(MouseButton::Left),
            x + 1,
            y,
            &geometry,
            screen,
        ));
        assert_eq!(app.edits.revision(), revision);
    }

    #[test]
    fn brush_drag_across_margin_does_not_join_exit_and_entry() {
        let mut app = App::builtin(RgbaImage::new(128, 128), None);
        let screen = Rect::new(0, 0, 100, 40);
        let area = crate::ui::layout(screen).preview;
        app.viewport.zoom = Zoom::In(1);
        app.brush_on = true;
        app.brush.radius = 1;
        app.brush.strength = 100;
        let geometry = app.geometry(area);
        let (x, y) = (geometry.target.x + 4, geometry.target.y + 4);
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            x,
            y,
            &geometry,
            screen,
        );
        let before = app.edits.current().clone();
        let outside = geometry.target.x - 1;
        assert!(app.source_point(&geometry, outside, y).is_none());
        assert!(!app.mouse_at(
            MouseEventKind::Drag(MouseButton::Left),
            outside,
            y,
            &geometry,
            screen,
        ));
        assert!(app.mouse_at(
            MouseEventKind::Drag(MouseButton::Left),
            x + 24,
            y,
            &geometry,
            screen,
        ));
        let entry = app.source_point(&geometry, x + 24, y).unwrap();
        let map = &app.edits.current().layers()[0].exposure;
        assert!(map.get(entry.0.round() as u32, entry.1.round() as u32) > 0);
        for column in x + 2..x + 23 {
            let point = app.source_point(&geometry, column, y).unwrap();
            let (px, py) = (point.0.round() as u32, point.1.round() as u32);
            assert_eq!(map.get(px, py), before.layers()[0].exposure.get(px, py));
        }
        app.mouse_at(
            MouseEventKind::Up(MouseButton::Left),
            x + 24,
            y,
            &geometry,
            screen,
        );
        assert!(app.edits.undo());
        assert!(app.edits.current().layers().is_empty());
        assert!(!app.edits.can_undo());
    }

    #[test]
    fn unchanged_strokes_restore_redo_with_and_without_a_layer() {
        for has_layer in [false, true] {
            let mut app = App::builtin(RgbaImage::new(8, 8), None);
            if has_layer {
                let mut document = Document::new(8, 8);
                document.add_layer(document.next_layer_name());
                for y in 0..8 {
                    for x in 0..8 {
                        document.target_mut().unwrap().exposure.add(x, y, i16::MAX);
                    }
                }
                app.edits = History::new(document);
            } else {
                app.brush.strength = 0;
            }
            let before = app.edits.current().clone();
            app.edits.edit(|doc| {
                doc.add_layer("Redo".into());
            });
            let redo = app.edits.current().clone();
            assert!(app.edits.undo());
            app.start_stroke((4.0, 4.0));
            app.end_stroke();
            assert_eq!(app.edits.current(), &before);
            assert!(!app.edits.can_undo());
            assert!(app.edits.redo());
            assert_eq!(app.edits.current(), &redo);
        }
    }

    #[test]
    fn brush_toggle_ends_stroke_and_history_chords_undo_and_redo_it() {
        let mut empty = App::empty();
        press(&mut empty, KeyCode::Char('B'));
        assert_eq!(empty.brush.mode.name(), "Dodge");
        let mut app = App::builtin(RgbaImage::new(16, 16), None);
        press(&mut app, KeyCode::Char('B'));
        assert_eq!(app.brush.mode.name(), "Burn");
        assert!(!app.brush_on);
        app.brush_on = true;
        let screen = Rect::new(0, 0, 100, 40);
        let geometry = app.geometry(crate::ui::layout(screen).preview);
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            geometry.target.x,
            geometry.target.y,
            &geometry,
            screen,
        );
        assert!(app.stroke.is_some());
        press(&mut app, KeyCode::Char('B'));
        assert!(app.stroke.is_none());
        assert_eq!(app.brush.mode.name(), "Dodge");
        let painted = app.edits.current().clone();
        ctrl(&mut app, 'z');
        assert!(app.edits.current().layers().is_empty());
        for redo in [
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
            KeyEvent::new(
                KeyCode::Char('z'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
            KeyEvent::new(
                KeyCode::Char('Z'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            ),
        ] {
            app.handle_key(redo, &geometry);
            assert_eq!(app.edits.current(), &painted);
            ctrl(&mut app, 'z');
        }
        app.handle_key(
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
            &geometry,
        );
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            geometry.target.x,
            geometry.target.y,
            &geometry,
            screen,
        );
        assert!(app.stroke.is_some());
        ctrl(&mut app, 'z');
        assert!(app.stroke.is_none());
        assert_eq!(app.edits.current(), &painted);
        for overlay in [false, true] {
            app.keys_open = overlay;
            app.modal = (!overlay).then(Modal::templates);
            let revision = app.edits.revision();
            for c in ['z', 'y'] {
                ctrl(&mut app, c);
            }
            assert_eq!(app.edits.revision(), revision);
        }
        app.keys_open = false;
        app.modal = None;
        ctrl(&mut app, 'z');
        ctrl(&mut app, 'z');
        assert_eq!(app.notice(), Some("nothing to undo"));
        while app.edits.can_redo() {
            ctrl(&mut app, 'y');
        }
        ctrl(&mut app, 'y');
        assert_eq!(app.notice(), Some("nothing to redo"));
    }

    #[test]
    fn brush_at_fit_paints_source_pixels_at_the_drawn_edge() {
        let mut app = App::builtin(RgbaImage::new(16, 16), None);
        let screen = Rect::new(0, 0, 100, 40);
        app.brush_on = true;
        app.brush.radius = 1;
        let geometry = app.geometry(crate::ui::layout(screen).preview);
        assert_eq!(geometry.zoom, Zoom::Fit);
        let (x, y) = (geometry.target.right() - 1, geometry.target.bottom() - 1);
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            x,
            y,
            &geometry,
            screen,
        );
        app.mouse_at(
            MouseEventKind::Up(MouseButton::Left),
            x,
            y,
            &geometry,
            screen,
        );
        assert!(app.edits.current().layers()[0].exposure.get(15, 15) > 0);
    }

    #[test]
    fn saturated_stroke_does_not_add_an_undo_step() {
        for can_undo in [false, true] {
            let mut app = App::builtin(RgbaImage::new(128, 128), None);
            let mut document = Document::new(128, 128);
            document.add_layer(document.next_layer_name());
            let map = &mut document.target_mut().unwrap().exposure;
            for y in 0..128 {
                for x in 0..128 {
                    map.add(x, y, i16::MAX);
                }
            }
            if can_undo {
                app.edits.edit(|doc| *doc = document.clone());
            } else {
                app.edits = History::new(document.clone());
            }
            let screen = Rect::new(0, 0, 100, 40);
            let area = crate::ui::layout(screen).preview;
            app.viewport.zoom = Zoom::In(1);
            app.brush_on = true;
            let geometry = app.geometry(area);
            let (x, y) = (geometry.target.x + 4, geometry.target.y + 4);
            app.mouse_at(
                MouseEventKind::Down(MouseButton::Left),
                x,
                y,
                &geometry,
                screen,
            );
            app.mouse_at(
                MouseEventKind::Drag(MouseButton::Left),
                x + 8,
                y,
                &geometry,
                screen,
            );
            app.mouse_at(
                MouseEventKind::Up(MouseButton::Left),
                x + 8,
                y,
                &geometry,
                screen,
            );
            assert_eq!(app.edits.current(), &document);
            assert_eq!(app.edits.can_undo(), can_undo);
            let revision = app.edits.revision();
            app.end_stroke();
            assert_eq!(app.edits.revision(), revision);
            if can_undo {
                assert!(app.edits.undo());
                assert!(app.edits.current().layers().is_empty());
            }
        }
    }

    #[test]
    fn brush_rejects_hidden_target_and_minimap_and_paper_presses() {
        let mut app = App::builtin(RgbaImage::new(128, 128), None);
        let screen = Rect::new(0, 0, 100, 40);
        let area = crate::ui::layout(screen).preview;
        app.viewport.zoom = Zoom::In(1);
        app.brush_on = true;
        let geometry = app.geometry(area);
        let map = geometry.minimap.unwrap();
        let revision = app.edits.revision();
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            map.map.x,
            map.map.y,
            &geometry,
            screen,
        );
        assert!(app.stroke.is_none());
        assert!(app.edits.current().layers().is_empty());
        assert_eq!(app.edits.revision(), revision);
        app.mouse_at(
            MouseEventKind::Drag(MouseButton::Left),
            map.map.right() - 1,
            map.map.bottom() - 1,
            &geometry,
            screen,
        );
        assert!(app.viewport.x > 0 || app.viewport.y > 0);
        app.mouse_at(
            MouseEventKind::Up(MouseButton::Left),
            map.map.x,
            map.map.y,
            &geometry,
            screen,
        );
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Char('v'));
        let revision = app.edits.revision();
        let geometry = app.geometry(area);
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            geometry.target.x,
            geometry.target.y + 4,
            &geometry,
            screen,
        );
        assert_eq!(app.notice(), Some("target layer is hidden"));
        assert!(app.stroke.is_none());
        app.mouse_at(
            MouseEventKind::Drag(MouseButton::Left),
            geometry.target.x + 4,
            geometry.target.y + 4,
            &geometry,
            screen,
        );
        assert_eq!(app.edits.revision(), revision);
        assert!(app.edits.current().layers()[0].exposure.is_empty());
        app.settings.margin = 10;
        app.viewport = Viewport::default();
        let geometry = app.geometry(area);
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            geometry.target.x,
            geometry.target.y,
            &geometry,
            screen,
        );
        assert!(app.stroke.is_none());
        assert_eq!(app.edits.revision(), revision);
    }

    #[test]
    fn stroke_ends_on_unrelated_input_and_paints_at_fit() {
        let mut app = App::builtin(RgbaImage::new(128, 128), None);
        let screen = Rect::new(0, 0, 100, 40);
        let area = crate::ui::layout(screen).preview;
        app.brush_on = true;
        let geometry = app.geometry(area);
        let (x, y) = (geometry.target.x + 2, geometry.target.y + 2);
        for event in 0..6 {
            app.mouse_at(
                MouseEventKind::Down(MouseButton::Left),
                x,
                y,
                &geometry,
                screen,
            );
            assert!(app.stroke.is_some());
            match event {
                0 => {
                    app.mouse_at(MouseEventKind::Moved, x, y, &geometry, screen);
                }
                1 => {
                    app.mouse_at(
                        MouseEventKind::Drag(MouseButton::Right),
                        x,
                        y,
                        &geometry,
                        screen,
                    );
                }
                2 => app.handle_key(
                    KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE),
                    &geometry,
                ),
                3 => app.handle_paste("text"),
                4 => app.end_stroke(),
                _ => app.handle_key(
                    KeyEvent::new_with_kind(
                        KeyCode::Char('z'),
                        KeyModifiers::NONE,
                        KeyEventKind::Release,
                    ),
                    &geometry,
                ),
            }
            assert!(app.stroke.is_none());
            let revision = app.edits.revision();
            app.mouse_at(
                MouseEventKind::Drag(MouseButton::Left),
                x + 2,
                y,
                &geometry,
                screen,
            );
            assert_eq!(app.edits.revision(), revision);
            assert_eq!((app.viewport.x, app.viewport.y), (0, 0));
        }
        app.viewport.zoom = Zoom::In(1);
        app.preview_focus = true;
        let geometry = app.geometry(area);
        let revision = app.edits.revision();
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), &geometry);
        assert!(app.viewport.x > 0);
        assert_eq!(app.edits.revision(), revision);
    }

    #[test]
    fn layer_keys_history_targets_names_and_cursor() {
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.notice(), Some("nothing to undo"));
        press(&mut app, KeyCode::Char('U'));
        assert_eq!(app.notice(), Some("nothing to redo"));
        for _ in 0..2 {
            press(&mut app, KeyCode::Char('n'));
        }
        assert_eq!(app.edits.current().layers()[0].name, "Layer 1");
        assert_eq!(app.edits.current().layers()[1].name, "Layer 2");
        assert_eq!(
            app.panel_row(Setting::ALL.len() + BrushRow::ALL.len()),
            PanelItem::Layer(1)
        );
        assert_eq!(
            app.panel_row(Setting::ALL.len() + BrushRow::ALL.len() + 1),
            PanelItem::Layer(0)
        );
        assert_eq!(app.selected_setting(), None);
        let revision = app.edits.revision();
        let versions = versions(&app);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.edits.current().selected(), None);
        assert_eq!(app.edits.current().target(), Some(1));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.edits.current().selected(), Some(1));
        assert_eq!(app.edits.revision(), revision);
        assert_eq!(versions, self::versions(&app));
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(app.edits.revision(), revision);
        press(&mut app, KeyCode::Char('['));
        assert_eq!(app.edits.current().layers()[0].name, "Layer 2");
        assert_eq!(
            app.panel.selected(&app.panel_lens()),
            Some(app.layer_row(0))
        );
        let revision = app.edits.revision();
        press(&mut app, KeyCode::Char('['));
        assert_eq!(app.edits.revision(), revision);
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('v'));
        assert!(!app.edits.current().layers()[1].visible);
        app.panel.select(app.layer_row(0), &app.panel_lens());
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(
            app.panel.selected(&app.panel_lens()),
            Some(app.layer_row(0))
        );
        press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.edits.current().layers()[1].name, "Layer 1");
        app.preview_focus = true;
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.edits.current().layers().len(), 1);
        assert_eq!(
            app.panel.selected(&app.panel_lens()),
            Some(app.layer_row(0))
        );
        press(&mut app, KeyCode::Char('U'));
        assert_eq!(app.edits.current().layers().len(), 2);
        app.preview_focus = false;
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(
            app.panel.selected(&app.panel_lens()),
            Some(Row::Header(LAYER_GROUP))
        );
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(
            app.panel.selected(&app.panel_lens()),
            Some(Row::Header(LAYER_GROUP))
        );
        press(&mut app, KeyCode::Char('n'));
        app.panel.select(Row::Item(0), &app.panel_lens());
        app.open_image("next.png".into(), RgbaImage::new(2, 2));
        assert_eq!(app.panel.selected(&app.panel_lens()), Some(Row::Item(0)));
        assert!(!app.edits.can_undo());
    }

    #[test]
    fn layer_keys_require_source_and_modal_keeps_input() {
        let mut app = App::empty();
        for code in ['n', 'u', 'U', 'v', 'x', '[', ']'] {
            press(&mut app, KeyCode::Char(code));
        }
        assert_eq!(app.edits.revision(), 0);
        assert!(app.notice().is_none());
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        app.show_modal(Modal::templates());
        let revision = app.edits.revision();
        for code in ['n', 'u', 'U', 'v', 'x', '[', ']'] {
            press(&mut app, KeyCode::Char(code));
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.edits.revision(), revision);
        assert!(app.edits.current().layers().is_empty());
    }

    #[test]
    fn layer_click_after_adding_without_redraw_ignores_stale_layout() {
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        press(&mut app, KeyCode::Char('n'));
        let screen = Rect::new(0, 0, 100, 40);
        let layout = crate::ui::layout(screen);
        let geometry = app.geometry(layout.preview);
        render_panel(&app, layout.panel);
        let top = layout.panel.y + 1 + 5 + (Setting::ALL.len() + BrushRow::ALL.len()) as u16;
        let mark_x = layout.panel.x + 3;
        press(&mut app, KeyCode::Char('n'));
        let revision = app.edits.revision();
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            mark_x,
            top,
            &geometry,
            screen,
        );
        assert_eq!(app.edits.revision(), revision);
        assert!(
            app.edits
                .current()
                .layers()
                .iter()
                .all(|layer| layer.visible)
        );
        render_panel(&app, layout.panel);
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            mark_x,
            top,
            &geometry,
            screen,
        );
        assert!(app.edits.current().layers()[0].visible);
        assert!(!app.edits.current().layers()[1].visible);
    }

    #[test]
    fn layer_click_after_reorder_without_redraw_ignores_stale_layout() {
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Char('n'));
        let screen = Rect::new(0, 0, 100, 40);
        let layout = crate::ui::layout(screen);
        let geometry = app.geometry(layout.preview);
        render_panel(&app, layout.panel);
        let top = layout.panel.y + 1 + 5 + (Setting::ALL.len() + BrushRow::ALL.len()) as u16;
        let mark_x = layout.panel.x + 3;
        press(&mut app, KeyCode::Char('['));
        let revision = app.edits.revision();
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            mark_x,
            top,
            &geometry,
            screen,
        );
        assert_eq!(app.edits.revision(), revision);
        render_panel(&app, layout.panel);
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            mark_x,
            top,
            &geometry,
            screen,
        );
        assert!(!app.edits.current().layers()[1].visible);
        assert!(app.edits.current().layers()[0].visible);
    }

    #[test]
    fn layer_mouse_selects_marks_and_groups_drag_into_one_undo() {
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        for _ in 0..3 {
            press(&mut app, KeyCode::Char('n'));
        }
        let screen = Rect::new(0, 0, 100, 40);
        let layout = crate::ui::layout(screen);
        let geometry = app.geometry(layout.preview);
        render_panel(&app, layout.panel);
        let top = layout.panel.y + 1 + 5 + (Setting::ALL.len() + BrushRow::ALL.len()) as u16;
        let x = layout.panel.x + 5;
        let click = MouseEventKind::Down(MouseButton::Left);
        app.mouse_at(click, x, top + 1, &geometry, screen);
        assert_eq!(app.edits.current().selected(), Some(1));
        app.mouse_at(
            MouseEventKind::Up(MouseButton::Left),
            x,
            top + 1,
            &geometry,
            screen,
        );
        app.mouse_at(click, x, top + 1, &geometry, screen);
        assert_eq!(app.edits.current().selected(), Some(1));
        app.mouse_at(
            MouseEventKind::Up(MouseButton::Left),
            x,
            top + 1,
            &geometry,
            screen,
        );
        assert_eq!(app.edits.current().selected(), None);
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 40)).unwrap();
        let mut renderer = crate::ui::Renderer::default();
        app.sync(layout.preview);
        terminal
            .draw(|frame| crate::ui::draw(frame, &app, &layout, &mut renderer))
            .unwrap();
        let mark_x = layout.panel.x + 3;
        assert_eq!(terminal.backend().buffer()[(mark_x, top + 1)].symbol(), "●");
        let revision = app.edits.revision();
        app.mouse_at(click, mark_x, top + 1, &geometry, screen);
        assert!(!app.edits.current().layers()[1].visible);
        assert_eq!(app.edits.current().selected(), None);
        assert_eq!(app.edits.revision(), revision + 1);
        assert!(app.layer_drag.is_none());
        app.mouse_at(
            MouseEventKind::Drag(MouseButton::Left),
            x,
            top + 2,
            &geometry,
            screen,
        );
        assert_eq!(app.edits.current().layers()[1].name, "Layer 2");
        assert_eq!(app.edits.revision(), revision + 1);
        app.mouse_at(click, x, top, &geometry, screen);
        app.mouse_at(
            MouseEventKind::Up(MouseButton::Left),
            x,
            top,
            &geometry,
            screen,
        );
        app.mouse_at(click, x, top, &geometry, screen);
        assert_eq!(app.edits.current().selected(), Some(2));
        let revision = app.edits.revision();
        let drag = MouseEventKind::Drag(MouseButton::Left);
        app.mouse_at(drag, x, top + 1, &geometry, screen);
        app.mouse_at(drag, x, top + 2, &geometry, screen);
        // The release after a moved drag redraws so the panel accepts clicks again.
        assert!(app.mouse_at(
            MouseEventKind::Up(MouseButton::Left),
            x,
            top + 2,
            &geometry,
            screen,
        ));
        assert!(app.layer_drag.is_none());
        assert_eq!(app.edits.revision(), revision + 2);
        assert_eq!(app.edits.current().layers()[0].name, "Layer 3");
        assert_eq!(app.edits.current().selected(), Some(0));
        assert_eq!(app.edits.current().target(), Some(0));
        assert_eq!(
            app.panel.selected(&app.panel_lens()),
            Some(app.layer_row(0))
        );
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(
            app.edits
                .current()
                .layers()
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>(),
            ["Layer 1", "Layer 2", "Layer 3"]
        );
        press(&mut app, KeyCode::Char('u'));
        assert!(app.edits.current().layers()[1].visible);
    }

    #[test]
    fn layer_drag_back_to_start_discards_undo_step() {
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        for _ in 0..2 {
            press(&mut app, KeyCode::Char('n'));
        }
        press(&mut app, KeyCode::Char('v'));
        press(&mut app, KeyCode::Char('u'));
        let before = app.edits.current().clone();
        let screen = Rect::new(0, 0, 100, 40);
        let layout = crate::ui::layout(screen);
        let geometry = app.geometry(layout.preview);
        render_panel(&app, layout.panel);
        let top = layout.panel.y + 1 + 5 + (Setting::ALL.len() + BrushRow::ALL.len()) as u16;
        let x = layout.panel.x + 5;
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            x,
            top,
            &geometry,
            screen,
        );
        app.mouse_at(
            MouseEventKind::Drag(MouseButton::Left),
            x,
            top + 1,
            &geometry,
            screen,
        );
        app.mouse_at(
            MouseEventKind::Drag(MouseButton::Left),
            x,
            top,
            &geometry,
            screen,
        );
        app.mouse_at(
            MouseEventKind::Up(MouseButton::Left),
            x,
            top,
            &geometry,
            screen,
        );
        assert_eq!(app.edits.current(), &before);
        press(&mut app, KeyCode::Char('U'));
        assert!(!app.edits.current().layers()[1].visible);
        press(&mut app, KeyCode::Char('u'));
        assert!(app.edits.current().layers()[1].visible);
    }

    #[test]
    fn image_export_updates_for_dot_size_and_rotation() {
        let mut app = App::new(
            Some(("test.png".into(), RgbaImage::new(30, 20))),
            Some("preview.png".into()),
            None,
            Some(254),
            Loaded::default(),
        );
        assert_eq!(
            encoded_export(jobs(&mut app).pop().unwrap()).1.dimensions(),
            (60, 40)
        );
        app.view = View::Mass;
        app.refresh(false);
        assert!(jobs(&mut app).is_empty());
        let before = versions(&app);
        let mass_version = app.grid_cache.mass_version();
        app.settings.dot = 40;
        app.refresh(false);
        assert_eq!(versions(&app), before);
        assert_eq!(app.grid_cache.mass_version(), mass_version);
        assert_eq!(app.view, View::Mass);
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
            let mut app = App::new(
                None,
                Some("preview.png".into()),
                None,
                dpi,
                Loaded::default(),
            );
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
            None,
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

    #[test]
    fn mass_svg_queues_once_and_writes_svg() {
        let path = PathBuf::from("mass.svg");
        let mut app = App::new(
            Some(("test.png".into(), RgbaImage::new(8, 6))),
            None,
            Some(path.clone()),
            None,
            Loaded::default(),
        );
        let queued = jobs(&mut app);
        assert_eq!(queued.len(), 1);
        let (output, bytes) = encoded_bytes(queued.into_iter().next().unwrap());
        assert_eq!(output, path);
        assert!(bytes.starts_with(b"<svg"));
        app.refresh(false);
        assert!(jobs(&mut app).is_empty());
        app.finish();
        assert!(jobs(&mut app).is_empty());
    }

    #[test]
    fn mass_svg_reports_a_failed_mass_build_and_recovers() {
        let path = PathBuf::from("mass.svg");
        let mut app = App::new(None, None, Some(path.clone()), None, Loaded::default());
        app.mass.sigma = -1.0;
        app.open_image("test.png".into(), RgbaImage::new(8, 6));
        assert!(jobs(&mut app).is_empty());
        assert!(
            app.export_status()
                .is_some_and(|status| status.starts_with("SVG mass.svg: mass study:"))
        );
        assert!(app.export_result().is_err());

        app.mass = MassOptions::default();
        app.refresh(false);
        assert!(
            matches!(jobs(&mut app).as_slice(), [Job::Export { path: output, .. }] if output == &path)
        );
        app.complete(done(&app, IoOutcome::Exported(Ok(path))));
        assert!(app.export_status().is_none());
        assert!(app.export_result().is_ok());
    }

    #[test]
    fn png_and_mass_svg_queue_together_and_svg_errors_surface() {
        let svg = PathBuf::from("mass.svg");
        let mut app = App::new(
            Some(("test.png".into(), RgbaImage::new(8, 6))),
            Some("preview.png".into()),
            Some(svg.clone()),
            None,
            Loaded::default(),
        );
        let queued = jobs(&mut app);
        assert_eq!(queued.len(), 2);
        assert!(
            queued
                .iter()
                .any(|job| matches!(job, Job::Export { path, .. } if path == &svg))
        );
        let error = rthid_io::io::IoError {
            op: rthid_io::io::Op::Encode,
            path: svg,
            cause: rthid_io::io::Cause::Io(std::io::Error::other("denied")),
        };
        app.complete(done(&app, IoOutcome::Exported(Err(error))));
        assert_eq!(app.export_status(), Some("SVG mass.svg: denied"));
        app.complete(done(&app, IoOutcome::Exported(Ok("preview.png".into()))));
        assert_eq!(app.notice(), Some("SVG mass.svg: denied"));
    }

    fn encoded_bytes(job: Job) -> (PathBuf, Vec<u8>) {
        let Job::Export { path, write } = job else {
            panic!("expected export");
        };
        let mut file = tempfile::tempfile().unwrap();
        write(&mut file).unwrap();
        use std::io::{Read, Seek};
        file.rewind().unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        (path, bytes)
    }

    fn encoded_export(job: Job) -> (PathBuf, image::RgbaImage) {
        let (path, bytes) = encoded_bytes(job);
        (path, image::load_from_memory(&bytes).unwrap().to_rgba8())
    }

    fn failure(op: rthid_io::io::Op) -> rthid_io::io::IoError {
        rthid_io::io::IoError {
            op,
            path: if op == rthid_io::io::Op::Encode {
                "preview.png"
            } else {
                "failed.png"
            }
            .into(),
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
                IoOutcome::Opened(Err(error))
                | IoOutcome::Listed(Err(error))
                | IoOutcome::Loaded(Err(error)) => error.to_string(),
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
        assert_eq!(app.view, View::Original);

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
                .all(|pixel| pixel.0[..3] == app.settings.palette.colors()[0].rgb)
        );
        assert!(
            matches!(queued.as_slice(), [Job::Open { path: source, dir: Some(dir), protect }]
            if protect == &vec![path.clone()] && source == &PathBuf::from("photo.png") && dir == &PathBuf::from("/images"))
        );
        app.complete(done(&app, IoOutcome::Opened(Err(failure(Op::Decode)))));
        assert!(app.complete(done(&app, IoOutcome::Exported(Err(failure(Op::Encode))))));
        let error = app.export_status().unwrap().to_owned();
        assert_eq!(app.notice(), Some(error.as_str()));
        assert!(app.export_result().is_err());
        let grid_version = app.grid_cache.grid.version();
        for code in [KeyCode::Char('r'), KeyCode::Char('q'), KeyCode::Char('y')] {
            app.handle_key(
                KeyEvent::new(code, KeyModifiers::NONE),
                &app.geometry(test_layout().preview),
            );
        }
        assert!(app.quit);
        assert_eq!(app.grid_cache.grid.version(), grid_version);
        jobs(&mut app);
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
        app.view = View::Dithered;
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
                matches!(jobs(&mut app).as_slice(), [Job::Open { path, dir: Some(base), protect }]
                if protect.is_empty() && path == &PathBuf::from("bad.png") && base == &PathBuf::from(dir))
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
                    matches!(jobs(&mut app).as_slice(), [Job::Open { path: entered, dir: Some(base), protect }]
                    if protect.is_empty() && entered == &PathBuf::from(path) && base == &PathBuf::from(dir))
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
            Job::Open { path, dir: None, protect }, Job::List(_), Job::List(listed)
        ] if protect.is_empty() && path == &PathBuf::from("/tmp/My Photo.jpg") && listed == &PathBuf::from("/other")));
        assert!(jobs(&mut app).is_empty());

        let mut app = App::empty();
        app.open_dialog("/images".into());
        app.handle_paste(&"猫\n".repeat(100_000));
        let Some(Modal::OpenFile(picker)) = &app.modal else {
            panic!("open dialog expected")
        };
        assert!(picker.typed);
        assert_eq!(picker.input.text().chars().count(), PATH_MAX_CHARS);
        app.handle_paste(&format!("/{}.png", "a".repeat(5000)));
        let Some(Modal::OpenFile(picker)) = &app.modal else {
            panic!("open dialog expected")
        };
        assert_eq!(picker.input.text().chars().count(), PATH_MAX_CHARS);
    }

    #[test]
    fn key_overlay_yields_to_new_modals() {
        let mut app = App::empty();
        app.keys_open = true;
        app.show_modal(Modal::templates());
        assert!(!app.keys_open);
        assert!(matches!(app.modal, Some(Modal::Templates { .. })));
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
            assert_eq!(app.view, View::Original);
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
        assert_eq!(app.view, View::Original);
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
        app.view = View::Dithered;
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
        assert!(app.last_drag.is_none() && app.view == View::Original);
        assert_eq!(app.settings.rotation, Rotation::R0);
        assert!(!app.settings.flip);
        assert_eq!(
            (app.settings.zoom, app.settings.pan_x, app.settings.pan_y),
            (100, 0, 0)
        );
        assert_eq!(app.settings.options(), settings.options());
        assert_eq!(app.settings.palette, settings.palette);
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
            app.view == View::Dithered && app.modal.is_none() && take_changes(&mut app).is_empty()
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
            matches!(&app.modal, Some(Modal::Editor(editor)) if editor.draft.colors == app.settings.palette.hex() && editor.rows.selected(editor.draft.colors.len() + 2) == Some(1) && editor.input.is_none())
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
        assert_eq!(app.settings.palette.name(), "Z");
        assert!(app.library.palettes.find("Z").is_some());
        assert!(
            app.modal.is_none() && !take_changes(&mut app).is_empty() && app.view == View::Dithered
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
        let mut app = App::new(None, Some("preview.png".into()), None, None, loaded);
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
        let mut app = App::new(
            None,
            Some("preview.png".into()),
            None,
            None,
            Loaded::default(),
        );
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
        let mut app = App::new(None, None, None, None, loaded);
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
                rthid_widgets::Confirm::new("Delete preset?").yes_label("delete"),
                crate::modal::OnYes::RemovePreset("Selected".into()),
            )),
        };
        let mut template_confirm = Modal::Templates {
            list: rthid_widgets::ListPicker::new(0),
            prompt: None,
            confirm: Some((
                rthid_widgets::Confirm::new("Delete template?").yes_label("delete"),
                crate::modal::OnYes::RemoveTemplate("Zine".into()),
            )),
        };
        let index = app.library.palettes.find("Selected").unwrap();
        let text = "version = 1\n[[palette]]\nname = 'Other'\ncolors = ['#fff']\n[[palette]]\nname = 'Selected'\ncolors = ['#123456']\n[[template]]\nname = 'Other'\n[[template]]\nname = 'Zine'\n[[palette]]\nname = 'Bad'\ncolors = ['oops']\n";
        let notice = crate::store::parse(text).notice();
        saved(&mut app, text);
        assert_eq!(app.settings.palette.name(), "Selected");
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
        for (code, expected) in [('h', (-2, 0)), ('j', (0, 5)), ('L', (16, 0)), ('J', (0, 5))] {
            assert_eq!(
                pan_delta(KeyCode::Char(code), (20, 12), (2, 5)),
                Some(expected),
                "{code}"
            );
        }

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
    }

    fn test_layout() -> Layout {
        crate::ui::layout(Rect::new(0, 0, 41, 9))
    }

    fn render_panel(app: &App, area: Rect) {
        let groups = crate::settings::GROUPS
            .map(|(title, len)| {
                (
                    ratatui::text::Line::raw(title),
                    vec![ratatui::text::Line::default(); len],
                )
            })
            .into_iter()
            .chain(std::iter::once((
                ratatui::text::Line::raw("Brush"),
                vec![ratatui::text::Line::default(); BrushRow::ALL.len()],
            )))
            .chain(std::iter::once((
                ratatui::text::Line::raw("Layers"),
                vec![ratatui::text::Line::default(); app.edits.current().layers().len()],
            )));
        app.panel.render(
            groups,
            Some(ratatui::widgets::Block::bordered()),
            ratatui::style::Style::default(),
            area,
            &mut ratatui::buffer::Buffer::empty(area),
        );
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
        app.panel.select(
            Row::Item(Setting::ALL.iter().position(|&row| row == setting).unwrap()),
            &app.panel_lens(),
        );
    }

    fn app_at(setting: Setting) -> App {
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        select(&mut app, setting);
        app
    }

    #[test]
    fn grid_cache_rebuilds_only_stale_stages() {
        let mut app = business_app(RgbaImage::new(20, 10), None);
        assert_eq!(app.grid_cache.grid.version(), 1);
        assert_eq!(app.grid_cache.dithered.version(), 0);
        app.view = View::Dithered;
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.dithered.version(), 1);
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.grid.version(), 1);
        assert_eq!(app.grid_cache.dithered.version(), 1);
        app.settings.strength = 0.5;
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.grid.version(), 1);
        assert_eq!(app.grid_cache.dithered.version(), 2);
        app.view = View::Original;
        app.settings.rotation = Rotation::R180;
        app.settings.dot = 19;
        app.sync(test_layout().preview);
        assert_eq!(app.grid_cache.grid.version(), 2);
        assert_eq!(app.grid_cache.dithered.version(), 2);
        app.view = View::Dithered;
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
        app.edits
            .reset(Document::new(replacement.width(), replacement.height()));
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
        assert_eq!(app.view, View::Original);
        for (focus, rebuilt) in [(false, true), (true, false)] {
            app.preview_focus = focus;
            assert_eq!(press(&mut app, KeyCode::Char(' ')), rebuilt);
            assert_eq!(app.view, View::Dithered);
            assert!(!press(&mut app, KeyCode::Char(' ')));
            assert_eq!(app.view, View::Original);
        }
        app.preview_focus = false;
        assert!(!press(&mut app, KeyCode::Right));
        assert_eq!(app.view, View::Original);
        assert!(press(&mut app, KeyCode::Left));
        assert_eq!(app.view, View::Dithered);
        assert!(!press(&mut app, KeyCode::Char(' ')));
        app.panel = Outline::new(&app.panel_lens());
        assert!(press(&mut app, KeyCode::Left));
        assert_eq!(app.view, View::Original);
        for code in [KeyCode::Char('r'), KeyCode::Char('d')] {
            assert!(press(&mut app, code));
            assert_eq!(app.view, View::Original);
        }
        app.view = View::Mass;
        select(&mut app, Setting::Strength);
        let strength = app.settings.strength;
        press(&mut app, KeyCode::Left);
        assert_ne!(app.settings.strength, strength);
        assert_eq!(app.view, View::Mass);
        app.apply(Effect::Preview(app.settings.clone())).unwrap();
        assert_eq!(app.view, View::Mass);
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
            assert_eq!(app.view, View::Mass);
            assert!(app.sync(area));
            assert_eq!(app.grid_cache.grid_version(), grid_version);
            assert_eq!(app.grid_cache.dithered.version(), 0);
            let Some(Pixels::Rgb {
                image,
                version,
                view: View::Mass,
            }) = app.displayed()
            else {
                panic!("expected mass RGB pixels");
            };
            assert_eq!(image.dimensions(), (8, 10));
            assert_eq!(version, app.grid_cache.mass_version());
            assert!(!app.sync(area));
            assert!(!press(&mut app, KeyCode::Char('a')));
            assert_eq!(app.view, View::Original);
            press(&mut app, KeyCode::Char('a'));
            press(&mut app, KeyCode::Char(' '));
            assert_eq!(app.view, View::Dithered);
            press(&mut app, KeyCode::Char('a'));
            assert_eq!(app.view, View::Mass);
            press(&mut app, KeyCode::Char('a'));
            assert_eq!(app.view, View::Dithered);
            press(&mut app, KeyCode::Char('a'));
            app.open_image(PathBuf::from("next.png"), RgbaImage::new(20, 10));
            assert_eq!(
                (app.view, app.mass_return),
                (View::Original, View::Original)
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
    fn mass_error_notice_follows_view_and_minimap_visibility() {
        let mut app = App::builtin(RgbaImage::new(400, 400), None);
        let area = Rect::new(0, 0, 69, 36);
        app.mass.sigma = -1.0;
        app.view = View::Mass;
        app.sync(area);
        let expected = format!("mass study: {}", app.grid_cache.mass_error().unwrap().0);
        assert_eq!(app.notice(), Some(expected.as_str()));
        assert!(!app.sync(area));
        assert_eq!(app.notice(), Some(expected.as_str()));
        app.set_notice("retained".into());
        assert_eq!(app.notice(), Some("retained"));
        app.notice = None;
        assert_eq!(app.notice(), Some(expected.as_str()));

        app.view = View::Original;
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

        app.view = View::Mass;
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
                view: View::Original,
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
                view: View::Mass,
                ..
            })
        ));
        assert!(matches!(
            app.displayed(),
            Some(Pixels::Rgb {
                view: View::Original,
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
                view: View::Mass,
                ..
            })
        ));
        app.mass_minimap = false;
        assert!(matches!(
            app.inset(),
            Some(Pixels::Rgb {
                view: View::Original,
                ..
            })
        ));
        app.view = View::Dithered;
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
            view: View::Mass,
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
                assert_eq!(app.view, View::Original);
            }
        }
    }

    #[test]
    fn image_template_apply_uses_the_loaded_source() {
        for (through_library, source, margin, rejected) in [
            (true, None, 100, false),
            (true, Some((16, 16)), 100, false),
            (true, Some((1774, 1774)), 100, true),
            (false, Some((1, 1)), 400, false),
            (false, Some((16, 16)), 400, false),
            (false, Some((1024, 1024)), 400, true),
        ] {
            let name = format!("library {through_library} source {source:?}");
            let (expected_dot, expected_margin, notice) = if rejected {
                (20, 0, Some("grid exceeds four million dots"))
            } else {
                (8, margin, None)
            };
            let mut app = App::empty();
            if let Some((w, h)) = source {
                app.open_image("source.png".into(), RgbaImage::new(w, h));
            }
            let settings = Settings {
                dot: 8,
                margin,
                ..Settings::default()
            };
            let template = Template::capture("Border".into(), &settings);
            if through_library {
                app.library
                    .commit(Change::UpsertTemplate(template))
                    .unwrap();
                press(&mut app, KeyCode::Char('T'));
                press(&mut app, KeyCode::Enter);
            } else {
                app.modal = Some(Modal::templates());
                app.apply_step(Step {
                    next: Next::Close,
                    effect: Some(Effect::Preview(template.apply(&app.settings).unwrap())),
                });
            }
            assert!(
                app.settings
                    == Settings {
                        dot: expected_dot,
                        margin: expected_margin,
                        ..Settings::default()
                    },
                "{name}"
            );
            assert_eq!(app.modal.is_some(), rejected, "{name}");
            assert_eq!(app.notice(), notice, "{name}");
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
        press(&mut app, KeyCode::Char('d'));
        assert!(app.grid_cache.placement(&app.settings).is_some());
        assert!(app.notice().is_none());
    }

    #[test]
    fn mass_tones_follow_palette_changes() {
        let mut app = App::builtin(RgbaImage::new(20, 10), None);
        assert_eq!(app.mass_tones, None);
        app.view = View::Mass;
        let area = test_layout().preview;
        app.sync(area);
        let version = app.grid_cache.mass_version();
        app.settings.palette = crate::settings::Palette::custom(&["#0a141e", "#d2dce6"]);
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
        assert_eq!(app.panel.selected(&app.panel_lens()), Some(Row::Header(0)));
        assert!(!press(&mut app, KeyCode::Up));
        assert_eq!(app.panel.selected(&app.panel_lens()), Some(Row::Header(0)));
        for _ in 0..Setting::ALL.len() + BrushRow::ALL.len() + 4 {
            assert!(!press(&mut app, KeyCode::Char('j')));
        }
        assert_eq!(app.selected_setting(), None);
        for code in [KeyCode::Char('x'), KeyCode::Enter, KeyCode::Esc] {
            assert!(!press(&mut app, code));
            assert_eq!(
                app.panel.selected(&app.panel_lens()),
                Some(Row::Header(LAYER_GROUP))
            );
            assert!(!app.quit);
        }
    }

    #[test]
    fn keyboard_navigation_skips_inactive_rows_in_both_directions() {
        let mut app = app_at(Setting::Serpentine);
        app.settings.dither = Dither::Atkinson;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.panel.selected(&app.panel_lens()), Some(Row::Header(2)));
        press(&mut app, KeyCode::Up);
        assert_eq!(app.selected_setting(), Some(Setting::Serpentine));

        select(&mut app, Setting::Dither);
        app.settings.dither = Dither::Noise;
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_setting(), Some(Setting::Seed));
    }

    #[test]
    fn inactive_selected_row_stays_until_navigation() {
        let mut app = app_at(Setting::Seed);
        app.settings.dither = Dither::Atkinson;
        assert_eq!(app.selected_setting(), Some(Setting::Seed));
        press(&mut app, KeyCode::Down);
        assert_eq!(app.panel.selected(&app.panel_lens()), Some(Row::Header(2)));
    }

    #[test]
    fn preview_focus_enter_does_not_toggle_a_group_header() {
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        app.panel.select(Row::Header(1), &app.panel_lens());
        app.preview_focus = true;
        assert!(!press(&mut app, KeyCode::Enter));
        assert!(!app.panel.is_collapsed(1));
        assert_eq!(app.panel.selected(&app.panel_lens()), Some(Row::Header(1)));
    }

    #[test]
    fn panel_mouse_toggles_headers_without_an_image_and_does_not_submit_items() {
        let mut app = App::empty();
        let screen = Rect::new(0, 0, 80, 24);
        let layout = crate::ui::layout(screen);
        render_panel(&app, layout.panel);
        let geometry = app.geometry(layout.preview);
        assert!(app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            layout.panel.x + 1,
            layout.panel.y + 1,
            &geometry,
            screen,
        ));
        assert!(app.panel.is_collapsed(0));

        let mut app = App::empty();
        app.settings.dither = Dither::Noise;
        app.settings.seed = 7;
        render_panel(&app, layout.panel);
        let geometry = app.geometry(layout.preview);
        let seed_row = layout.panel.y + 15;
        for _ in 0..2 {
            app.mouse_at(
                MouseEventKind::Down(MouseButton::Left),
                layout.panel.x + 2,
                seed_row,
                &geometry,
                screen,
            );
        }
        assert_eq!(app.selected_setting(), Some(Setting::Seed));
        assert_eq!(app.settings.seed, 7);

        let mut app = App::builtin(RgbaImage::new(120, 120), None);
        app.settings = Settings::for_pixels(120, 120);
        app.refresh(false);
        app.viewport.zoom = Zoom::In(1);
        app.preview_focus = true;
        render_panel(&app, layout.panel);
        let geometry = app.geometry(layout.preview);
        let viewport = (app.viewport.x, app.viewport.y);
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            layout.panel.x + 2,
            layout.panel.y + 17,
            &geometry,
            screen,
        );
        assert_eq!(app.selected_setting(), Some(Setting::Brightness));
        assert!(!app.preview_focus);
        app.preview_focus = true;
        assert!(app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            layout.panel.x + 2,
            layout.panel.y + 17,
            &geometry,
            screen,
        ));
        assert!(!app.preview_focus);
        assert!(press(&mut app, KeyCode::Right));
        assert_eq!((app.viewport.x, app.viewport.y), viewport);
        assert_eq!(app.settings.tone.brightness, 0.05);
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
        app.view = View::Original;
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
                assert_eq!(app.view, View::Original);
            }
        }
        select(&mut app, Setting::Dither);
        app.view = View::Dithered;
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
        assert_eq!(app.view, View::Original);
        app.settings.dither = Dither::Noise;
        app.settings.seed = 0;
        assert!(!press(&mut app, KeyCode::Left));
        assert_eq!(app.settings.seed, 0);
        assert_eq!(app.view, View::Original);
        assert!(press(&mut app, KeyCode::Right));
        assert_eq!(app.settings.seed, 1);
        let seed = app.settings.seed;
        app.view = View::Original;
        assert!(press(&mut app, KeyCode::Enter));
        assert_ne!(app.settings.seed, seed);
        assert_eq!(app.view, View::Dithered);
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
        assert_eq!(app.settings.palette.name(), "Pratt");
        assert_eq!(app.settings.palette.colors().len(), 5);
        assert_eq!(app.settings.mode, Mode::Color);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.settings.palette.name(), "Black/white");
        assert_eq!(app.settings.palette.colors().len(), 2);
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
            app.settings.dither = dither;
            app.settings.seed = 0;
            app.sync(test_layout().preview);
            if !setting.is_active(&app.settings) {
                let before = app.settings.clone();
                app.view = View::Original;
                for key in [
                    KeyCode::Left,
                    KeyCode::Right,
                    KeyCode::Enter,
                    KeyCode::Char('d'),
                ] {
                    assert!(!press(&mut app, key));
                    assert!(app.settings == before);
                    assert_eq!(app.view, View::Original);
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
        }
        let mut app = app_at(Setting::Card);
        press(&mut app, KeyCode::Char('r'));
        assert!(!press(&mut app, KeyCode::Char('d')));
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
    fn flip_toggles_in_both_focuses_and_views_and_rebuilds_once() {
        for focus in [false, true] {
            for view in [View::Original, View::Dithered, View::Mass] {
                let mut app = App::builtin(RgbaImage::new(16, 12), None);
                app.settings = Settings::for_pixels(16, 12);
                app.preview_focus = focus;
                app.view = view;
                app.refresh(false);
                assert!(!app.settings.flip);
                for expected in [true, false] {
                    let before = versions(&app);
                    assert!(press(&mut app, KeyCode::Char('f')));
                    assert_eq!(app.settings.flip, expected);
                    assert_eq!(app.view, view);
                    let after = (before.0 + 1, before.1 + u64::from(view == View::Dithered));
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
        app.view = View::Dithered;
        let size = app.grid_size();
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
        for (key, expected) in [
            ('r', Rotation::R90),
            ('r', Rotation::R180),
            ('r', Rotation::R270),
            ('r', Rotation::R0),
            ('R', Rotation::R270),
            ('R', Rotation::R180),
            ('R', Rotation::R90),
            ('R', Rotation::R0),
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
        let temp = tempfile::tempdir().unwrap();
        app.request_open(temp.path().to_path_buf(), None);
        assert!(!matches!(app.modal, Some(Modal::Confirm { .. })));
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
    fn event_ending_a_moved_layer_drag_is_still_handled() {
        let mut app = App::builtin(RgbaImage::new(120, 120), None);
        app.settings = Settings::for_pixels(120, 120);
        app.refresh(false);
        let area = Rect::new(2, 3, 48, 18);
        app.viewport.zoom = Zoom::In(1);
        app.layer_drag = Some(LayerDrag {
            index: 0,
            moved: true,
            toggle: false,
        });
        let y = app.viewport.y;
        assert!(app.handle_mouse(
            mouse_event(MouseEventKind::ScrollDown, 13, 12),
            &app.geometry(area),
            test_layout().screen,
        ));
        assert!(app.layer_drag.is_none());
        assert_ne!(app.viewport.y, y);
    }

    #[test]
    fn minimap_drag_pauses_over_panel_and_resumes_outside_the_map() {
        let mut app = App::drag_fixture();
        app.viewport.hide_minimap = false;
        let screen = Rect::new(0, 0, 80, 24);
        let layout = crate::ui::layout(screen);
        let geometry = app.geometry(layout.preview);
        let minimap = geometry.minimap.unwrap();
        let map = minimap.map;
        app.mouse_at(
            MouseEventKind::Down(MouseButton::Left),
            map.x + 2,
            map.y + 1,
            &geometry,
            screen,
        );
        let centered = (app.viewport.x, app.viewport.y);
        app.mouse_at(
            MouseEventKind::Drag(MouseButton::Left),
            layout.panel.x + 2,
            layout.panel.y + 2,
            &app.geometry(layout.preview),
            screen,
        );
        assert!(app.last_drag.is_some_and(|drag| drag.minimap));
        let back = (minimap.frame.x, minimap.frame.bottom());
        assert!(layout.preview.contains(back.into()));
        assert!(!minimap.frame.contains(back.into()));
        app.mouse_at(
            MouseEventKind::Drag(MouseButton::Left),
            back.0,
            back.1,
            &app.geometry(layout.preview),
            screen,
        );
        assert_eq!((app.viewport.x, app.viewport.y), centered);
        assert!(app.last_drag.is_some_and(|drag| drag.minimap));
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
    fn print_controls_rebuild_once_and_preserve_the_view() {
        let mut app = business_app(RgbaImage::new(20, 10), None);
        select(&mut app, Setting::Dot);
        let before = versions(&app);
        assert!(press(&mut app, KeyCode::Right));
        assert_eq!(app.grid_size(), (238, 429));
        assert_eq!(versions(&app), (before.0 + 1, before.1));
        assert_eq!(app.view, View::Original);
        assert!(!app.sync(test_layout().preview));
        app.view = View::Dithered;
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
            assert_eq!(app.view, View::Dithered);
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
}
