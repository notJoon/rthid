//! Component based dialogs and palette drafts with cancel snapshots.

use std::path::PathBuf;

use crate::settings::{
    MAX_COLORS, MAX_NAME_CHARS, Palettes, Preset, Settings, Templates, build_palette, validate_name,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::Block,
};
use rthid_dither::dither::Mode;
use rthid_io::dir::Entry;
use rthid_widgets::{Confirm, Dialog, ListPicker, Outcome, TextInput};

/// Character limit of the open dialog's path field.
pub const PATH_MAX_CHARS: usize = 4096;

const OPEN_HINTS: &[(&str, &str)] = &[
    ("Up/Down", "select"),
    ("click", "select"),
    ("click again", "open"),
    ("Enter", "open"),
    ("Esc", "close"),
];

const PRESET_HINTS: &[(&str, &str)] = &[
    ("Up/Down j/k", "select"),
    ("click", "select"),
    ("click again", "apply"),
    ("Enter", "apply"),
    ("n", "new"),
    ("e", "edit"),
    ("x", "delete"),
    ("Esc", "close"),
];
const TEMPLATE_HINTS: &[(&str, &str)] = &[
    ("Up/Down j/k", "select"),
    ("click", "select"),
    ("click again", "apply"),
    ("Enter", "apply"),
    ("n", "save current settings"),
    ("x", "delete"),
    ("Esc", "close"),
];
const PROMPT_HINTS: &[(&str, &str)] = &[
    ("Enter", "save"),
    ("Esc", "back"),
    ("arrows/Home/End", "cursor"),
];
const INPUT_HINTS: &[(&str, &str)] = &[
    ("Enter", "commit"),
    ("Esc", "cancel edit"),
    ("arrows/Home/End", "cursor"),
];
const EDITOR_HINTS: &[(&str, &str)] = &[
    ("Up/Down j/k", "row"),
    ("click", "select"),
    ("click again", "edit"),
    ("Enter/e", "edit"),
    ("Left/Right h/l", "mode"),
    ("a", "add"),
    ("x", "remove"),
    ("s", "save"),
    ("Esc", "cancel"),
];

/// Read access to the state needed for a modal decision.
pub struct Ctx<'a> {
    /// Available presets and their protection status.
    pub palettes: &'a Palettes,
    /// Saved settings available to apply or overwrite.
    pub templates: &'a Templates,
    /// Current preview settings.
    pub settings: &'a Settings,
    /// Preview selection to restore when editing is cancelled.
    pub show_original: bool,
}

/// A modal transition and its optional application change.
pub struct Step {
    /// Transition to follow after the effect succeeds.
    pub next: Next,
    /// Change for App to validate and apply.
    pub effect: Option<Effect>,
}

impl Step {
    fn stay() -> Self {
        Self {
            next: Next::Stay,
            effect: None,
        }
    }
    fn close() -> Self {
        Self {
            next: Next::Close,
            effect: None,
        }
    }
    fn effect(next: Next, effect: Effect) -> Self {
        Self {
            next,
            effect: Some(effect),
        }
    }
    fn notice(message: String) -> Self {
        Self::effect(Next::Stay, Effect::Notice(message))
    }
}

/// The modal remains open if applying its effect fails.
pub enum Next {
    /// Keep the current dialog.
    Stay,
    /// Dismiss the current dialog.
    Close,
    /// Open a different dialog.
    Replace(Modal),
}

/// Changes applied by App after a modal handles input.
pub enum Effect {
    /// Request a source image relative to the supplied folder without closing the dialog.
    Open(PathBuf, PathBuf),
    /// Request the entries of a directory.
    ListDir(PathBuf),
    /// Report a validation error without leaving the dialog.
    Notice(String),
    /// Show settings in the dithered preview.
    Preview(Settings),
    /// Recover the preview captured before editing.
    Restore {
        /// Captured processing settings.
        settings: Settings,
        /// Captured preview selection.
        show_original: bool,
    },
    /// Persist and select the draft using the captured base settings.
    SavePreset {
        /// Draft to validate and store.
        preset: Preset,
        /// Settings to retain outside the palette.
        base: Settings,
    },
    /// Remove a user preset and request persistence.
    RemovePreset(String),
    /// Capture current settings under this name and request persistence.
    SaveTemplate(String),
    /// Remove a saved snapshot and request persistence.
    RemoveTemplate(String),
}

/// Action held while a confirmation captures keys.
pub enum OnYes {
    /// Remove the selected user preset.
    RemovePreset(String),
    /// Remove the selected snapshot.
    RemoveTemplate(String),
    /// Save current settings under an existing name.
    OverwriteTemplate(String),
}

impl OnYes {
    // Names the action instead of a generic yes, as the help line did before.
    fn hints(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::RemovePreset(_) | Self::RemoveTemplate(_) => {
                &[("y", "delete"), ("any other key", "cancel")]
            }
            Self::OverwriteTemplate(_) => {
                &[("y", "overwrite"), ("any other key", "return to name")]
            }
        }
    }
}

/// Dialogs that capture all input except quitting with Ctrl+C.
pub enum Modal {
    /// Directory listing and editable image path.
    OpenFile(Box<FilePicker>),
    /// Palette selection and optional removal confirmation.
    Presets {
        /// Selection and retained scroll position.
        list: ListPicker,
        /// Pending question and accepted action.
        confirm: Option<(Confirm, OnYes)>,
    },
    /// Saved settings with a name prompt and optional confirmation.
    Templates {
        /// Selection and retained scroll position.
        list: ListPicker,
        /// Name retained through overwrite confirmation.
        prompt: Option<TextInput>,
        /// Pending question and accepted action.
        confirm: Option<(Confirm, OnYes)>,
    },
    /// Palette draft with an independent cancel snapshot.
    Editor(Box<Editor>),
}

