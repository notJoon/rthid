//! Screen layout and per frame drawing of the preview, parameter panel, help line
//! and status line.
//!
//! Drawing borrows App immutably. Renderer retains preview caches and terminal
//! graphics while the panel retains its own scroll offset.

use crate::{
    brush::BrushRow,
    cells::{self, View},
    graphics,
    modal::{Modal, editor_prefix, editor_rows, preset_rows, swatch_span, template_rows},
    viewport,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout as Split, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use rthid_widgets::{Dialog, Hint, KeyHelp, ListPicker, Row, TextInput, hint_line};

use crate::{
    app::{App, BRUSH_GROUP, LAYER_GROUP, PanelItem},
    settings::{GROUPS, Setting},
};

const KEY_HELP_LEFT: &[(&str, &[Hint])] = &[
    (
        "Parameters / Brush",
        &[
            ListPicker::MOVE,
            Hint::new("Left/Right h/l", "change/close/open"),
            Hint::new("Enter", "new seed/toggle"),
            Hint::new("d", "reset"),
        ],
    ),
    (
        "Preview",
        &[
            Hint::new("hjkl/arrows", "pan"),
            Hint::new("HJKL", "page"),
            Hint::new("+/=/-", "zoom"),
            Hint::new("0", "fit"),
            Hint::new("m", "map"),
            Hint::new("M", "mass map"),
        ],
    ),
    (
        "Image",
        &[
            Hint::new("Space", "original/dithered"),
            Hint::new("a", "mass study"),
            Hint::new("r/R", "rotate"),
            Hint::new("f", "flip"),
        ],
    ),
    (
        "Layers",
        &[
            Hint::new("n", "new layer"),
            Hint::new("u/U", "undo/redo"),
            Hint::new("Enter", "target"),
            Hint::new("v", "show/hide"),
            Hint::new("x", "delete"),
            Hint::new("[/]", "move down/up"),
        ],
    ),
];

const KEY_HELP_RIGHT: &[(&str, &[Hint])] = &[
    (
        "Files",
        &[
            Hint::new("o", "open"),
            Hint::new("P", "presets"),
            Hint::new("T", "templates"),
        ],
    ),
    (
        "General",
        &[
            Hint::new("Tab/Esc", "focus"),
            Hint::new("?", "keys"),
            Hint::new("b", "brush"),
            Hint::new("q/Ctrl+C", "quit"),
        ],
    ),
    (
        "Mouse: panel/preview",
        &[
            Hint::new("minimap click/drag", "center view"),
            Hint::new("drag on preview", "move image"),
            Hint::new("brush left drag", "paint"),
            Hint::new("wheel", "pan"),
            Hint::new("layer click", "target"),
            Hint::new("click on mark", "show/hide"),
            Hint::new("layer drag", "reorder"),
        ],
    ),
    (
        "Mouse: dialogs",
        &[
            ListPicker::CLICK,
            ListPicker::CLICK_AGAIN,
            ListPicker::WHEEL,
        ],
    ),
];

fn zoom_hint(app: &App, geometry: &viewport::PreviewGeometry) -> Option<Hint> {
    let can_zoom_in =
        app.viewport.zoom.step(true, geometry.grid, geometry.area) != app.viewport.zoom;
    let can_zoom_out =
        app.viewport.zoom.step(false, geometry.grid, geometry.area) != app.viewport.zoom;
    let key = match (can_zoom_in, can_zoom_out) {
        (true, true) => "+/-",
        (true, false) => "+",
        (false, true) => "-",
        (false, false) => return None,
    };
    Some(Hint::new(key, "zoom"))
}

fn hints(app: &App, geometry: &viewport::PreviewGeometry) -> Vec<Hint> {
    let mut result = main_hints(app, geometry);
    if app.modal.is_none() && !app.keys_open && app.source_path().is_some() {
        if let Some(Row::Item(item)) = app.panel.selected(&app.panel_lens())
            && matches!(app.panel_row(item), PanelItem::Layer(_))
        {
            let at = usize::from(!app.preview_focus);
            result.splice(
                at..at,
                [
                    Hint::new("Enter", "target"),
                    Hint::new("v", "show/hide"),
                    Hint::new("x", "delete"),
                    Hint::new("[ ]", "move"),
                ],
            );
        }
        result.push(Hint::new("b", "brush"));
        if app.brush_on {
            result.push(Hint::new("drag", "paint"));
        }
        result.push(Hint::new("n", "new layer"));
        if app.edits.can_undo() {
            result.push(Hint::new("u", "undo"));
        }
        if app.edits.can_redo() {
            result.push(Hint::new("U", "redo"));
        }
    }
    result
}

fn main_hints(app: &App, geometry: &viewport::PreviewGeometry) -> Vec<Hint> {
    if app.keys_open {
        let max = key_help_scroll_max(geometry.area.height);
        let offset = app.keys_scroll.min(max);
        let mut hints = vec![Hint::new("Esc/?", "close")];
        if offset > 0 {
            hints.push(Hint::new("Up/k", "scroll up"));
        }
        if offset < max {
            hints.push(Hint::new("Down/j", "scroll down"));
        }
        return hints;
    }
    if let Some(modal) = &app.modal {
        return modal.hints(&app.modal_ctx());
    }
    let reset = app
        .selected_setting()
        .is_some_and(|setting| app.settings.is_changed(setting, &app.library.palettes));
    let image = app.source_path().is_some();
    let overflowing = geometry.overflows();
    let zoomed = app.viewport.zoom != viewport::Zoom::Fit;
    let mass_map = app.mass_map_available(geometry);
    if app.preview_focus {
        return if image {
            [
                overflowing.then_some(Hint::new("hjkl/arrows", "pan")),
                overflowing.then_some(Hint::new("HJKL", "page")),
                zoom_hint(app, geometry),
                zoomed.then_some(Hint::new("0", "fit")),
                overflowing.then_some(Hint::new("m", "map")),
                mass_map.then_some(Hint::new("M", "mass map")),
                Some(Hint::new("Space", "original/dithered")),
                Some(Hint::new("a", "mass study")),
                Some(Hint::new("Tab/Esc", "parameters")),
                Some(Hint::new("q", "quit")),
                Some(Hint::new("r/R", "rotate")),
                Some(Hint::new("f", "flip")),
                reset.then_some(Hint::new("d", "reset")),
                Some(Hint::new("o", "open")),
                Some(Hint::new("P", "presets")),
                Some(Hint::new("T", "templates")),
            ]
            .into_iter()
            .flatten()
            .collect()
        } else {
            vec![
                Hint::new("o", "open"),
                Hint::new("Tab/Esc", "parameters"),
                Hint::new("q", "quit"),
                Hint::new("P", "presets"),
                Hint::new("T", "templates"),
            ]
        };
    }
    let mut result = vec![ListPicker::MOVE];
    match app.panel.selected(&app.panel_lens()) {
        Some(Row::Header(_)) => result.extend_from_slice(app.panel.hints(&app.panel_lens())),
        Some(Row::Item(i)) => {
            if let PanelItem::Setting(setting) = app.panel_row(i)
                && setting.is_active(&app.settings)
            {
                result.push(Hint::new("Left/Right h/l", "change"));
                if app.settings.can_reseed(setting) {
                    result.push(Hint::new("Enter", "new seed"));
                }
                if reset {
                    result.push(Hint::new("d", "reset"));
                }
            } else if let PanelItem::Brush(row) = app.panel_row(i) {
                result.push(Hint::new("Left/Right h/l", "change"));
                if row.changed(app.brush) {
                    result.push(Hint::new("d", "reset"));
                }
            }
        }
        None => {}
    }
    result.extend(
        [
            Some(Hint::new("Tab", "preview")),
            Some(Hint::new("q", "quit")),
            Some(Hint::new("o", "open")),
            image.then_some(Hint::new("Space", "original/dithered")),
            image.then_some(Hint::new("a", "mass study")),
            image.then(|| zoom_hint(app, geometry)).flatten(),
            (image && zoomed).then_some(Hint::new("0", "fit")),
            (image && overflowing).then_some(Hint::new("m", "map")),
            (image && mass_map).then_some(Hint::new("M", "mass map")),
            Some(Hint::new("P", "presets")),
            Some(Hint::new("T", "templates")),
            image.then_some(Hint::new("r/R", "rotate")),
            image.then_some(Hint::new("f", "flip")),
        ]
        .into_iter()
        .flatten(),
    );
    result
}

fn more_hint(app: &App) -> Option<Hint> {
    (app.modal.is_none() && !app.keys_open).then_some(Hint::new("?", "keys"))
}

/// Terminal regions shared by input handling and rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Full terminal rectangle for modal placement.
    pub screen: Rect,
    /// Preview border and title.
    pub preview_block: Rect,
    /// Preview content inside the border, the area passed to App::sync and
    /// App::geometry.
    pub preview: Rect,
    /// Parameter list including its border.
    pub panel: Rect,
    /// Key bindings below the body.
    pub help: Rect,
    /// Preview and export status.
    pub status: Rect,
}

/// Splits the screen into the preview and a fixed width parameter panel above
/// one help row and one status row.
pub fn layout(screen: impl Into<Rect>) -> Layout {
    let screen = screen.into();
    let [body, help, status] = Split::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(screen);
    let [preview_block, panel] =
        Split::horizontal([Constraint::Min(0), Constraint::Length(29)]).areas(body);
    Layout {
        screen,
        preview_block,
        preview: Block::bordered().inner(preview_block),
        panel,
        help,
        status,
    }
}

/// Drawing state that lives across frames, so draw can update caches while App
/// stays borrowed immutably. Holds terminal graphics and RGB overview caches.
#[derive(Default)]
pub struct Renderer {
    graphics: Option<graphics::Graphics>,
    overviews: cells::OverviewCaches,
}

impl Renderer {
    /// Pass None to draw every view with cell glyphs.
    pub fn new(graphics: Option<graphics::Graphics>) -> Self {
        Self {
            graphics,
            ..Self::default()
        }
    }
}

// TODO: Use Glyphs { approximate } and Image { protocol, error } variants when the slider lands.
/// What the preview draw learned for the status line.
#[derive(Default)]
struct PreviewReport {
    approximate: bool,
    graphics_name: Option<&'static str>,
    graphics_error: Option<String>,
}

/// Writes a minimap record for RTHID_DEBUG_LOG. After a header line, each minimap
/// cell is logged as drawn next to the same cell of an undimmed overview.
///
/// Only the header is written when no minimap is shown.
pub fn log_minimap(
    app: &App,
    log: &mut impl std::io::Write,
    frame: &ratatui::CompletedFrame<'_>,
    geometry: &viewport::PreviewGeometry,
) -> std::io::Result<()> {
    let (w, h) = geometry.grid;
    writeln!(
        log,
        "[minimap] frame={} grid={w}x{h} view={:?} area={:?}",
        frame.count, app.viewport, geometry.area
    )?;
    let Some(pixels) = app.inset() else {
        return log.flush();
    };
    let mut overview = ratatui::buffer::Buffer::empty(geometry.area);
    if let Some(viewport::Minimap {
        frame: inset,
        map,
        viewport,
    }) = geometry.minimap
    {
        let mut fitted = viewport::geometry(&viewport::Viewport::default(), geometry.grid, map);
        fitted.target = map;
        cells::render(
            pixels,
            pixels,
            app.glyphs,
            &fitted,
            &mut overview,
            &mut cells::OverviewCaches::default(),
        );
        writeln!(log, "inset={inset:?} map={map:?} viewport={viewport:?}")?;
        for pos in inset.positions() {
            writeln!(
                log,
                "{},{} displayed={:?} overview={:?}",
                pos.x, pos.y, frame.buffer[pos], overview[pos]
            )?;
        }
    }
    log.flush()
}