/// Dialog regions shared by drawing and mouse input.
pub struct ModalLayout {
    /// Border and title rectangle.
    pub dialog: Rect,
    /// Active text field, when present.
    pub field: Option<Rect>,
    /// List content without a border, when present.
    pub list: Option<Rect>,
}

/// Path field and directory selection for opening an image.
pub struct FilePicker {
    /// Editable path submitted after a text edit.
    pub input: TextInput,
    /// Selected entry and retained scroll position.
    pub list: ListPicker,
    /// Directory containing the listed entries.
    pub dir: PathBuf,
    /// Visible directories and supported image files.
    pub entries: Vec<Entry>,
    /// Whether Enter submits the field instead of the selected entry.
    pub typed: bool,
    /// Number the app gave this dialog when it opened, matched against IO results.
    pub generation: u64,
}

impl FilePicker {
    /// Replaces the directory and resets the field, selection and scroll.
    pub fn set_listing(&mut self, dir: PathBuf, entries: Vec<Entry>) {
        self.input = TextInput::new(dir.join("").to_string_lossy(), PATH_MAX_CHARS);
        self.list = ListPicker::new(usize::from(entries.len() > 1 && entries[0].name == ".."));
        self.dir = dir;
        self.entries = entries;
        self.typed = false;
    }

    fn handle_key(&mut self, key: KeyEvent) -> Step {
        if matches!(key.code, KeyCode::Up | KeyCode::Down) {
            self.list.handle_key(key, self.entries.len());
            self.typed = false;
            return Step::stay();
        }
        match self.input.handle_key(key) {
            Outcome::Consumed => self.typed = true,
            Outcome::Cancel => return Step::close(),
            Outcome::Submit(()) if self.typed => {
                return Step::effect(
                    Next::Stay,
                    Effect::Open(PathBuf::from(self.input.text()), self.dir.clone()),
                );
            }
            Outcome::Submit(()) => {
                if let Some(entry) = self
                    .list
                    .selected(self.entries.len())
                    .and_then(|i| self.entries.get(i))
                {
                    let path = self.dir.join(&entry.file_name);
                    return Step::effect(
                        Next::Stay,
                        if entry.dir {
                            Effect::ListDir(path)
                        } else {
                            Effect::Open(PathBuf::from(&entry.file_name), self.dir.clone())
                        },
                    );
                }
            }
            Outcome::Ignored => {}
        }
        Step::stay()
    }
}

/// Editable palette and the settings it temporarily replaces.
pub struct Editor {
    /// Palette applied to the live preview.
    pub draft: Preset,
    /// Name, colors, then optional mode.
    pub rows: ListPicker,
    /// Uncommitted name or hex text.
    pub input: Option<TextInput>,
    original: Settings,
    show_original: bool,
}

impl Editor {
    /// Captures the preview before showing an editable draft.
    pub(crate) fn new(ctx: &Ctx<'_>, draft: Preset) -> Self {
        let rows = ListPicker::new(0);
        rows.reset_scroll();
        Self {
            draft,
            rows,
            input: None,
            original: ctx.settings.clone(),
            show_original: ctx.show_original,
        }
    }

    /// Whether the active hex edit can be committed.
    pub fn invalid_hex(&self) -> bool {
        self.rows
            .selected(self.draft.colors.len() + 2)
            .is_some_and(|row| row > 0 && row <= self.draft.colors.len())
            && self
                .input
                .as_ref()
                .is_some_and(|input| build_palette(&[input.text()]).is_err())
    }

    /// Color for a draft row, including any uncommitted hex edit.
    pub fn swatch_rgb(&self, row: usize) -> Option<[u8; 3]> {
        let color = self.draft.colors.get(row.checked_sub(1)?)?;
        let text = if Some(row) == self.rows.selected(self.draft.colors.len() + 2) {
            self.input.as_ref().map_or(color.as_str(), TextInput::text)
        } else {
            color
        };
        build_palette(&[text]).ok().map(|palette| palette[0].rgb)
    }

    // Uses committed values only. The field's own swatch shows the typed color, so partial
    // values such as #123 on the way to #123456 never start a dither.
    fn preview(&self) -> Settings {
        self.original.with_preset(&self.draft)
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &Ctx<'_>) -> Step {
        let row = self.rows.selected(self.draft.colors.len() + 2).unwrap();
        if let Some(input) = &mut self.input {
            match input.handle_key(key) {
                Outcome::Cancel => {
                    self.input = None;
                    return Step::effect(Next::Stay, Effect::Preview(self.preview()));
                }
                Outcome::Submit(()) => {
                    if row == 0 {
                        if let Err(error) = ctx.palettes.check_name(input.text()) {
                            return Step::notice(error);
                        }
                        self.draft.name = input.text().into();
                    } else if build_palette(&[input.text()]).is_ok() {
                        self.draft.colors[row - 1] = input.text().into();
                    } else {
                        return Step::stay();
                    }
                    self.input = None;
                    return Step::effect(Next::Stay, Effect::Preview(self.preview()));
                }
                Outcome::Consumed => return Step::stay(),
                // An open field keeps every key, so row moves and commands cannot drop or misplace the edit.
                Outcome::Ignored => return Step::stay(),
            }
        }
        let mode_row = self.draft.colors.len() + 1;
        match self.rows.handle_key(key, mode_row + 1) {
            Outcome::Cancel => {
                return Step::effect(
                    Next::Close,
                    Effect::Restore {
                        settings: self.original.clone(),
                        show_original: self.show_original,
                    },
                );
            }
            Outcome::Submit(_) if row < mode_row => self.open_input(),
            Outcome::Consumed => self.input = None,
            Outcome::Ignored => match key.code {
                KeyCode::Char('e') if row < mode_row => self.open_input(),
                KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l') if row == mode_row => {
                    self.draft.mode = match (
                        self.draft.mode,
                        matches!(key.code, KeyCode::Left | KeyCode::Char('h')),
                    ) {
                        (None, false) | (Some(Mode::Color), true) => Some(Mode::Tone),
                        (Some(Mode::Tone), false) | (None, true) => Some(Mode::Color),
                        _ => None,
                    };
                }
                KeyCode::Char('a') if self.draft.colors.len() < MAX_COLORS => {
                    let index = row.min(self.draft.colors.len());
                    self.draft.colors.insert(index, "#888888".into());
                    self.rows = ListPicker::new(index + 1);
                }
                KeyCode::Char('x') if row > 0 && row < mode_row && self.draft.colors.len() > 1 => {
                    self.draft.colors.remove(row - 1);
                    self.rows = ListPicker::new(row.min(self.draft.colors.len()));
                }
                KeyCode::Char('s') => {
                    return Step::effect(
                        Next::Close,
                        Effect::SavePreset {
                            preset: self.draft.clone(),
                            base: self.original.clone(),
                        },
                    );
                }
                _ => {}
            },
            _ => {}
        }
        Step::effect(Next::Stay, Effect::Preview(self.preview()))
    }

    fn open_input(&mut self) {
        let row = self.rows.selected(self.draft.colors.len() + 2).unwrap();
        let text = if row == 0 {
            &self.draft.name
        } else {
            &self.draft.colors[row - 1]
        };
        self.input = Some(TextInput::new(
            text.clone(),
            if row == 0 { MAX_NAME_CHARS } else { 7 },
        ));
    }
}