fn brush_group(app: &App) -> (Line<'_>, Vec<Line<'_>>) {
    let changed = BrushRow::ALL
        .into_iter()
        .filter(|row| row.changed(app.brush))
        .count();
    let title = if app.panel.is_collapsed(BRUSH_GROUP) && changed > 0 {
        Line::from(vec![
            Span::raw("Brush"),
            Span::styled(
                format!("  {changed} changed"),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::raw("Brush")
    };
    // Collapsed items are still passed so the rendered lens matches App::panel_lens.
    let items = BrushRow::ALL
        .into_iter()
        .map(|row| Line::raw(row.label(app.brush)))
        .collect();
    (title, items)
}

fn layer_group(app: &App) -> (Line<'_>, Vec<Line<'_>>) {
    let doc = app.edits.current();
    let dim = Style::default().fg(Color::DarkGray);
    let title = if app.panel.is_collapsed(LAYER_GROUP) {
        Line::from(vec![
            Span::raw("Layers"),
            Span::styled(format!("  {}", doc.layers().len()), dim),
        ])
    } else {
        Line::raw("Layers")
    };
    let items = doc
        .layers()
        .iter()
        .enumerate()
        .rev()
        .map(|(index, layer)| {
            let line = Line::raw(format!(
                "{} {}{}",
                if layer.visible { "●" } else { "○" },
                layer.name,
                if doc.target() == Some(index) {
                    " *"
                } else {
                    ""
                }
            ));
            if layer.visible { line } else { line.style(dim) }
        })
        .collect();
    (title, items)
}

/// Draws using a shared App borrow and retains the panel scroll offset.
/// Expects App::sync to have run for this layout.
pub fn draw(frame: &mut Frame, app: &App, layout: &Layout, renderer: &mut Renderer) {
    let focus_style =
        |focused| Style::default().fg(if focused { Color::Cyan } else { Color::Reset });
    let block = Block::bordered()
        .title("rthid")
        .border_style(focus_style(app.preview_focus));
    let geometry = app.geometry(layout.preview);
    let report = draw_preview(frame, app, &geometry, renderer);
    frame.render_widget(block, layout.preview_block);
    let mut first = 0;
    let groups = GROUPS
        .iter()
        .enumerate()
        .map(|(group, &(title, len))| {
            let settings = &Setting::ALL[first..first + len];
            first += len;
            // Collapsed groups still pass their items so the rendered lens
            // matches App::panel_lens and mouse input stays live.
            let items = settings
                .iter()
                .map(|&setting| {
                    let label = Line::raw(setting.label(&app.settings, &app.library.palettes));
                    if setting.is_active(&app.settings) {
                        label
                    } else {
                        label.style(Style::default().fg(Color::DarkGray))
                    }
                })
                .collect();
            let changed = settings
                .iter()
                .filter(|&&setting| app.settings.is_changed(setting, &app.library.palettes))
                .count();
            let title = if app.panel.is_collapsed(group) && changed > 0 {
                Line::from(vec![
                    Span::raw(title),
                    Span::styled(
                        format!("  {changed} changed"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            } else {
                Line::raw(title)
            };
            (title, items)
        })
        .chain(std::iter::once(brush_group(app)))
        .chain(std::iter::once(layer_group(app)));
    let block = Block::bordered()
        .title("Parameters")
        .border_style(focus_style(!app.preview_focus));
    let highlight = Style::default().fg(Color::Black).bg(Color::Cyan);
    app.panel.render(
        groups,
        Some(block),
        highlight,
        layout.panel,
        frame.buffer_mut(),
    );
    frame.render_widget(
        Paragraph::new(hint_line(
            &hints(app, &geometry),
            more_hint(app),
            layout.help.width,
        )),
        layout.help,
    );
    frame.render_widget(
        Paragraph::new(status(app, &geometry, report)),
        layout.status,
    );
    if app.keys_open {
        draw_key_help(frame, app, layout);
    } else if app.modal.is_some() {
        draw_modal(frame, app, layout);
    }
}

const KEY_HELP: (KeyHelp, KeyHelp) = (
    KeyHelp {
        sections: KEY_HELP_LEFT,
    },
    KeyHelp {
        sections: KEY_HELP_RIGHT,
    },
);
const KEY_HELP_GAP: u16 = 3;
const KEY_HELP_DIALOG: Dialog = Dialog {
    title: "Keys",
    min_width: 28,
};

fn key_help_area(layout: &Layout) -> Rect {
    let (left, right) = (KEY_HELP.0.size(), KEY_HELP.1.size());
    let body = Rect {
        height: layout.help.y.saturating_sub(layout.screen.y),
        ..layout.screen
    };
    KEY_HELP_DIALOG.area(
        layout.preview,
        body,
        (
            left.0.saturating_add(KEY_HELP_GAP).saturating_add(right.0),
            left.1.max(right.1),
        ),
    )
}

/// Maximum key reference offset for the available content height.
pub(crate) fn key_help_scroll_max(height: u16) -> u16 {
    KEY_HELP
        .0
        .size()
        .1
        .max(KEY_HELP.1.size().1)
        .saturating_sub(height)
}

fn draw_key_help(frame: &mut Frame, app: &App, layout: &Layout) {
    let (left, right) = KEY_HELP;
    let area = key_help_area(layout);
    let inner = KEY_HELP_DIALOG.render(area, frame.buffer_mut());
    let content = inner.inner(Margin::new(1, 0));
    let height = left.size().1.max(right.size().1);
    let max = key_help_scroll_max(content.height);
    let offset = app.keys_scroll.min(max);
    let mut buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, content.width, height));
    left.render(buffer.area, &mut buffer);
    let right_offset = left.size().0.saturating_add(KEY_HELP_GAP);
    if content.width > right_offset {
        right.render(
            Rect::new(right_offset, 0, content.width - right_offset, height),
            &mut buffer,
        );
    }
    for y in 0..content.height {
        for x in 0..content.width {
            frame.buffer_mut()[(content.x + x, content.y + y)] = buffer[(x, y + offset)].clone();
        }
    }
    for (visible, y, label) in [
        (offset > 0, area.y, "↑ more"),
        (offset < max, area.bottom().saturating_sub(1), "↓ more"),
    ] {
        if visible && area.width >= 8 && area.height > 0 {
            frame.buffer_mut().set_string(
                area.right() - 8,
                y,
                label,
                Style::default().fg(Color::DarkGray),
            );
        }
    }
}

struct ModalContent<'a> {
    title: &'a str,
    rows: Vec<Line<'static>>,
    picker: Option<&'a ListPicker>,
}

struct Editing<'a> {
    input: &'a TextInput,
    prefix: String,
    invalid: bool,
    swatch: Option<[u8; 3]>,
}

fn draw_modal(frame: &mut Frame, app: &App, screen_layout: &Layout) {
    let modal = app.modal.as_ref().unwrap();
    let layout = modal.layout(
        screen_layout.preview,
        screen_layout.screen,
        &app.modal_ctx(),
    );
    let mut editing = None;
    let ModalContent {
        title,
        rows,
        picker,
    } = match modal {
        Modal::OpenFile(picker) => {
            let rows: Vec<_> = picker
                .entries
                .iter()
                .map(|entry| {
                    Line::raw(format!(
                        "{}{}",
                        entry.name,
                        if entry.dir { "/" } else { "" }
                    ))
                })
                .collect();
            Dialog {
                title: "Open image",
                min_width: 28,
            }
            .render(layout.dialog, frame.buffer_mut());
            let field = layout.field.unwrap();
            let entries = layout.list.unwrap();
            if let Some(cursor) =
                picker
                    .input
                    .render("", Style::default(), field, frame.buffer_mut())
            {
                frame.set_cursor_position(cursor);
            }
            picker.list.render(
                rows,
                None,
                Style::default().bg(Color::DarkGray),
                entries,
                frame.buffer_mut(),
            );
            return;
        }
        Modal::Templates {
            list,
            prompt,
            confirm,
        } => {
            let title = confirm
                .as_ref()
                .map(|(question, _)| question.message())
                .unwrap_or(if prompt.is_some() {
                    "Save template"
                } else {
                    "Templates"
                });
            if let Some(input) = prompt {
                editing = Some(Editing {
                    input,
                    prefix: "Name: ".to_owned(),
                    invalid: false,
                    swatch: None,
                });
                ModalContent {
                    title,
                    rows: vec![Line::raw(format!("Name: {}", input.text()))],
                    picker: None,
                }
            } else {
                let rows = template_rows(&app.modal_ctx());
                ModalContent {
                    title,
                    rows,
                    picker: Some(list),
                }
            }
        }
        Modal::Presets { list, confirm } => {
            let rows = preset_rows(&app.modal_ctx());
            ModalContent {
                title: confirm
                    .as_ref()
                    .map_or("Presets", |(question, _)| question.message()),
                rows,
                picker: Some(list),
            }
        }
        Modal::Editor(editor) => {
            if let Some(input) = &editor.input
                && let Some(row) = editor.selected_row()
            {
                editing = Some(Editing {
                    input,
                    prefix: editor_prefix(row),
                    invalid: editor.invalid_hex(),
                    swatch: editor.swatch_rgb(row),
                });
            }
            ModalContent {
                title: "Preset editor",
                rows: editor_rows(editor),
                picker: Some(&editor.rows),
            }
        }
    };
    let dialog = Dialog {
        title,
        min_width: 28,
    };
    dialog.render(layout.dialog, frame.buffer_mut());
    let highlight = Style::default().bg(Color::DarkGray);
    if let Some(picker) = picker {
        picker.render(
            rows,
            None,
            highlight,
            layout.list.unwrap(),
            frame.buffer_mut(),
        );
    }
    if let Some(Editing {
        input,
        prefix,
        invalid,
        swatch,
    }) = editing
        && let Some(text_area) = modal
            .layout(
                screen_layout.preview,
                screen_layout.screen,
                &app.modal_ctx(),
            )
            .field
    {
        frame.render_widget(Paragraph::new("").style(highlight), text_area);
        let style = highlight.fg(if invalid { Color::Red } else { Color::Reset });
        if let Some(cursor) = input.render(&prefix, style, text_area, frame.buffer_mut()) {
            frame.set_cursor_position(cursor);
        }
        let width = Line::raw(format!("{prefix}{}", input.text())).width();
        if let Some(rgb) = swatch
            && width + 4 <= usize::from(text_area.width)
        {
            frame.render_widget(
                Paragraph::new(swatch_span(rgb, highlight)),
                Rect::new(text_area.x + width as u16, text_area.y, 4, 1),
            );
        }
    }
}

// Chooses the nearest fixed xterm color to shorten Original preview output during dragging.
fn rgb_to_ansi256(rgb: [u8; 3]) -> u8 {
    let levels: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let cube = rgb.map(|v| match v {
        0..=47 => 0,
        48..=114 => 1,
        _ => 2 + (v - 115) / 40,
    });
    let sum: i32 = rgb.into_iter().map(i32::from).sum();
    let gray = ((sum - 24 + 15) / 30).clamp(0, 23) as u8;
    let mut cube_distance = 0;
    let mut gray_distance = 0;
    for channel in 0..3 {
        cube_distance +=
            (i32::from(rgb[channel]) - i32::from(levels[usize::from(cube[channel])])).pow(2);
        gray_distance += (i32::from(rgb[channel]) - i32::from(8 + 10 * gray)).pow(2);
    }
    if gray_distance < cube_distance {
        232 + gray
    } else {
        16 + 36 * cube[0] + 6 * cube[1] + cube[2]
    }
}

// Uses terminal graphics only for Original at Fit with the brush off.
// Calls present every frame so a hidden image is cleared.
fn draw_preview(
    frame: &mut Frame,
    app: &App,
    geometry: &viewport::PreviewGeometry,
    renderer: &mut Renderer,
) -> PreviewReport {
    let Some(pixels) = app.displayed() else {
        if let Some(graphics) = &mut renderer.graphics {
            graphics.present(frame, None);
        }
        let hint = geometry
            .area
            .centered(Constraint::Percentage(100), Constraint::Length(2));
        frame.render_widget(
            Paragraph::new("No image\no: open file | drop an image file here").centered(),
            hint,
        );
        return PreviewReport::default();
    };
    let image_mode = app.modal.is_none()
        && !app.keys_open
        && !app.brush_on
        && renderer.graphics.is_some()
        && app.view == View::Original
        && geometry.zoom == viewport::Zoom::Fit
        && !geometry.area.is_empty();
    let approximate = if image_mode {
        false
    } else {
        let approximate = cells::render(
            pixels,
            app.inset().unwrap_or(pixels),
            app.glyphs,
            geometry,
            frame.buffer_mut(),
            &mut renderer.overviews,
        );
        if app.dragging() && matches!(pixels, cells::Pixels::Rgb { .. }) {
            cells::map_rgb_colors(
                frame.buffer_mut(),
                geometry
                    .target
                    .positions()
                    .filter(|&pos| !geometry.minimap.is_some_and(|map| map.frame.contains(pos))),
                |rgb| Color::Indexed(rgb_to_ansi256(rgb)),
            );
        }
        approximate
    };
    let mut report = PreviewReport {
        approximate,
        ..PreviewReport::default()
    };
    if let Some(graphics) = &mut renderer.graphics {
        graphics.present(
            frame,
            image_mode.then(|| {
                (
                    app.grid_cache.grid().expect("processed grid"),
                    app.grid_cache.grid_version(),
                    geometry,
                )
            }),
        );
        if image_mode {
            report.graphics_name = Some(graphics.name());
            report.graphics_error = graphics.error().map(str::to_owned);
        }
    }
    report
}

// PNG errors, config notices and graphics errors replace the normal status line.
fn status(app: &App, geometry: &viewport::PreviewGeometry, report: PreviewReport) -> String {
    if let Some(message) = app.notice() {
        return message.to_owned();
    }
    let Some(placement) = app.grid_cache.placement(&app.settings) else {
        return "No image".into();
    };
    let source = app.source_size();
    let dimensions = app.settings.dimensions(source).expect("valid placement");
    let (card_w, card_h) = dimensions.card_mm;
    let (gw, gh) = geometry.grid;
    let mode = if geometry.zoom == viewport::Zoom::Fit {
        format!(
            "Fit ({}){}",
            app.glyphs.name(),
            if report.approximate {
                " approximate"
            } else {
                ""
            }
        )
    } else {
        let percent = (geometry.zoom.scale(geometry.grid, geometry.area) * 100.0).round() as u32;
        let (w, h) = geometry.visible;
        if w == 0 || h == 0 {
            format!("{percent}% empty")
        } else {
            format!(
                "{percent}% x {}-{} y {}-{}",
                geometry.origin.0 + 1,
                geometry.origin.0 + w,
                geometry.origin.1 + 1,
                geometry.origin.1 + h
            )
        }
    };
    let mode = if app.brush_on {
        format!(
            "{mode} | Brush {} {}px {}%",
            app.brush.mode.name(),
            app.brush.radius,
            app.brush.strength
        )
    } else {
        mode
    };
    let displayed = if let Some(name) = report.graphics_name {
        format!("Original ({name})")
    } else {
        app.view.name().into()
    };
    report.graphics_error.unwrap_or_else(|| {
        format!(
            // Print fields lead so an 80 column terminal clips navigation, not them.
            "Card {}x{}{}mm | Dots {gw}x{gh} @{:.2}mm | Rot {}°{}{}{} | {displayed} | {mode}{}",
            card_w,
            card_h,
            if app.settings.bleed > 0 && app.settings.card.size_mm.is_some() {
                format!("+{}", app.settings.bleed)
            } else {
                String::new()
            },
            f64::from(app.settings.dot) / 100.0,
            app.settings.rotation.degrees(),
            if app.settings.flip { " Flip" } else { "" },
            app.export_dpi()
                .and_then(|dpi| crate::print::png_size(&app.settings, source, dpi)
                    .ok()
                    .map(|(w, h)| { format!(" | PNG {w}x{h} @{dpi}dpi") }))
                .unwrap_or_default(),
            if placement.aw == 0 {
                " | no image area"
            } else {
                ""
            },
            if app.mass_inset_shown(geometry) {
                " | map Mass"
            } else {
                ""
            },
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{BUSINESS_CARD, Dither, Preset, Template};
    use image::RgbaImage;
    use ratatui::{
        Terminal, TerminalOptions, Viewport,
        backend::TestBackend,
        crossterm::{
            event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind},
            style::Colored,
        },
    };
    use std::io::{self, BufWriter, Write};

    #[test]
    fn brush_panel_changed_count_status_and_hints() {
        let mut app = App::builtin(RgbaImage::new(120, 120), None);
        let (title, rows) = brush_group(&app);
        assert_eq!(title.to_string(), "Brush");
        assert_eq!(
            rows.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["Mode  Dodge", "Radius  16 px", "Strength  20%"]
        );
        app.panel
            .select(Row::Header(BRUSH_GROUP), &app.panel_lens());
        let geometry = app.geometry(layout(Rect::new(0, 0, 100, 40)).preview);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &geometry);
        assert_eq!(brush_group(&app).0.to_string(), "Brush");
        app.brush.radius = 24;
        app.brush.strength = 25;
        let (title, rows) = brush_group(&app);
        assert_eq!(title.to_string(), "Brush  2 changed");
        assert_eq!(title.spans[1].style.fg, Some(Color::DarkGray));
        // Outline hides collapsed rows, so the group still passes all of them.
        assert_eq!(rows.len(), BrushRow::ALL.len());
        app.brush_on = true;
        assert!(status(&app, &geometry, PreviewReport::default()).contains("Brush Dodge 24px 25%"));
        let context = main_hints(&app, &geometry);
        let actual = hints(&app, &geometry);
        assert_eq!(&actual[..context.len()], context);
        assert_eq!(
            &actual[context.len()..],
            [
                Hint::new("b", "brush"),
                Hint::new("drag", "paint"),
                Hint::new("n", "new layer")
            ]
        );
        app.panel
            .select(Row::Item(Setting::ALL.len() + 1), &app.panel_lens());
        assert!(hints(&app, &geometry).contains(&Hint::new("d", "reset")));
        app.brush_on = false;
        assert!(!hints(&app, &geometry).contains(&Hint::new("drag", "paint")));
    }

    #[test]
    fn layer_panel_renders_top_first_visibility_target_and_collapsed_count() {
        let mut app = hint_fixture(HintState::Layer);
        app.edits.edit(|doc| doc.set_visible(1, false));
        let (_, items) = layer_group(&app);
        assert_eq!(
            items.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["● Layer 3", "○ Layer 2 *", "● Layer 1"]
        );
        assert_eq!(items[1].style.fg, Some(Color::DarkGray));
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut renderer = Renderer::default();
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert!(text.contains("● Layer 3"));
        assert!(text.contains("○ Layer 2 *"));
        app.panel
            .select(Row::Header(LAYER_GROUP), &app.panel_lens());
        let geometry = app.geometry(layout(terminal.size().unwrap()).preview);
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &geometry);
        let (title, _) = layer_group(&app);
        assert_eq!(title.to_string(), "Layers  3");
        assert_eq!(title.spans[1].style.fg, Some(Color::DarkGray));
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert!(text.contains("Layers  3"));
        assert!(!text.contains("Layer 2"));
    }

    #[test]
    fn layer_hints_follow_source_history_row_and_modal_state() {
        let mut app = hint_fixture(HintState::LayerRedo);
        let geometry = app.geometry(layout(Rect::new(0, 0, 100, 40)).preview);
        for preview in [false, true] {
            app.preview_focus = preview;
            let hints = hints(&app, &geometry);
            for expected in [
                Hint::new("n", "new layer"),
                Hint::new("u", "undo"),
                Hint::new("U", "redo"),
                Hint::new("Enter", "target"),
                Hint::new("v", "show/hide"),
                Hint::new("x", "delete"),
                Hint::new("[ ]", "move"),
            ] {
                assert!(hints.contains(&expected), "{expected:?}");
            }
        }
        app.modal = Some(Modal::templates());
        assert!(!hints(&app, &geometry).contains(&Hint::new("n", "new layer")));
        let app = App::empty();
        assert!(!hints(&app, &geometry).contains(&Hint::new("n", "new layer")));
    }

    #[test]
    fn ansi256_uses_nearest_cube_or_gray() {
        for (rgb, expected) in [
            ([0, 0, 0], 16),
            ([255, 255, 255], 231),
            ([255, 0, 0], 196),
            ([0, 255, 0], 46),
            ([0, 0, 255], 21),
            ([128, 128, 128], 244),
            ([95, 135, 175], 67),
            ([8, 8, 8], 232),
            ([238, 238, 238], 255),
        ] {
            assert_eq!(rgb_to_ansi256(rgb), expected, "{rgb:?}");
        }
    }

    fn draw_text(
        terminal: &mut Terminal<TestBackend>,
        app: &App,
        renderer: &mut Renderer,
    ) -> String {
        terminal
            .draw(|frame| draw(frame, app, &layout(frame.area()), renderer))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[derive(Clone, Copy, Debug)]
    enum HintState {
        EmptyPanel,
        EmptyPreview,
        FitPanel,
        Layer,
        LayerRedo,
        FitPreview,
        SmallFitPreview,
        OverflowPreview,
        Seed,
        SeedPreview,
        Header,
        CollapsedHeader,
        DiffusionSkip,
        InactiveRow,
        Keys,
        OpenFile,
        Presets,
        PresetsConfirm,
        TemplatesEmpty,
        Templates,
        TemplatesPrompt,
        TemplatesConfirm,
        EditorColor,
        EditorMode,
        EditorInput,
    }

    fn hint_fixture(state: HintState) -> App {
        use crate::modal::{Editor, OnYes};
        use rthid_widgets::Confirm;

        let mut app = match state {
            HintState::EmptyPanel | HintState::EmptyPreview | HintState::OpenFile => App::empty(),
            HintState::SmallFitPreview => App::builtin(RgbaImage::new(10, 10), None),
            _ => App::builtin(RgbaImage::new(120, 120), None),
        };
        let add_templates = |app: &mut App| {
            for (name, seed) in [("One", 1), ("Two", 2), ("Three", 3)] {
                let mut settings = app.settings.clone();
                settings.seed = seed;
                app.library
                    .templates
                    .upsert(Template::capture(name.into(), &settings))
                    .unwrap();
            }
        };
        let row = |setting| Setting::ALL.iter().position(|s| *s == setting).unwrap();
        match state {
            HintState::EmptyPanel => app
                .panel
                .select(Row::Item(row(Setting::Zoom)), &app.panel_lens()),
            HintState::EmptyPreview | HintState::FitPreview | HintState::SmallFitPreview => {
                app.preview_focus = true
            }
            HintState::Layer | HintState::LayerRedo => {
                for _ in 0..3 {
                    app.edits.edit(|doc| {
                        doc.add_layer(format!("Layer {}", doc.layers().len() + 1));
                    });
                }
                app.edits.select(Some(1));
                if matches!(state, HintState::LayerRedo) {
                    app.edits.edit(|doc| doc.set_visible(1, false));
                    app.edits.undo();
                }
                app.panel.select(
                    Row::Item(Setting::ALL.len() + BrushRow::ALL.len() + 1),
                    &app.panel_lens(),
                );
            }
            HintState::FitPanel => {
                app.panel
                    .select(Row::Item(row(Setting::Brightness)), &app.panel_lens());
                app.settings.tone.brightness = 0.1;
            }
            HintState::OverflowPreview => {
                app.preview_focus = true;
                app.viewport.zoom = viewport::Zoom::In(1);
            }
            HintState::Seed | HintState::SeedPreview => {
                app.panel
                    .select(Row::Item(row(Setting::Seed)), &app.panel_lens());
                app.settings.dither = Dither::Noise;
                app.settings.seed = 7;
                app.preview_focus = matches!(state, HintState::SeedPreview);
            }
            HintState::Header | HintState::CollapsedHeader => {
                app.panel.select(Row::Header(1), &app.panel_lens());
                if matches!(state, HintState::CollapsedHeader) {
                    app.panel.handle_key(
                        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                        &app.panel_lens(),
                    );
                }
            }
            HintState::DiffusionSkip => {
                app.panel
                    .select(Row::Item(row(Setting::Serpentine)), &app.panel_lens());
            }
            HintState::InactiveRow => app
                .panel
                .select(Row::Item(row(Setting::Seed)), &app.panel_lens()),
            HintState::Keys => app.keys_open = true,
            HintState::OpenFile => {
                app.modal = Some(Modal::open_file("/images".into(), 1));
                app.set_listing(
                    "/images".into(),
                    (0..3)
                        .map(|i| rthid_io::dir::Entry {
                            file_name: format!("image{i}.png").into(),
                            name: format!("image{i}.png"),
                            dir: false,
                        })
                        .collect(),
                );
                if let Some(Modal::OpenFile(picker)) = &mut app.modal {
                    picker.list.select(1);
                    picker.input = TextInput::new(
                        "/images/abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz.png",
                        4096,
                    );
                }
            }
            HintState::Presets => {
                app.modal = Some(Modal::presets(&app.settings, &app.library.palettes));
                if let Some(Modal::Presets { list, .. }) = &mut app.modal {
                    list.select(2);
                }
            }
            HintState::PresetsConfirm => {
                app.modal = Some(Modal::Presets {
                    list: ListPicker::new(2),
                    confirm: Some((
                        Confirm::new("Delete preset?"),
                        OnYes::RemovePreset("Warm".into()),
                    )),
                });
            }
            HintState::TemplatesEmpty => app.modal = Some(Modal::templates()),
            HintState::Templates => {
                add_templates(&mut app);
                app.modal = Some(Modal::Templates {
                    list: ListPicker::new(1),
                    prompt: None,
                    confirm: None,
                });
            }
            HintState::TemplatesPrompt => {
                app.modal = Some(Modal::Templates {
                    list: ListPicker::new(0),
                    prompt: Some(TextInput::new("abcdefghijklmnopqrstuvwxyzabcdef", 32)),
                    confirm: None,
                });
            }
            HintState::TemplatesConfirm => {
                add_templates(&mut app);
                app.modal = Some(Modal::Templates {
                    list: ListPicker::new(1),
                    prompt: Some(TextInput::new("Two", 32)),
                    confirm: Some((
                        Confirm::new("Overwrite template?"),
                        OnYes::OverwriteTemplate("Two".into()),
                    )),
                });
            }
            HintState::EditorColor | HintState::EditorMode | HintState::EditorInput => {
                let mut editor = Editor::new(
                    &app.modal_ctx(),
                    Preset {
                        name: "Draft".into(),
                        colors: vec!["#000000".into(), "#888888".into(), "#ffffff".into()],
                        mode: None,
                        builtin: false,
                    },
                );
                editor.rows.select(match state {
                    HintState::EditorMode => 4,
                    HintState::EditorInput => 0,
                    _ => 2,
                });
                if matches!(state, HintState::EditorInput) {
                    editor.input = Some(TextInput::new("abcdefghijklmnopqrstuvwxyzabcdef", 32));
                }
                app.modal = Some(Modal::Editor(Box::new(editor)));
            }
        }
        app
    }

    fn named_keys(text: &str) -> Vec<ratatui::crossterm::event::KeyEvent> {
        use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
        if text == "any other key" {
            return vec![key(KeyCode::Esc)];
        }
        text.split_whitespace()
            .flat_map(|group| group.split('/'))
            .flat_map(|token| match token {
                "Enter" => vec![key(KeyCode::Enter)],
                "Esc" => vec![key(KeyCode::Esc)],
                "Tab" => vec![key(KeyCode::Tab)],
                "Space" => vec![key(KeyCode::Char(' '))],
                "Up" => vec![key(KeyCode::Up)],
                "Down" => vec![key(KeyCode::Down)],
                "Left" => vec![key(KeyCode::Left)],
                "Right" => vec![key(KeyCode::Right)],
                "Home" => vec![key(KeyCode::Home)],
                "End" => vec![key(KeyCode::End)],
                "Ctrl+C" => vec![KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)],
                "arrows" => [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right]
                    .into_iter()
                    .map(key)
                    .collect(),
                token => token.chars().map(|c| key(KeyCode::Char(c))).collect(),
            })
            .collect()
    }

    const HINT_STATES: &[HintState] = &[
        HintState::EmptyPanel,
        HintState::EmptyPreview,
        HintState::FitPanel,
        HintState::Layer,
        HintState::LayerRedo,
        HintState::FitPreview,
        HintState::SmallFitPreview,
        HintState::OverflowPreview,
        HintState::Seed,
        HintState::SeedPreview,
        HintState::Header,
        HintState::CollapsedHeader,
        HintState::DiffusionSkip,
        HintState::InactiveRow,
        HintState::Keys,
        HintState::OpenFile,
        HintState::Presets,
        HintState::PresetsConfirm,
        HintState::TemplatesEmpty,
        HintState::Templates,
        HintState::TemplatesPrompt,
        HintState::TemplatesConfirm,
        HintState::EditorColor,
        HintState::EditorMode,
        HintState::EditorInput,
    ];

    fn state_hints(app: &App, screen: &Layout) -> Vec<Hint> {
        let mut result = hints(app, &app.geometry(screen.preview));
        result.extend(more_hint(app));
        result
    }

    fn key_changes(state: HintState, key: ratatui::crossterm::event::KeyEvent) -> bool {
        use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

        let mut app = hint_fixture(state);
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
        let mut renderer = Renderer::default();
        let screen = layout(Rect::new(0, 0, 80, 30));
        app.sync(screen.preview);
        if matches!(state, HintState::OverflowPreview) {
            let geometry = app.geometry(screen.preview);
            app.viewport.x = (geometry.grid.0 - geometry.visible.0) / 2;
            app.viewport.y = (geometry.grid.1 - geometry.visible.1) / 2;
        }
        if matches!(key.code, KeyCode::Right | KeyCode::End)
            && let Some(modal) = &mut app.modal
            && let Some(input) = modal.text_input_mut()
        {
            input.handle_key(KeyEvent::new(
                if key.code == KeyCode::Right {
                    KeyCode::Left
                } else {
                    KeyCode::Home
                },
                KeyModifiers::NONE,
            ));
        }
        let before = snapshot(&mut app, &mut terminal, &mut renderer);
        let geometry = app.geometry(screen.preview);
        app.handle_key(key, &geometry);
        before != snapshot(&mut app, &mut terminal, &mut renderer)
    }

    fn snapshot(
        app: &mut App,
        terminal: &mut Terminal<TestBackend>,
        renderer: &mut Renderer,
    ) -> (ratatui::buffer::Buffer, bool, bool) {
        let screen = layout(terminal.size().unwrap());
        app.sync(screen.preview);
        terminal
            .draw(|frame| draw(frame, app, &screen, renderer))
            .unwrap();
        // Brush mode can change while its status and hints are clipped.
        (terminal.backend().buffer().clone(), app.quit, app.brush_on)
    }

    #[test]
    fn every_advertised_key_changes_the_representative_state() {
        for &state in HINT_STATES {
            let mut source = hint_fixture(state);
            let screen = layout(Rect::new(0, 0, 80, 30));
            source.sync(screen.preview);
            let state_hints = state_hints(&source, &screen);
            for hint in state_hints {
                for key in named_keys(hint.key) {
                    if matches!(state, HintState::EditorMode)
                        && matches!(key.code, KeyCode::Down | KeyCode::Char('j'))
                    {
                        continue;
                    }
                    assert!(key_changes(state, key), "{state:?}: {} ({key:?})", hint.key);
                }
            }
        }
    }

    #[test]
    fn every_overlay_key_is_handled_in_a_representative_state() {
        for sections in [KEY_HELP_LEFT, KEY_HELP_RIGHT] {
            for &(section, section_hints) in sections {
                if section.starts_with("Mouse:") {
                    continue;
                }
                for hint in section_hints {
                    for key in named_keys(hint.key) {
                        assert!(
                            HINT_STATES.iter().copied().any(|state| {
                                hint_fixture(state).modal.is_none() && key_changes(state, key)
                            }),
                            "{section}: {} ({key:?})",
                            hint.key
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn changed_keys_are_advertised_outside_dialogs() {
        use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

        let keys = ('!'..='~')
            .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
            .chain(named_keys("Space Enter Esc Tab arrows"));
        for &state in HINT_STATES {
            let mut app = hint_fixture(state);
            if app.modal.is_some() {
                continue;
            }
            let screen = layout(Rect::new(0, 0, 80, 30));
            app.sync(screen.preview);
            let advertised: Vec<_> = state_hints(&app, &screen)
                .iter()
                .flat_map(|hint| named_keys(hint.key))
                .collect();
            for key in keys.clone() {
                let allowed = match key.code {
                    // Enter also rerolls an active Noise seed while preview has focus.
                    KeyCode::Enter => {
                        app.preview_focus
                            && app
                                .selected_setting()
                                .is_some_and(|setting| app.settings.can_reseed(setting))
                    }
                    KeyCode::Char('u') => !app.edits.can_undo(),
                    KeyCode::Char('U') => !app.edits.can_redo(),
                    // = is the unshifted alias for the advertised + zoom binding.
                    KeyCode::Char('=') => true,
                    _ => false,
                };
                assert!(
                    !key_changes(state, key) || advertised.contains(&key) || allowed,
                    "{state:?}: changed without a hint: {key:?}"
                );
            }
        }
    }

    #[test]
    fn key_overlay_clips_on_a_tiny_screen_and_captures_input() {
        use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

        let mut app = hint_fixture(HintState::Keys);
        let mut renderer = Renderer::default();
        let mut terminal = Terminal::new(TestBackend::new(36, 6)).unwrap();
        let before = draw_text(&mut terminal, &app, &mut renderer);
        assert!(before.contains("Keys"));
        assert!(before.contains("Esc/?: close"));
        let geometry = app.geometry(layout(terminal.size().unwrap()).preview);
        app.handle_key(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            &geometry,
        );
        assert_eq!(draw_text(&mut terminal, &app, &mut renderer), before);
        assert!(!app.handle_mouse(
            ratatui::crossterm::event::MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 1,
                row: 1,
                modifiers: KeyModifiers::NONE,
            },
            &geometry,
            Rect::new(0, 0, 36, 6),
        ));
        assert!(app.keys_open);
        app.set_notice("notice".into());
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &geometry);
        assert!(!app.keys_open);
        assert!(app.notice().is_none());
    }

    fn key_overlay_rows(width: u16, height: u16) -> Vec<String> {
        let app = hint_fixture(HintState::Keys);
        let mut renderer = Renderer::default();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        draw_text(&mut terminal, &app, &mut renderer);
        let inner = Block::bordered().inner(key_help_area(&layout(Rect::new(0, 0, width, height))));
        let buffer = terminal.backend().buffer();
        inner
            .rows()
            .map(|row| {
                row.columns()
                    .map(|column| buffer[(column.x, row.y)].symbol())
                    .collect()
            })
            .collect()
    }

    /// Returns the first section or `key  label` row the overlay does not show in full.
    fn key_overlay_missing(width: u16, height: u16) -> Option<String> {
        let rows = key_overlay_rows(width, height);
        let has = |key: &str, label: &str| {
            rows.iter().any(|row| {
                row.find(key)
                    .is_some_and(|start| row[start + key.len()..].contains(label))
            })
        };
        [KEY_HELP_LEFT, KEY_HELP_RIGHT]
            .into_iter()
            .flatten()
            .find_map(|&(section, hints)| {
                if !rows.iter().any(|row| row.contains(section)) {
                    return Some(section.to_string());
                }
                hints
                    .iter()
                    .find(|hint| !has(hint.key, hint.label))
                    .map(|hint| format!("{}  {}", hint.key, hint.label))
            })
    }

    #[test]
    fn key_overlay_fits_every_reference_row_at_71_by_31() {
        assert_eq!(key_overlay_missing(71, 31), None);
        assert!(key_overlay_missing(70, 31).is_some());
        assert!(key_overlay_missing(71, 30).is_some());
        assert_eq!(key_overlay_missing(80, 31), None);
        assert_eq!(key_overlay_missing(80, 32), None);
    }

    #[test]
    fn key_overlay_scroll_reaches_last_entry_at_80_by_24() {
        use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

        let mut app = hint_fixture(HintState::Keys);
        let mut renderer = Renderer::default();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let geometry = app.geometry(layout(Rect::new(0, 0, 80, 24)).preview);
        let before = draw_text(&mut terminal, &app, &mut renderer);
        assert!(before.contains("↓ more"));
        assert!(!before.contains("↑ more"));
        assert!(main_hints(&app, &geometry).contains(&Hint::new("Down/j", "scroll down")));
        assert!(!main_hints(&app, &geometry).contains(&Hint::new("Up/k", "scroll up")));
        for _ in 0..50 {
            app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &geometry);
        }
        let after = draw_text(&mut terminal, &app, &mut renderer);
        assert!(after.contains("move down/up"));
        assert!(after.contains("↑ more"));
        assert!(!after.contains("↓ more"));
        assert!(!main_hints(&app, &geometry).contains(&Hint::new("Down/j", "scroll down")));
        assert!(main_hints(&app, &geometry).contains(&Hint::new("Up/k", "scroll up")));
        assert_eq!(app.keys_scroll, key_help_scroll_max(geometry.area.height));
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &geometry);
        assert_eq!(
            app.keys_scroll,
            key_help_scroll_max(geometry.area.height) - 1
        );
        for _ in 0..50 {
            app.handle_key(
                KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
                &geometry,
            );
        }
        assert_eq!(app.keys_scroll, 0);
        app.handle_key(
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
            &geometry,
        );
        assert_eq!(app.keys_scroll, 1);
        app.handle_key(
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            &geometry,
        );
        app.handle_key(
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            &geometry,
        );
        assert_eq!(app.keys_scroll, 0);
        let large = app.geometry(layout(Rect::new(0, 0, 80, 31)).preview);
        assert_eq!(main_hints(&app, &large), vec![Hint::new("Esc/?", "close")]);
    }

    #[test]
    fn paste_closes_key_overlay_and_opens_the_dropped_image() {
        let mut app = hint_fixture(HintState::Keys);
        app.handle_paste("'/tmp/My Photo.jpg'");
        assert!(!app.keys_open);
        assert_eq!(app.notice(), Some("Loading My Photo.jpg"));
    }

    #[test]
    fn question_mark_stays_text_inside_a_dialog() {
        use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

        let mut app = hint_fixture(HintState::OpenFile);
        let screen = layout(Rect::new(0, 0, 80, 30));
        let before = app
            .modal
            .as_mut()
            .unwrap()
            .text_input_mut()
            .unwrap()
            .text()
            .to_owned();
        let geometry = app.geometry(screen.preview);
        app.handle_key(
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            &geometry,
        );
        assert!(!app.keys_open);
        assert_eq!(
            app.modal.as_mut().unwrap().text_input_mut().unwrap().text(),
            format!("{before}?")
        );
    }

    #[test]
    fn narrow_help_keeps_dialog_escape_and_panel_change() {
        let screen = layout(Rect::new(0, 0, 80, 30));
        for state in [
            HintState::OpenFile,
            HintState::Presets,
            HintState::PresetsConfirm,
            HintState::TemplatesEmpty,
            HintState::Templates,
            HintState::TemplatesPrompt,
            HintState::TemplatesConfirm,
            HintState::EditorColor,
            HintState::EditorMode,
            HintState::EditorInput,
        ] {
            let mut app = hint_fixture(state);
            app.sync(screen.preview);
            let line = hint_line(&hints(&app, &app.geometry(screen.preview)), None, 80).to_string();
            assert!(
                line.contains("Esc") || line.contains("any other key"),
                "{state:?}: {line}"
            );
        }
        let mut app = hint_fixture(HintState::FitPanel);
        app.sync(screen.preview);
        let line = hint_line(&hints(&app, &app.geometry(screen.preview)), None, 80).to_string();
        assert!(line.contains("change"), "{line}");
    }

    #[derive(Default)]
    struct CountingWriter {
        bytes: usize,
        writes: usize,
    }

    impl Write for CountingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes += bytes.len();
            self.writes += 1;
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn panning_terminal_output_stays_within_budget() {
        const AREA: Rect = Rect::new(0, 0, 200, 55);
        const FRAMES: usize = 60;
        // See scripts/profile/results/2026-11-26.md for why bytes per frame bound pan speed.
        let cases = [
            (1, View::Original, 103_000),
            (1, View::Dithered, 56_000),
            (2, View::Original, 40_000),
            (2, View::Dithered, 25_500),
        ];
        let source = rthid_io::image::load_image(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/crates/rthid-dither/tests/fixtures/pratt.png"
        )))
        .unwrap();
        assert_eq!(source.dimensions(), (569, 1024));
        // Override crossterm's process-wide NO_COLOR cache so counts do not depend on the environment.
        Colored::set_ansi_color_disabled(false);

        println!(
            "case                         first bytes   avg bytes/frame   budget   avg writes/frame"
        );
        for (k, view, budget) in cases {
            let zoom = viewport::Zoom::In(k);
            let mut app = App::builtin(source.clone(), None);
            let mut renderer = Renderer::new(None);
            let mut terminal = Terminal::with_options(
                rthid_term::ElidingBackend::new(BufWriter::with_capacity(
                    1024 * 1024,
                    CountingWriter::default(),
                )),
                TerminalOptions {
                    viewport: Viewport::Fixed(AREA),
                },
            )
            .unwrap();
            let layout = layout(AREA);
            app.viewport.set_zoom(zoom, &app.geometry(layout.preview));
            app.view = view;
            app.sync(layout.preview);

            let geometry = app.geometry(layout.preview);
            let start = (
                geometry.target.right() - 2,
                geometry.target.y + geometry.target.height / 2,
            );
            assert!(!app.mouse_at(
                MouseEventKind::Down(MouseButton::Left),
                start.0,
                start.1,
                &geometry,
                layout.screen,
            ));
            let start_x = geometry.origin.0;
            // Frame 0 is the full first draw; the budget covers frames 1..=FRAMES.
            let (mut first_bytes, mut first_writes) = (0, 0);
            for frame in 0..=FRAMES {
                if frame > 0 {
                    let geometry = app.geometry(layout.preview);
                    assert!(app.mouse_at(
                        MouseEventKind::Drag(MouseButton::Left),
                        start.0 - frame as u16 * u16::from(k),
                        start.1,
                        &geometry,
                        layout.screen,
                    ));
                    assert_eq!(
                        app.geometry(layout.preview).origin.0,
                        start_x + frame as u32
                    );
                    app.sync(layout.preview);
                }
                rthid_term::synchronized_draw(&mut terminal, |terminal| {
                    terminal
                        .draw(|frame| draw(frame, &app, &layout, &mut renderer))
                        .map(|_| ())
                })
                .unwrap();
                if frame == 0 {
                    let writer = terminal.backend().writer().get_ref();
                    (first_bytes, first_writes) = (writer.bytes, writer.writes);
                }
            }

            let writer = terminal.backend().writer().get_ref();
            let bytes = writer.bytes - first_bytes;
            let writes = writer.writes - first_writes;
            let average = bytes.div_ceil(FRAMES);
            let name = format!("{} {:?}", view.name(), zoom);
            println!(
                "{name:<28} {first_bytes:>11} {average:>17} {budget:>8} {:>18.2}",
                writes as f64 / FRAMES as f64,
            );
            assert!(
                average <= budget,
                "{name}: measured {average} average bytes/frame exceeds budget {budget}; update the budget table if the increase is intended"
            );
        }
    }

    #[test]
    fn drag_colors_preserve_minimap_dithered_and_exact_cache() {
        let area = Rect::new(0, 0, 48, 18);
        let image = RgbaImage::from_fn(120, 120, |x, y| {
            image::Rgba([x as u8, y as u8, (x + y) as u8, 255])
        });
        let mut app = App::builtin(image, None);
        let mut renderer = Renderer::default();
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        for zoom in [
            viewport::Zoom::In(1),
            viewport::Zoom::Out(2),
            viewport::Zoom::Fit,
        ] {
            for original in [true, false] {
                app.viewport.zoom = viewport::Zoom::In(1);
                app.view = if original {
                    View::Original
                } else {
                    View::Dithered
                };
                app.sync(area);
                let g = app.geometry(area);
                for (kind, column) in [
                    (MouseEventKind::Down(MouseButton::Left), 2),
                    (MouseEventKind::Drag(MouseButton::Left), 1),
                ] {
                    app.mouse_at(kind, column, 10, &g, area);
                }
                assert!(app.dragging());
                app.viewport.zoom = zoom;
                let g = app.geometry(area);
                terminal
                    .draw(|frame| {
                        draw_preview(frame, &app, &g, &mut renderer);
                    })
                    .unwrap();
                let dragging = terminal.backend().buffer().clone();
                let cache_version = renderer.overviews.full.version();
                assert!(app.mouse_at(MouseEventKind::Moved, 1, 10, &g, area));
                terminal
                    .draw(|frame| {
                        draw_preview(frame, &app, &g, &mut renderer);
                    })
                    .unwrap();
                assert_eq!(renderer.overviews.full.version(), cache_version);
                let released = terminal.backend().buffer();
                if !original {
                    assert_eq!(&dragging, released);
                    continue;
                }
                if let Some(map) = g.minimap {
                    for pos in map.frame.positions() {
                        assert_eq!(dragging[pos], released[pos]);
                    }
                }
                let mut converted = 0;
                for pos in g
                    .target
                    .positions()
                    .filter(|&pos| !g.minimap.is_some_and(|map| map.frame.contains(pos)))
                {
                    let mut expected = released[pos].clone();
                    for color in [&mut expected.fg, &mut expected.bg] {
                        if let Color::Rgb(r, g, b) = *color {
                            converted += 1;
                            *color = Color::Indexed(rgb_to_ansi256([r, g, b]));
                        }
                    }
                    assert_eq!(dragging[pos], expected);
                }
                assert!(converted > 0);
            }
        }
    }

    #[test]
    fn original_click_and_drag_end_keep_full_color() {
        let mut app = App::drag_fixture();
        let mut renderer = Renderer::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let layout = layout(terminal.size().unwrap());
        let g = app.geometry(layout.preview);
        assert!(g.overflows());
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Moved,
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.mouse_at(
                kind,
                g.target.x + u16::from(matches!(kind, MouseEventKind::Down(_))),
                g.target.y,
                &g,
                layout.screen,
            );
            draw_text(&mut terminal, &app, &mut renderer);
            for pos in g.target.positions() {
                let cell = &terminal.backend().buffer()[pos];
                assert_eq!(cell.symbol(), " ");
                assert_eq!(cell.fg, Color::Reset);
                assert!(match kind {
                    MouseEventKind::Drag(_) => matches!(cell.bg, Color::Indexed(_)),
                    _ => matches!(cell.bg, Color::Rgb(..)),
                });
            }
        }
    }

    #[test]
    fn empty_preview_draws_hint_and_logs_only_header() {
        let mut app = App::empty();
        let mut renderer = Renderer::default();
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert!(text.contains("No image"));
        assert!(text.contains("o: open file | drop an image file here"));
        let geometry = app.geometry(layout(terminal.size().unwrap()).preview);
        assert_eq!(
            status(&app, &geometry, PreviewReport::default()),
            "No image"
        );
        let frame = terminal
            .draw(|frame| draw(frame, &app, &layout(frame.area()), &mut renderer))
            .unwrap();
        let mut log = Vec::new();
        log_minimap(&app, &mut log, &frame, &geometry).unwrap();
        let log = String::from_utf8(log).unwrap();
        assert!(log.starts_with("[minimap]"));
        assert_eq!(log.lines().count(), 1);
        app.set_notice("Config notice".into());
        assert_eq!(
            status(&app, &geometry, PreviewReport::default()),
            "Config notice"
        );
    }

    #[test]
    fn modals_draw_text_swatches_and_cursor_without_terminal_images() {
        use crate::{
            modal::Editor,
            settings::{MAX_NAME_CHARS, Preset, Settings},
        };
        use ratatui_image::picker::ProtocolType;
        use rthid_widgets::{ListPicker, TextInput};
        for protocol in [
            ProtocolType::Kitty,
            ProtocolType::Iterm2,
            ProtocolType::Sixel,
        ] {
            let mut picker = ratatui_image::picker::Picker::halfblocks();
            picker.set_protocol_type(protocol);
            let mut renderer = Renderer::new(Some(graphics::Graphics::new(picker)));
            let mut app = App::builtin(RgbaImage::new(12, 12), None);
            let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
            draw_text(&mut terminal, &app, &mut renderer);
            app.modal = Some(Modal::presets(&app.settings, &app.library.palettes));
            let text = draw_text(&mut terminal, &app, &mut renderer);
            assert!(text.contains("(built-in)") && text.contains("Presets"));
            assert!(!text.contains(&format!(
                "Original ({})",
                renderer.graphics.as_ref().unwrap().name()
            )));
            app.modal = Some(Modal::Editor(Box::new(Editor::new(
                &app.modal_ctx(),
                Preset {
                    name: "Paper".into(),
                    colors: vec!["#000".into(), "#fff".into()],
                    mode: None,
                    builtin: false,
                },
            ))));
            let text = draw_text(&mut terminal, &app, &mut renderer);
            assert!(text.contains("Preset editor") && text.contains("Mode: keep"));
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .any(|cell| cell.symbol() == "█" && cell.fg == Color::Rgb(0, 0, 0))
            );
            if let Some(Modal::Editor(editor)) = &mut app.modal {
                editor.rows = ListPicker::new(1);
                editor.input = Some(TextInput::new("oops", 7));
            }
            draw_text(&mut terminal, &app, &mut renderer);
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .any(|cell| cell.symbol() == "o" && cell.fg == Color::Red)
            );
            let layout = layout(terminal.size().unwrap());
            let inner = app
                .modal
                .as_ref()
                .unwrap()
                .layout(layout.preview, layout.screen, &app.modal_ctx())
                .list
                .unwrap();
            assert_eq!(
                terminal.get_cursor_position().unwrap(),
                (inner.x + 2 + 3 + 4, inner.y + 1).into()
            );
            for (width, height) in [(0, 0), (1, 1), (20, 5), (60, 12)] {
                let mut tiny = Terminal::new(TestBackend::new(width, height)).unwrap();
                draw_text(&mut tiny, &app, &mut renderer);
            }
            app.modal = Some(Modal::Templates {
                list: ListPicker::new(0),
                prompt: Some(TextInput::new("Z", MAX_NAME_CHARS)),
                confirm: None,
            });
            assert!(draw_text(&mut terminal, &app, &mut renderer).contains("Name: Z"));
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .area
                    .contains(terminal.get_cursor_position().unwrap())
            );
            app.modal = Some(Modal::templates());
            app.library
                .templates
                .upsert(crate::settings::Template::capture(
                    "Z".into(),
                    &Settings::default(),
                ))
                .unwrap();
            assert!(
                draw_text(&mut terminal, &app, &mut renderer)
                    .contains("Z | Black/white | tone | Atkinson | Image 0.20 mm")
            );
            let mut print = crate::settings::Template::capture("P".into(), &Settings::default());
            (print.card, print.dot, print.margin, print.bleed) =
                (crate::settings::Card::from_id("postcard"), 12, 15, 3);
            app.library.templates.upsert(print).unwrap();
            assert!(
                template_rows(&app.modal_ctx()).iter().any(|row| row.to_string()
                    == "P | Black/white | tone | Atkinson | Postcard 100x148 0.12 mm +1.5 mm margin +3 mm bleed")
            );
            let mut template = crate::settings::Template::capture(
                "A template with a long name".into(),
                &Settings::default(),
            );
            template.palette_name = "A palette with a long name".into();
            app.library.templates.upsert(template).unwrap();
            assert!(
                usize::from(
                    app.modal
                        .as_ref()
                        .unwrap()
                        .layout(layout.preview, layout.screen, &app.modal_ctx())
                        .list
                        .unwrap()
                        .width
                ) >= Line::raw("A template with a long name | A palette with a long name | tone")
                    .width()
            );
        }
    }

    #[test]
    fn editor_field_tracks_selected_row_after_resize() {
        use crate::{modal::Editor, settings::Preset};

        let mut app = App::builtin(RgbaImage::new(12, 12), None);
        let mut editor = Editor::new(
            &app.modal_ctx(),
            Preset {
                name: "Paper".into(),
                colors: vec!["#000".into(); 8],
                mode: None,
                builtin: false,
            },
        );
        editor.rows.select(8);
        editor.input = Some(TextInput::new("oops", 7));
        app.modal = Some(Modal::Editor(Box::new(editor)));
        let mut renderer = Renderer::new(None);
        for height in [24, 7, 24] {
            let mut terminal = Terminal::new(TestBackend::new(60, height)).unwrap();
            draw_text(&mut terminal, &app, &mut renderer);
            let modal = app.modal.as_ref().unwrap();
            let screen = layout(terminal.size().unwrap());
            let field = modal
                .layout(screen.preview, screen.screen, &app.modal_ctx())
                .field
                .unwrap();
            assert_eq!(terminal.get_cursor_position().unwrap().y, field.y);
            assert!(
                terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .any(|cell| cell.symbol() == "o" && cell.fg == Color::Red)
            );
        }
    }

    #[test]
    fn status_excludes_partial_edge_dots_until_panned() {
        let mut app = App::builtin(RgbaImage::new(16, 16), None);
        for (zoom, area, initial, panned) in [
            // 47 columns at 3x clip the sixteenth column and 31 rows at 4x clip the sixteenth row.
            (
                viewport::Zoom::In(3),
                Rect::new(0, 0, 47, 30),
                "300% x 1-15 y 1-16",
                "300% x 2-16 y 1-16",
            ),
            (
                viewport::Zoom::In(4),
                Rect::new(0, 0, 64, 31),
                "400% x 1-16 y 1-15",
                "400% x 1-16 y 2-16",
            ),
        ] {
            app.viewport = viewport::Viewport {
                zoom,
                ..viewport::Viewport::default()
            };
            let before = viewport::geometry(&app.viewport, (16, 16), area);
            assert!(status(&app, &before, PreviewReport::default()).contains(initial));
            app.viewport.pan(i64::MAX, i64::MAX, &before);
            let after = viewport::geometry(&app.viewport, (16, 16), area);
            assert!(status(&app, &after, PreviewReport::default()).contains(panned));
        }
    }

    #[test]
    fn status_uses_preview_report_and_prioritizes_notices() {
        let mut app = App::builtin(RgbaImage::new(4, 6), None);
        let geometry = app.geometry(Rect::new(1, 1, 20, 10));
        assert!(
            status(
                &app,
                &geometry,
                PreviewReport {
                    approximate: true,
                    ..PreviewReport::default()
                },
            )
            .ends_with(" | Original | Fit (sextant) approximate")
        );
        assert!(
            status(
                &app,
                &geometry,
                PreviewReport {
                    graphics_name: Some("kitty"),
                    ..PreviewReport::default()
                },
            )
            .ends_with(" | Original (kitty) | Fit (sextant)")
        );
        let dir = tempfile::tempdir().unwrap();
        for with_export in [false, true] {
            if with_export {
                app = App::builtin(RgbaImage::new(4, 6), Some(dir.path().to_path_buf()));
                app.sync(layout(Rect::new(0, 0, 80, 24)).preview);
                app.complete(rthid_io::io::Done::new(
                    0,
                    std::time::Duration::ZERO,
                    rthid_io::io::Outcome::Exported(Err(rthid_io::io::IoError {
                        op: rthid_io::io::Op::Encode,
                        path: dir.path().to_path_buf(),
                        cause: rthid_io::io::Cause::Io(std::io::Error::other("denied")),
                    })),
                ));
                assert!(app.export_result().is_err());
            }
            app.set_notice("Config notice".into());
            assert_eq!(
                status(
                    &app,
                    &geometry,
                    PreviewReport {
                        graphics_error: Some("Graphics failed".to_owned()),
                        ..PreviewReport::default()
                    },
                ),
                "Config notice",
            );
        }
    }

    #[test]
    fn layout_preserves_regions_and_preview_border() {
        for (screen, expected) in [
            (
                Rect::new(0, 0, 100, 40),
                Layout {
                    screen: Rect::new(0, 0, 100, 40),
                    preview_block: Rect::new(0, 0, 71, 38),
                    preview: Rect::new(1, 1, 69, 36),
                    panel: Rect::new(71, 0, 29, 38),
                    help: Rect::new(0, 38, 100, 1),
                    status: Rect::new(0, 39, 100, 1),
                },
            ),
            (
                Rect::new(2, 3, 41, 9),
                Layout {
                    screen: Rect::new(2, 3, 41, 9),
                    preview_block: Rect::new(2, 3, 12, 7),
                    preview: Rect::new(3, 4, 10, 5),
                    panel: Rect::new(14, 3, 29, 7),
                    help: Rect::new(2, 10, 41, 1),
                    status: Rect::new(2, 11, 41, 1),
                },
            ),
            (
                Rect::new(0, 0, 20, 5),
                Layout {
                    screen: Rect::new(0, 0, 20, 5),
                    preview_block: Rect::new(0, 0, 0, 3),
                    preview: Rect::new(0, 1, 0, 1),
                    panel: Rect::new(0, 0, 20, 3),
                    help: Rect::new(0, 3, 20, 1),
                    status: Rect::new(0, 4, 20, 1),
                },
            ),
            (
                Rect::default(),
                Layout {
                    screen: Rect::default(),
                    preview_block: Rect::default(),
                    preview: Rect::default(),
                    panel: Rect::default(),
                    help: Rect::default(),
                    status: Rect::default(),
                },
            ),
        ] {
            assert_eq!(layout(screen), expected, "{screen:?}");
        }
    }

    #[test]
    fn minimap_log_records_completed_cells_and_uncovered_reference() {
        let mut renderer = Renderer::default();
        let mut app = App::builtin(RgbaImage::from_pixel(180, 180, image::Rgba([255; 4])), None);
        app.settings = crate::settings::Settings::for_pixels(180, 180);
        app.mass_tones = Some(
            [rthid_dither::dither::PaletteColor {
                rgb: [80, 120, 160],
            }; 4],
        );
        app.viewport.zoom = viewport::Zoom::In(1);
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut log = Vec::new();
        let area = layout(terminal.size().unwrap()).preview;
        app.sync(area);
        let geometry = app.geometry(area);
        assert!(matches!(
            app.inset(),
            Some(cells::Pixels::Rgb {
                view: View::Mass,
                ..
            })
        ));
        let report = PreviewReport::default;
        assert!(status(&app, &geometry, report()).ends_with(" | map Mass"));
        app.view = View::Mass;
        assert!(!status(&app, &geometry, report()).contains("map Mass"));
        app.view = View::Original;
        app.mass_minimap = false;
        assert!(!status(&app, &geometry, report()).contains("map Mass"));
        app.mass_minimap = true;
        let frame = terminal
            .draw(|f| {
                draw(f, &app, &layout(f.area()), &mut renderer);
            })
            .unwrap();
        log_minimap(&app, &mut log, &frame, &geometry).unwrap();
        let text = String::from_utf8(log).unwrap();
        assert!(text.starts_with("[minimap] frame=0 grid=180x180"));
        let viewport::Minimap {
            frame: inset, map, ..
        } = geometry.minimap.unwrap();
        assert_eq!(text.lines().count(), 2 + inset.area() as usize);
        let corner = text
            .lines()
            .find(|line| line.starts_with(&format!("{},{} ", map.x, map.y)))
            .unwrap();
        let (displayed, overview) = corner.split_once(" overview=").unwrap();
        for cell in [displayed, overview] {
            assert!(cell.contains("fg: Reset"));
            assert!(cell.contains("bg: Rgb(80, 120, 160)"));
        }
        assert_eq!(frame.buffer[(map.x, map.y)].symbol(), " ");
        let outside = text
            .lines()
            .find(|line| line.starts_with(&format!("{},{} ", map.right() - 1, map.bottom() - 1)))
            .unwrap();
        let (displayed, overview) = outside.split_once(" overview=").unwrap();
        assert!(displayed.contains("fg: Reset"));
        assert!(displayed.contains("bg: Rgb(40, 60, 80)"));
        assert!(overview.contains("fg: Reset"));
        assert!(overview.contains("bg: Rgb(80, 120, 160)"));

        assert!(
            log_minimap(
                &app,
                &mut std::io::Cursor::new(&mut [0u8; 1][..]),
                &frame,
                &geometry,
            )
            .is_err()
        );
    }

    #[test]
    fn draws_settings_and_truecolor_backgrounds() {
        let mut renderer = Renderer::default();
        let mut app = App::builtin(RgbaImage::from_pixel(3, 3, image::Rgba([255; 4])), None);
        app.settings.card = BUSINESS_CARD;
        app.sync(Rect::new(0, 0, 180, 30));
        assert_eq!(app.view, View::Original);
        app.viewport.zoom = viewport::Zoom::In(1);
        let mut terminal = Terminal::new(TestBackend::new(180, 30)).unwrap();
        let text = draw_text(&mut terminal, &app, &mut renderer);
        let buffer = terminal.backend().buffer();
        let panel = layout(terminal.size().unwrap()).panel;
        assert_eq!(buffer[(panel.x + 1, panel.y + 1)].symbol(), "▾");
        assert_eq!(buffer[(panel.x + 1, panel.y + 2)].symbol(), ">");
        assert_eq!(buffer[(panel.x + 3, panel.y + 2)].symbol(), "C");
        for label in [
            "Card",
            "Dot:",
            "Margin:",
            "Bleed:",
            "Zoom:",
            "Pan X:",
            "Pan Y:",
            "Mode",
            "Palette",
            "Strength",
            "Serpentine",
            "Brightness",
            "Contrast",
            "Gamma: 1.00",
            "Black: 0.00",
            "White: 1.00",
            "Sharpen: 0.00",
            "Card 50x90mm | Dots 250x450 @0.20mm | Rot 0° | Original | 100%",
        ] {
            assert!(text.contains(label), "missing {label}");
        }
        let cell = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == " " && cell.bg == Color::Rgb(255, 255, 255))
            .unwrap();
        assert_eq!(cell.fg, Color::Reset);
        assert_eq!(cell.bg, Color::Rgb(255, 255, 255));
        // Tiny terminals must not panic.
        for (width, height) in [(1, 1), (20, 5), (60, 12)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    draw(frame, &app, &layout(frame.area()), &mut renderer);
                })
                .unwrap();
        }
    }

    #[test]
    fn collapsed_group_shows_changed_count_and_hides_items() {
        let mut renderer = Renderer::default();
        let mut app = App::empty();
        app.settings.tone.brightness = 0.1;
        app.settings.tone.contrast = 0.1;
        app.panel.select(Row::Header(2), &app.panel_lens());
        app.panel.handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &app.panel_lens(),
        );
        app.panel.select(Row::Item(0), &app.panel_lens());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert!(text.contains("▸ Tone  2 changed"), "{text}");
        assert!(text.contains("> Card: Image"), "{text}");
        assert!(!text.contains("Brightness:"), "{text}");
    }

    #[test]
    fn collapsed_group_header_click_expands_after_draw() {
        let mut renderer = Renderer::default();
        let mut app = App::empty();
        for group in [BRUSH_GROUP, 0] {
            app.panel.select(Row::Header(group), &app.panel_lens());
            app.panel.handle_key(
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                &app.panel_lens(),
            );
            assert!(app.panel.is_collapsed(group));
        }
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        draw_text(&mut terminal, &app, &mut renderer);
        let screen = terminal.size().unwrap().into();
        let layout = layout(screen);
        app.handle_mouse(
            ratatui::crossterm::event::MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: layout.panel.x + 2,
                row: layout.panel.y + 1,
                modifiers: KeyModifiers::NONE,
            },
            &app.geometry(layout.preview),
            screen,
        );
        assert!(!app.panel.is_collapsed(0));
    }
    #[test]
    fn open_dialog_keeps_path_above_scrolled_entries() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use rthid_io::dir::Entry;
        let mut app = App::empty();
        app.modal = Some(Modal::open_file("/images".into(), 1));
        app.set_listing(
            "/images".into(),
            (0..30)
                .map(|i| Entry {
                    file_name: format!("folder{i:02}_with_a_very_long_name").into(),
                    name: format!("folder{i:02}_with_a_very_long_name"),
                    dir: true,
                })
                .collect(),
        );
        let mut renderer = Renderer::default();
        for (width, height) in [(60, 12), (1, 1), (20, 5)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let layout = layout(terminal.size().unwrap());
            for _ in 0..20 {
                app.handle_key(
                    KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                    &app.geometry(layout.preview),
                );
            }
            terminal
                .draw(|frame| draw(frame, &app, &layout, &mut renderer))
                .unwrap();
            if width == 60 {
                let Some(Modal::OpenFile(picker)) = &app.modal else {
                    unreachable!()
                };
                assert!(picker.list.offset() > 0);
                let cursor = terminal.get_cursor_position().unwrap();
                let buffer = terminal.backend().buffer();
                let text: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
                assert!(text.contains("Open image"));
                assert!(text.contains("/images"));
                assert!(text.contains("folder20_with_a_very_long_name/"));
                assert!(cursor.y < buffer.area.height);
                assert_eq!(buffer[(cursor.x - 1, cursor.y)].symbol(), "/");
            }
        }
    }

    #[test]
    fn setting_list_keeps_scroll_offset_when_moving_within_visible_rows() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut renderer = Renderer::default();
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        let mut terminal = Terminal::new(TestBackend::new(60, 7)).unwrap();
        for _ in 0..15 {
            app.handle_key(
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                &app.geometry(layout(terminal.size().unwrap()).preview),
            );
            terminal
                .draw(|frame| {
                    draw(frame, &app, &layout(frame.area()), &mut renderer);
                })
                .unwrap();
        }
        assert_eq!(app.panel.selected(&app.panel_lens()), Some(Row::Item(14)));
        let panel = layout(terminal.size().unwrap()).panel;
        let first_panel_row = |terminal: &Terminal<TestBackend>| {
            (panel.x + 1..panel.right() - 1)
                .map(|x| terminal.backend().buffer()[(x, panel.y + 1)].symbol())
                .collect::<String>()
                .trim()
                .to_owned()
        };
        assert_eq!(first_panel_row(&terminal), "▾ Tone");
        app.handle_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &app.geometry(layout(terminal.size().unwrap()).preview),
        );
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert_eq!(app.panel.selected(&app.panel_lens()), Some(Row::Item(13)));
        assert_eq!(first_panel_row(&terminal), "▾ Tone");
        for label in ["Tone", "Brightness", "Contrast"] {
            assert!(text.contains(label), "missing {label}: {text}");
        }
    }

    #[test]
    fn overview_and_exact_modes_show_their_status() {
        let mut renderer = Renderer::default();
        let mut app = App::builtin(RgbaImage::new(8, 12), None);
        app.settings = crate::settings::Settings::for_pixels(8, 12);
        app.view = View::Dithered;
        app.sync(layout(Rect::new(0, 0, 100, 24)).preview);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert!(text.contains("Dithered | Fit (sextant)"));
        app.glyphs = cells::Glyphs::Quadrant;
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert!(text.contains("Fit (quadrant)"));
        app.viewport.zoom = viewport::Zoom::In(1);
        app.preview_focus = true;
        let text = draw_text(&mut terminal, &app, &mut renderer);
        let buffer = terminal.backend().buffer();
        assert!(text.contains("100% x 1-8 y 1-12"));
        assert_eq!(buffer[(0, 0)].fg, Color::Cyan);
        app.view = View::Mass;
        app.sync(layout(Rect::new(0, 0, 100, 24)).preview);
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert!(text.contains("Mass | 100%"));
    }
    #[test]
    fn leaving_graphics_repaints_the_previous_image_in_one_draw() {
        use ratatui::buffer::CellDiffOption;
        use ratatui_image::picker::ProtocolType;
        for (view, zoom, brush_on) in [
            (View::Dithered, viewport::Zoom::Fit, false),
            (View::Original, viewport::Zoom::In(1), false),
            (View::Original, viewport::Zoom::Fit, true),
        ] {
            for protocol in [
                ProtocolType::Kitty,
                ProtocolType::Iterm2,
                ProtocolType::Sixel,
            ] {
                let mut app = App::builtin(RgbaImage::new(12, 12), None);
                let mut picker = ratatui_image::picker::Picker::halfblocks();
                picker.set_protocol_type(protocol);
                let font = picker.font_size();
                let geometry = app.geometry(layout(ratatui::layout::Size::new(100, 40)).preview);
                let (cw, ch) = viewport::fit_aspect(
                    geometry.grid.0,
                    geometry.grid.1,
                    geometry.area,
                    f64::from(font.height) / f64::from(font.width),
                );
                let old = geometry
                    .area
                    .centered(Constraint::Length(cw), Constraint::Length(ch));
                let mut renderer = Renderer::new(Some(graphics::Graphics::new(picker)));
                let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
                terminal
                    .draw(|frame| {
                        crate::ui::draw(frame, &app, &layout(frame.area()), &mut renderer);
                    })
                    .unwrap();
                app.view = view;
                app.viewport.zoom = zoom;
                app.brush_on = brush_on;
                app.sync(layout(terminal.size().unwrap()).preview);
                let completed = terminal
                    .draw(|frame| {
                        crate::ui::draw(frame, &app, &layout(frame.area()), &mut renderer);
                    })
                    .unwrap();
                for pos in completed.buffer.area.positions() {
                    assert_eq!(
                        completed.buffer[pos].diff_option == CellDiffOption::AlwaysUpdate,
                        protocol != ProtocolType::Kitty && old.contains(pos),
                        "{protocol:?} at {pos:?}",
                    );
                }
                let mut glyphs = crate::ui::Renderer::new(None);
                let mut baseline = Terminal::new(TestBackend::new(100, 40)).unwrap();
                baseline
                    .draw(|frame| {
                        crate::ui::draw(frame, &app, &layout(frame.area()), &mut glyphs);
                    })
                    .unwrap();
                for pos in completed.buffer.area.positions() {
                    let mut actual = completed.buffer[pos].clone();
                    actual.set_diff_option(CellDiffOption::None);
                    assert_eq!(actual, baseline.backend().buffer()[pos]);
                }
                let text: String = completed
                    .buffer
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(!text.contains(&format!(
                    "Original ({})",
                    renderer.graphics.as_ref().unwrap().name()
                )));
            }
        }
    }
    #[test]
    fn graphics_are_only_drawn_for_original_at_fit() {
        use ratatui_image::picker::ProtocolType;
        for protocol in [
            ProtocolType::Kitty,
            ProtocolType::Iterm2,
            ProtocolType::Sixel,
        ] {
            let mut picker = ratatui_image::picker::Picker::halfblocks();
            picker.set_protocol_type(protocol);
            let mut renderer = Renderer::new(Some(graphics::Graphics::new(picker)));
            let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
            let mut app = App::builtin(RgbaImage::new(400, 400), None);
            for view in [View::Original, View::Dithered, View::Mass] {
                app.view = view;
                app.sync(layout(terminal.size().unwrap()).preview);
                for zoom in [
                    viewport::Zoom::Fit,
                    viewport::Zoom::Out(4),
                    viewport::Zoom::Out(3),
                    viewport::Zoom::Out(2),
                    viewport::Zoom::In(1),
                    viewport::Zoom::In(2),
                    viewport::Zoom::In(3),
                    viewport::Zoom::In(4),
                ] {
                    app.viewport.zoom = zoom;
                    for brush_on in [false, true, false] {
                        app.brush_on = brush_on;
                        terminal
                            .draw(|frame| {
                                let g = app.geometry(layout(frame.area()).preview);
                                let report = draw_preview(frame, &app, &g, &mut renderer);
                                assert_eq!(
                                    report.graphics_name.is_some(),
                                    !brush_on
                                        && view == View::Original
                                        && zoom == viewport::Zoom::Fit
                                );
                            })
                            .unwrap();
                    }
                }
            }
        }
    }
    #[test]
    fn image_status_uses_resolved_physical_size() {
        let mut app = App::builtin(RgbaImage::new(30, 20), None);
        let text = |app: &App| {
            status(
                app,
                &app.geometry(Rect::new(0, 0, 100, 20)),
                PreviewReport::default(),
            )
        };
        assert!(text(&app).starts_with("Card 6x4mm | Dots 30x20 @0.20mm"));
        app.settings.dot = 40;
        app.settings.rotation = crate::settings::Rotation::R90;
        assert!(text(&app).starts_with("Card 8x12mm | Dots 20x30 @0.40mm"));
        app.settings.margin = 20;
        app.settings.bleed = 3;
        assert!(text(&app).starts_with("Card 18.4x22.4mm | Dots 46x56 @0.40mm"));
        assert_eq!(
            status(
                &App::empty(),
                &app.geometry(Rect::default()),
                PreviewReport::default()
            ),
            "No image"
        );
    }

    #[test]
    fn print_status_includes_bleed_and_empty_area_and_truncates() {
        let mut app = App::builtin(RgbaImage::new(8, 8), None);
        app.settings.card = BUSINESS_CARD;
        app.sync(Rect::new(0, 0, 180, 30));
        app.settings.bleed = 3;
        app.settings.rotation = crate::settings::Rotation::R90;
        app.settings.flip = true;
        let geometry = app.geometry(Rect::new(0, 0, 100, 20));
        let text = status(&app, &geometry, PreviewReport::default());
        assert!(text.starts_with("Card 50x90+3mm | Dots 280x480 @0.20mm | Rot 90° Flip | "));
        assert!(!text.contains("PNG"));
        let with_dpi = |dpi| {
            let mut exporting = App::new(
                Some(("test.png".into(), RgbaImage::new(8, 8))),
                Some("p.png".into()),
                None,
                Some(dpi),
                crate::store::Loaded::default(),
            );
            exporting.settings = app.settings.clone();
            exporting
        };
        assert!(
            status(&with_dpi(600), &geometry, PreviewReport::default())
                .contains("Rot 90° Flip | PNG 1323x2268 @600dpi | ")
        );
        // Worst case: the widest PNG size and DPI still fit in 80 columns.
        let report = PreviewReport {
            approximate: true,
            ..PreviewReport::default()
        };
        let text = status(&with_dpi(4800), &geometry, report);
        let print =
            "Card 50x90+3mm | Dots 280x480 @0.20mm | Rot 90° Flip | PNG 10583x18142 @4800dpi";
        assert!(text.starts_with(print), "{text}");
        assert!(print.chars().count() <= 80);
        app.settings = crate::settings::Settings {
            margin: 100,
            ..crate::settings::Settings::for_pixels(8, 8)
        };
        app.sync(Rect::new(0, 0, 100, 20));
        let geometry = app.geometry(Rect::new(0, 0, 100, 20));
        let text = status(&app, &geometry, PreviewReport::default());
        assert!(text.contains("no image area"));
        assert!(!text.contains("bleed"));
        assert!(!text.contains("Flip"));
        let mut renderer = Renderer::default();
        let mut terminal = Terminal::new(TestBackend::new(20, 7)).unwrap();
        let _ = draw_text(&mut terminal, &app, &mut renderer);
        let buffer = terminal.backend().buffer();
        let row: String = (0..20).map(|x| buffer[(x, 6)].symbol()).collect();
        assert_eq!(row, text.chars().take(20).collect::<String>());
    }
}