/// Preset labels shared by dialog measurement and drawing.
pub(crate) fn preset_rows(ctx: &Ctx<'_>) -> Vec<Line<'static>> {
    ctx.palettes
        .iter()
        .map(|preset| {
            Line::raw(format!(
                "{}{}{}",
                if ctx.settings.uses(preset) {
                    "* "
                } else {
                    "  "
                },
                preset.name,
                if preset.builtin { " (built-in)" } else { "" }
            ))
        })
        .collect()
}

/// Template labels or the empty hint shared by dialog measurement and drawing.
pub(crate) fn template_rows(ctx: &Ctx<'_>) -> Vec<Line<'static>> {
    if ctx.templates.is_empty() {
        vec![Line::raw("n: save current settings")]
    } else {
        ctx.templates
            .iter()
            .map(|template| {
                Line::raw(format!(
                    "{} | {} | {}",
                    template.name,
                    template.palette_name,
                    crate::settings::mode_label(template.mode)
                ))
            })
            .collect()
    }
}

impl Modal {
    /// Computes dialog and content rectangles without changing selection or scroll.
    pub fn layout(&self, preview: Rect, screen: Rect, ctx: &Ctx<'_>) -> ModalLayout {
        let (title, width, height) = match self {
            Self::OpenFile(picker) => (
                "Open image",
                picker
                    .entries
                    .iter()
                    .map(|entry| Line::raw(&entry.name).width() + usize::from(entry.dir))
                    .chain([Line::raw(picker.input.text()).width()])
                    .max()
                    .unwrap_or(0),
                picker.entries.len().saturating_add(1),
            ),
            Self::Presets { confirm, .. } => (
                confirm
                    .as_ref()
                    .map_or("Presets", |(question, _)| question.message()),
                preset_rows(ctx).iter().map(Line::width).max().unwrap_or(0),
                ctx.palettes.len(),
            ),
            Self::Templates {
                prompt, confirm, ..
            } => (
                confirm.as_ref().map_or(
                    if prompt.is_some() {
                        "Save template"
                    } else {
                        "Templates"
                    },
                    |(question, _)| question.message(),
                ),
                prompt.as_ref().map_or_else(
                    || {
                        template_rows(ctx)
                            .iter()
                            .map(Line::width)
                            .max()
                            .unwrap_or(0)
                    },
                    |input| Line::raw(format!("Name: {}", input.text())).width(),
                ),
                if prompt.is_some() {
                    1
                } else {
                    ctx.templates.len().max(1)
                },
            ),
            Self::Editor(editor) => {
                let width = (0..=editor.draft.colors.len())
                    .map(|row| {
                        let prefix = if row == 0 {
                            "Name: ".into()
                        } else {
                            format!("{row}: ")
                        };
                        let text = if let Some(input) = &editor.input
                            && editor.rows.selected(editor.draft.colors.len() + 2) == Some(row)
                        {
                            input.text()
                        } else if row == 0 {
                            &editor.draft.name
                        } else {
                            &editor.draft.colors[row - 1]
                        };
                        Line::raw(format!("{prefix}{text}")).width()
                            + if editor.swatch_rgb(row).is_some() {
                                4
                            } else {
                                0
                            }
                    })
                    .chain([Line::raw(format!(
                        "Mode: {}",
                        editor
                            .draft
                            .mode
                            .map_or("keep", crate::settings::mode_label)
                    ))
                    .width()])
                    .max()
                    .unwrap_or(0);
                ("Preset editor", width, editor.draft.colors.len() + 2)
            }
        };
        let dialog = Dialog {
            title,
            min_width: 28,
        }
        .area(
            preview,
            screen,
            (
                width.min(u16::MAX as usize) as u16,
                height.min(u16::MAX as usize) as u16,
            ),
        );
        let inner = Block::bordered().inner(dialog);
        let (field, list) = match self {
            Self::OpenFile(_) => {
                let [field, list] =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
                (Some(field), Some(list))
            }
            Self::Templates {
                prompt: Some(_), ..
            } => (
                (inner.width > 2 && inner.height > 0)
                    .then(|| Rect::new(inner.x + 2, inner.y, inner.width - 2, 1)),
                None,
            ),
            Self::Editor(editor) => {
                let field = editor.input.as_ref().and_then(|_| {
                    let selected = editor.rows.selected(editor.draft.colors.len() + 2)?;
                    let offset = editor.rows.offset().min(selected).max(
                        selected
                            .saturating_add(1)
                            .saturating_sub(usize::from(inner.height)),
                    );
                    let row = selected.checked_sub(offset)?;
                    (row < usize::from(inner.height) && inner.width > 2)
                        .then(|| Rect::new(inner.x + 2, inner.y + row as u16, inner.width - 2, 1))
                });
                (field, Some(inner))
            }
            _ => (None, Some(inner)),
        };
        ModalLayout {
            dialog,
            field,
            list,
        }
    }

    /// Routes list mouse outcomes through the same actions as keyboard input.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, layout: &ModalLayout, ctx: Ctx<'_>) -> Step {
        let Some(area) = layout.list else {
            return Step::stay();
        };
        let outcome = match self {
            Self::OpenFile(picker) => {
                let outcome = picker.list.handle_mouse(mouse, area, picker.entries.len());
                if matches!(outcome, Outcome::Consumed | Outcome::Submit(_)) {
                    picker.typed = false;
                }
                outcome
            }
            Self::Presets {
                list,
                confirm: None,
            } => list.handle_mouse(mouse, area, ctx.palettes.len()),
            Self::Templates {
                list,
                prompt: None,
                confirm: None,
            } => list.handle_mouse(mouse, area, ctx.templates.len()),
            Self::Editor(editor) if editor.input.is_none() => {
                editor
                    .rows
                    .handle_mouse(mouse, area, editor.draft.colors.len() + 2)
            }
            _ => return Step::stay(),
        };
        match outcome {
            Outcome::Submit(_) => {
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), ctx)
            }
            Outcome::Consumed => {
                if let Self::Editor(editor) = self {
                    Step::effect(Next::Stay, Effect::Preview(editor.preview()))
                } else {
                    Step::stay()
                }
            }
            _ => Step::stay(),
        }
    }

    /// Opens a path field while the event loop loads the starting directory.
    /// `generation` tells this dialog apart from earlier ones for late IO results.
    pub fn open_file(start: PathBuf, generation: u64) -> Self {
        Self::OpenFile(Box::new(FilePicker {
            input: TextInput::new(start.join("").to_string_lossy(), PATH_MAX_CHARS),
            list: ListPicker::new(0),
            dir: start,
            entries: Vec::new(),
            typed: false,
            generation,
        }))
    }

    /// Active text field, excluding prompts behind a confirmation.
    pub fn text_input_mut(&mut self) -> Option<&mut TextInput> {
        match self {
            Self::OpenFile(picker) => Some(&mut picker.input),
            Self::Templates {
                prompt,
                confirm: None,
                ..
            } => prompt.as_mut(),
            Self::Editor(editor) => editor.input.as_mut(),
            _ => None,
        }
    }

    /// Opens on the current preset, or the first preset for detached colors.
    pub fn presets(settings: &Settings, palettes: &Palettes) -> Self {
        let list = ListPicker::new(palettes.find(&settings.palette_name).unwrap_or(0));
        list.reset_scroll();
        Self::Presets {
            list,
            confirm: None,
        }
    }

    /// Opens the saved settings list at the first entry.
    pub fn templates() -> Self {
        let list = ListPicker::new(0);
        list.reset_scroll();
        Self::Templates {
            list,
            prompt: None,
            confirm: None,
        }
    }

    /// Key descriptions for the component currently receiving input.
    pub fn hints(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::OpenFile(_) => OPEN_HINTS,
            Self::Presets {
                confirm: Some((_, on_yes)),
                ..
            }
            | Self::Templates {
                confirm: Some((_, on_yes)),
                ..
            } => on_yes.hints(),
            Self::Presets { .. } => PRESET_HINTS,
            Self::Templates {
                prompt: Some(_), ..
            } => PROMPT_HINTS,
            Self::Templates { .. } => TEMPLATE_HINTS,
            Self::Editor(editor) if editor.input.is_some() => INPUT_HINTS,
            Self::Editor(_) => EDITOR_HINTS,
        }
    }

    // TODO: Route keys to an open text field here for every dialog once more fields exist.
    // The editor and the template prompt each implement that capture today.
    /// Handles component outcomes without mutating application state.
    pub fn handle_key(&mut self, key: KeyEvent, ctx: Ctx<'_>) -> Step {
        if key.kind == KeyEventKind::Release {
            return Step::stay();
        }
        let confirm = match self {
            Self::OpenFile(picker) => return picker.handle_key(key),
            Self::Presets { confirm, .. } | Self::Templates { confirm, .. } => confirm,
            Self::Editor(editor) => return editor.handle_key(key, &ctx),
        };
        if let Some((question, _)) = confirm {
            match question.handle_key(key) {
                Outcome::Submit(()) => {
                    let (_, action) = confirm.take().unwrap();
                    return match action {
                        OnYes::RemovePreset(i) => Step::effect(Next::Stay, Effect::RemovePreset(i)),
                        OnYes::RemoveTemplate(i) => {
                            Step::effect(Next::Stay, Effect::RemoveTemplate(i))
                        }
                        OnYes::OverwriteTemplate(name) => {
                            Step::effect(Next::Close, Effect::SaveTemplate(name))
                        }
                    };
                }
                Outcome::Cancel => *confirm = None,
                _ => {}
            }
            return Step::stay();
        }
        match self {
            Self::Presets { list, confirm } => match list.handle_key(key, ctx.palettes.len()) {
                Outcome::Cancel => {
                    return Step::close();
                }
                Outcome::Submit(i) => {
                    return Step::effect(
                        Next::Close,
                        Effect::Preview(ctx.settings.with_preset(ctx.palettes.get(i).unwrap())),
                    );
                }
                Outcome::Ignored => match key.code {
                    KeyCode::Char('n' | 'e') => {
                        let draft = if key.code == KeyCode::Char('n') {
                            Preset {
                                name: String::new(),
                                colors: ctx.settings.palette_hex.clone(),
                                mode: None,
                                builtin: false,
                            }
                        } else {
                            let Some(preset) = list
                                .selected(ctx.palettes.len())
                                .and_then(|i| ctx.palettes.get(i))
                            else {
                                return Step::stay();
                            };
                            let mut preset = preset.clone();
                            if preset.builtin {
                                preset.name.push_str(" copy");
                            }
                            preset.builtin = false;
                            preset
                        };
                        let editor = Editor::new(&ctx, draft);
                        let preview = editor.preview();
                        return Step::effect(
                            Next::Replace(Self::Editor(Box::new(editor))),
                            Effect::Preview(preview),
                        );
                    }
                    KeyCode::Char('x') => {
                        let Some(preset) = list
                            .selected(ctx.palettes.len())
                            .and_then(|i| ctx.palettes.get(i))
                        else {
                            return Step::stay();
                        };
                        if preset.builtin {
                            return Step::notice("built-in presets cannot be removed".into());
                        }
                        *confirm = Some((
                            Confirm::new("Delete preset?"),
                            OnYes::RemovePreset(preset.name.clone()),
                        ));
                    }
                    _ => {}
                },
                _ => {}
            },
            Self::Templates {
                list,
                prompt,
                confirm,
            } => {
                if let Some(input) = prompt {
                    match input.handle_key(key) {
                        Outcome::Cancel => *prompt = None,
                        Outcome::Submit(()) => {
                            if let Err(error) = validate_name(input.text(), "template") {
                                return Step::notice(error);
                            }
                            if ctx.templates.find(input.text()).is_some() {
                                *confirm = Some((
                                    Confirm::new("Overwrite template?"),
                                    OnYes::OverwriteTemplate(input.text().into()),
                                ));
                            } else {
                                return Step::effect(
                                    Next::Close,
                                    Effect::SaveTemplate(input.text().into()),
                                );
                            }
                        }
                        _ => {}
                    }
                } else {
                    match list.handle_key(key, ctx.templates.len()) {
                        Outcome::Cancel => {
                            return Step::close();
                        }
                        Outcome::Submit(i) => {
                            return match ctx.templates.get(i).unwrap().apply(ctx.settings) {
                                Ok(settings) => {
                                    Step::effect(Next::Close, Effect::Preview(settings))
                                }
                                Err(error) => Step::notice(error),
                            };
                        }
                        Outcome::Ignored => match key.code {
                            KeyCode::Char('n') => {
                                *prompt = Some(TextInput::new("", MAX_NAME_CHARS))
                            }
                            KeyCode::Char('x') => {
                                let Some(template) = list
                                    .selected(ctx.templates.len())
                                    .and_then(|i| ctx.templates.get(i))
                                else {
                                    return Step::stay();
                                };
                                *confirm = Some((
                                    Confirm::new("Delete template?"),
                                    OnYes::RemoveTemplate(template.name.clone()),
                                ));
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
            }
            Self::Editor(_) | Self::OpenFile(_) => unreachable!(),
        }
        Step::stay()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Template;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn mouse(kind: ratatui::crossterm::event::MouseEventKind, area: Rect, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: area.x,
            row: area.y + row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn open_mouse_selects_submits_and_scrolls() {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let palettes = Palettes::builtin();
        let templates = Templates::default();
        let settings = Settings::default();
        let context = || ctx(&palettes, &templates, &settings);
        let mut modal = Modal::open_file("/images".into(), 1);
        let Modal::OpenFile(picker) = &mut modal else {
            unreachable!()
        };
        picker.set_listing(
            "/images".into(),
            vec![
                Entry {
                    file_name: "album".into(),
                    name: "album".into(),
                    dir: true,
                },
                Entry {
                    file_name: "photo.png".into(),
                    name: "photo.png".into(),
                    dir: false,
                },
            ],
        );
        picker.typed = true;
        let layout = modal.layout(
            Rect::new(0, 0, 80, 24),
            Rect::new(0, 0, 100, 30),
            &context(),
        );
        let area = layout.list.unwrap();
        let click = |row| mouse(MouseEventKind::Down(MouseButton::Left), area, row);
        assert!(
            modal
                .handle_mouse(click(1), &layout, context())
                .effect
                .is_none()
        );
        assert!(
            matches!(&modal, Modal::OpenFile(p) if p.list.selected(p.entries.len()) == Some(1) && !p.typed)
        );
        let step = modal.handle_mouse(click(1), &layout, context());
        assert!(
            matches!(step.effect, Some(Effect::Open(path, dir)) if path == std::path::Path::new("photo.png") && dir == std::path::Path::new("/images"))
        );
        modal.handle_mouse(mouse(MouseEventKind::ScrollUp, area, 0), &layout, context());
        assert!(
            matches!(&modal, Modal::OpenFile(p) if p.list.selected(p.entries.len()) == Some(0))
        );
        assert!(
            matches!(modal.handle_mouse(click(0), &layout, context()).effect, Some(Effect::ListDir(path)) if path == std::path::Path::new("/images/album"))
        );
        modal.handle_mouse(
            mouse(MouseEventKind::ScrollDown, area, 0),
            &layout,
            context(),
        );
        assert!(
            matches!(&modal, Modal::OpenFile(p) if p.list.selected(p.entries.len()) == Some(1))
        );
        let outside = mouse(MouseEventKind::Down(MouseButton::Left), layout.dialog, 0);
        assert!(
            modal
                .handle_mouse(outside, &layout, context())
                .effect
                .is_none()
        );
    }

    #[test]
    fn preset_mouse_click_again_applies_selection() {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let palettes = Palettes::builtin();
        let templates = Templates::default();
        let settings = Settings::default();
        let context = || ctx(&palettes, &templates, &settings);
        let mut modal = Modal::presets(&settings, &palettes);
        let layout = modal.layout(
            Rect::new(0, 0, 80, 24),
            Rect::new(0, 0, 100, 30),
            &context(),
        );
        let click = mouse(
            MouseEventKind::Down(MouseButton::Left),
            layout.list.unwrap(),
            1,
        );
        assert!(
            modal
                .handle_mouse(click, &layout, context())
                .effect
                .is_none()
        );
        let step = modal.handle_mouse(click, &layout, context());
        assert!(matches!(step.next, Next::Close));
        assert!(
            matches!(step.effect, Some(Effect::Preview(s)) if s == settings.with_preset(palettes.get(1).unwrap()))
        );
        let Modal::Presets { confirm, .. } = &mut modal else {
            unreachable!()
        };
        *confirm = Some((
            Confirm::new("Delete preset?"),
            OnYes::RemovePreset("Paper".into()),
        ));
        let step = modal.handle_mouse(click, &layout, context());
        assert!(step.effect.is_none());
        assert!(matches!(step.next, Next::Stay));
        assert!(matches!(
            modal,
            Modal::Presets {
                confirm: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn open_file_routes_selection_and_text() {
        let palettes = Palettes::builtin();
        let templates = Templates::default();
        let settings = Settings::default();
        let mut modal = Modal::open_file(PathBuf::from("/images"), 1);
        let Modal::OpenFile(picker) = &mut modal else {
            unreachable!()
        };
        picker.set_listing(
            PathBuf::from("/images"),
            vec![
                Entry {
                    file_name: "..".into(),
                    name: "..".into(),
                    dir: true,
                },
                Entry {
                    file_name: "album".into(),
                    name: "album".into(),
                    dir: true,
                },
                Entry {
                    file_name: "photo.png".into(),
                    name: "photo.png".into(),
                    dir: false,
                },
            ],
        );
        assert_eq!(picker.list.selected(picker.entries.len()), Some(1));
        assert_eq!(picker.input.text(), "/images/");
        let context = || ctx(&palettes, &templates, &settings);
        let step = modal.handle_key(key(KeyCode::Enter), context());
        assert!(matches!(step.next, Next::Stay));
        assert!(
            matches!(step.effect, Some(Effect::ListDir(path)) if path == std::path::Path::new("/images/album"))
        );
        modal.handle_key(key(KeyCode::Down), context());
        let step = modal.handle_key(key(KeyCode::Enter), context());
        assert!(
            matches!(step.effect, Some(Effect::Open(path, dir)) if path == std::path::Path::new("photo.png") && dir == std::path::Path::new("/images"))
        );
        modal.handle_key(key(KeyCode::Up), context());
        modal.handle_key(key(KeyCode::Char('j')), context());
        let Modal::OpenFile(picker) = &modal else {
            unreachable!()
        };
        assert_eq!(picker.list.selected(picker.entries.len()), Some(1));
        assert_eq!(picker.input.text(), "/images/j");
        let Modal::OpenFile(picker) = &mut modal else {
            unreachable!()
        };
        picker.input = TextInput::new("~/raw path.png", PATH_MAX_CHARS);
        let step = modal.handle_key(key(KeyCode::Enter), context());
        assert!(matches!(step.next, Next::Stay));
        assert!(
            matches!(step.effect, Some(Effect::Open(path, dir)) if path == std::path::Path::new("~/raw path.png") && dir == std::path::Path::new("/images"))
        );
        modal.handle_key(key(KeyCode::Down), context());
        let step = modal.handle_key(key(KeyCode::Enter), context());
        assert!(
            matches!(step.effect, Some(Effect::Open(path, dir)) if path == std::path::Path::new("photo.png") && dir == std::path::Path::new("/images"))
        );
        let Modal::OpenFile(picker) = &mut modal else {
            unreachable!()
        };
        picker.set_listing(
            PathBuf::from("/empty"),
            vec![Entry {
                file_name: "..".into(),
                name: "..".into(),
                dir: true,
            }],
        );
        assert_eq!(picker.list.selected(picker.entries.len()), Some(0));
        picker.set_listing(PathBuf::from("/empty"), vec![]);
        assert_eq!(picker.list.selected(picker.entries.len()), None);
        assert_eq!(picker.list.offset(), 0);
        assert!(!picker.typed);
        let step = modal.handle_key(key(KeyCode::Enter), context());
        assert!(matches!(step.next, Next::Stay));
        assert!(step.effect.is_none());
        assert!(matches!(
            modal.handle_key(key(KeyCode::Esc), context()).next,
            Next::Close
        ));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            let file_name = std::ffi::OsString::from_vec(b"photo-\xff.png".to_vec());
            let mut modal = Modal::open_file("/images".into(), 1);
            let Modal::OpenFile(picker) = &mut modal else {
                unreachable!()
            };
            picker.set_listing(
                "/images".into(),
                vec![Entry {
                    name: file_name.to_string_lossy().into_owned(),
                    file_name: file_name.clone(),
                    dir: false,
                }],
            );
            let step = modal.handle_key(key(KeyCode::Enter), context());
            assert!(
                matches!(step.effect, Some(Effect::Open(path, _)) if path.as_os_str() == file_name)
            );
        }
    }

    fn ctx<'a>(
        palettes: &'a Palettes,
        templates: &'a Templates,
        settings: &'a Settings,
    ) -> Ctx<'a> {
        Ctx {
            palettes,
            templates,
            settings,
            show_original: true,
        }
    }

    fn press(
        modal: &mut Modal,
        code: KeyCode,
        palettes: &Palettes,
        templates: &Templates,
        settings: &Settings,
    ) -> Step {
        modal.handle_key(key(code), ctx(palettes, templates, settings))
    }

    #[test]
    fn editor_rejects_invalid_input_previews_colors_and_restores_snapshot() {
        let palettes = Palettes::builtin();
        let templates = Templates::default();
        let mut original = Settings::default();
        original.options.tone.brightness = 0.2;
        let mut modal = Modal::presets(&original, &palettes);
        let step = modal.handle_key(
            key(KeyCode::Char('n')),
            ctx(&palettes, &templates, &original),
        );
        let Next::Replace(mut modal) = step.next else {
            panic!("expected editor")
        };
        press(&mut modal, KeyCode::Enter, &palettes, &templates, &original);
        assert!(matches!(
            press(&mut modal, KeyCode::Enter, &palettes, &templates, &original).effect,
            Some(Effect::Notice(_))
        ));
        assert!(matches!(&modal, Modal::Editor(e) if e.input.is_some()));
        for c in "Paper".chars() {
            press(
                &mut modal,
                KeyCode::Char(c),
                &palettes,
                &templates,
                &original,
            );
        }
        press(&mut modal, KeyCode::Enter, &palettes, &templates, &original);
        press(&mut modal, KeyCode::Down, &palettes, &templates, &original);
        press(
            &mut modal,
            KeyCode::Char('e'),
            &palettes,
            &templates,
            &original,
        );
        for _ in 0..4 {
            press(
                &mut modal,
                KeyCode::Backspace,
                &palettes,
                &templates,
                &original,
            );
        }
        for c in "oops".chars() {
            press(
                &mut modal,
                KeyCode::Char(c),
                &palettes,
                &templates,
                &original,
            );
        }
        press(&mut modal, KeyCode::Enter, &palettes, &templates, &original);
        assert!(
            matches!(&modal, Modal::Editor(e) if e.invalid_hex() && e.input.is_some() && e.draft.colors[0] == "#000")
        );
        for _ in 0..4 {
            press(
                &mut modal,
                KeyCode::Backspace,
                &palettes,
                &templates,
                &original,
            );
        }
        // Typing previews nothing until the value is committed.
        for c in "#123456".chars() {
            let step = press(
                &mut modal,
                KeyCode::Char(c),
                &palettes,
                &templates,
                &original,
            );
            assert!(step.effect.is_none());
        }
        // Row moves and commands must not drop or misplace an open edit.
        press(&mut modal, KeyCode::Down, &palettes, &templates, &original);
        modal.handle_key(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
            ctx(&palettes, &templates, &original),
        );
        assert!(
            matches!(&modal, Modal::Editor(e) if e.rows.selected(e.draft.colors.len() + 2) == Some(1) && e.draft.colors.len() == 2 && e.input.as_ref().is_some_and(|i| i.text() == "#123456"))
        );
        let step = press(&mut modal, KeyCode::Enter, &palettes, &templates, &original);
        assert!(matches!(step.effect, Some(Effect::Preview(s)) if s.palette_hex[0] == "#123456"));
        assert!(
            matches!(&modal, Modal::Editor(e) if e.input.is_none() && e.draft.colors[0] == "#123456")
        );
        press(&mut modal, KeyCode::Enter, &palettes, &templates, &original);
        press(&mut modal, KeyCode::Esc, &palettes, &templates, &original);
        assert!(matches!(&modal, Modal::Editor(e) if e.input.is_none()));
        let step = press(&mut modal, KeyCode::Esc, &palettes, &templates, &original);
        assert!(matches!(step.next, Next::Close));
        assert!(
            matches!(step.effect, Some(Effect::Restore { settings, show_original: true }) if settings == original)
        );
    }

    #[test]
    fn editor_keeps_color_bounds_and_cycles_mode() {
        let palettes = Palettes::builtin();
        let templates = Templates::default();
        let settings = Settings::default();
        let mut editor = Editor::new(
            &ctx(&palettes, &templates, &settings),
            palettes.get(0).unwrap().clone(),
        );
        for _ in 0..20 {
            editor.handle_key(
                key(KeyCode::Char('a')),
                &ctx(&palettes, &templates, &settings),
            );
        }
        assert_eq!(editor.draft.colors.len(), MAX_COLORS);
        for _ in 0..20 {
            editor.handle_key(
                key(KeyCode::Char('x')),
                &ctx(&palettes, &templates, &settings),
            );
        }
        assert_eq!(editor.draft.colors, ["#888888"]);
        editor.handle_key(key(KeyCode::Down), &ctx(&palettes, &templates, &settings));
        for expected in [Some(Mode::Tone), Some(Mode::Color), None] {
            editor.handle_key(
                key(KeyCode::Char('l')),
                &ctx(&palettes, &templates, &settings),
            );
            assert_eq!(editor.draft.mode, expected);
        }
        editor.handle_key(
            key(KeyCode::Char('h')),
            &ctx(&palettes, &templates, &settings),
        );
        assert_eq!(editor.draft.mode, Some(Mode::Color));
    }

    #[test]
    fn preset_confirmation_protects_builtins_and_retains_selection() {
        let mut palettes = Palettes::builtin();
        let templates = Templates::default();
        let settings = Settings::default();
        let index = palettes
            .upsert(Preset {
                name: "Paper".into(),
                colors: vec!["#123".into()],
                mode: None,
                builtin: false,
            })
            .unwrap();
        let mut modal = Modal::presets(&settings, &palettes);
        assert!(
            matches!(press(&mut modal, KeyCode::Char('x'), &palettes, &templates, &settings).effect, Some(Effect::Notice(message)) if message.contains("built-in"))
        );
        for _ in 0..index {
            press(&mut modal, KeyCode::Down, &palettes, &templates, &settings);
        }
        press(
            &mut modal,
            KeyCode::Char('x'),
            &palettes,
            &templates,
            &settings,
        );
        let mut release = key(KeyCode::Char('y'));
        release.kind = KeyEventKind::Release;
        assert!(
            modal
                .handle_key(release, ctx(&palettes, &templates, &settings))
                .effect
                .is_none()
        );
        press(
            &mut modal,
            KeyCode::Char('k'),
            &palettes,
            &templates,
            &settings,
        );
        assert!(
            matches!(&modal, Modal::Presets { list, confirm: None } if list.selected(palettes.len()) == Some(index))
        );
        press(
            &mut modal,
            KeyCode::Char('x'),
            &palettes,
            &templates,
            &settings,
        );
        assert!(
            matches!(press(&mut modal, KeyCode::Char('y'), &palettes, &templates, &settings).effect, Some(Effect::RemovePreset(name)) if name == "Paper")
        );
    }

    #[test]
    fn template_overwrite_cancel_retains_text_cursor_and_list_selection() {
        let palettes = Palettes::builtin();
        let mut templates = Templates::default();
        let settings = Settings::default();
        for name in ["First", "Zine"] {
            templates
                .upsert(Template::capture(name.into(), &settings))
                .unwrap();
        }
        let mut modal = Modal::templates();
        press(&mut modal, KeyCode::Down, &palettes, &templates, &settings);
        press(
            &mut modal,
            KeyCode::Char('n'),
            &palettes,
            &templates,
            &settings,
        );
        assert!(matches!(
            press(&mut modal, KeyCode::Enter, &palettes, &templates, &settings).effect,
            Some(Effect::Notice(_))
        ));
        for c in "Zine".chars() {
            press(
                &mut modal,
                KeyCode::Char(c),
                &palettes,
                &templates,
                &settings,
            );
        }
        press(&mut modal, KeyCode::Left, &palettes, &templates, &settings);
        press(&mut modal, KeyCode::Left, &palettes, &templates, &settings);
        press(&mut modal, KeyCode::Enter, &palettes, &templates, &settings);
        assert!(matches!(
            &modal,
            Modal::Templates {
                confirm: Some(_),
                ..
            }
        ));
        press(&mut modal, KeyCode::Esc, &palettes, &templates, &settings);
        assert!(
            matches!(&modal, Modal::Templates { list, prompt: Some(input), confirm: None } if input.text() == "Zine" && list.selected(templates.len()) == Some(1))
        );
        press(
            &mut modal,
            KeyCode::Char('!'),
            &palettes,
            &templates,
            &settings,
        );
        assert!(
            matches!(&modal, Modal::Templates { prompt: Some(input), .. } if input.text() == "Zi!ne")
        );
        press(
            &mut modal,
            KeyCode::Backspace,
            &palettes,
            &templates,
            &settings,
        );
        press(&mut modal, KeyCode::Enter, &palettes, &templates, &settings);
        let step = press(
            &mut modal,
            KeyCode::Char('y'),
            &palettes,
            &templates,
            &settings,
        );
        assert!(matches!(step.next, Next::Close));
        assert!(matches!(step.effect, Some(Effect::SaveTemplate(name)) if name == "Zine"));
        press(&mut modal, KeyCode::Esc, &palettes, &templates, &settings);
        press(
            &mut modal,
            KeyCode::Char('x'),
            &palettes,
            &templates,
            &settings,
        );
        press(&mut modal, KeyCode::Esc, &palettes, &templates, &settings);
        assert!(
            matches!(&modal, Modal::Templates { list, prompt: None, confirm: None } if list.selected(templates.len()) == Some(1))
        );
        press(
            &mut modal,
            KeyCode::Char('x'),
            &palettes,
            &templates,
            &settings,
        );
        assert!(matches!(
            press(
                &mut modal,
                KeyCode::Char('y'),
                &palettes,
                &templates,
                &settings
            )
            .effect,
            Some(Effect::RemoveTemplate(name)) if name == "Zine"
        ));
    }
}
