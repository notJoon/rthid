//! Screen layout and per frame drawing of the preview, parameter panel, help line
//! and status line.
//!
//! Drawing borrows App immutably. Renderer retains preview caches and terminal
//! graphics while the panel retains its own scroll offset.

use crate::{
    cells, graphics,
    modal::{Modal, preset_rows, template_rows},
    viewport,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout as Split, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use rthid_widgets::{Dialog, ListPicker, TextInput, key_hints};

use crate::{app::App, settings::Setting};

// Keys that open a modal, shown first on the main screen.
const MODAL_HINTS: &[(&str, &str)] = &[("o", "open"), ("P", "presets"), ("T", "templates")];

const PREVIEW_HINTS: &[(&str, &str)] = &[
    ("Tab", "focus"),
    ("Space", "original/dithered"),
    ("+/-", "zoom"),
    ("0", "fit"),
    ("hjkl/arrows", "pan"),
    ("HJKL", "page"),
    ("m", "map"),
    ("mouse", "drag/wheel/map"),
    ("r/R", "rotate"),
    ("f", "flip"),
    ("d/D", "reset"),
    ("Esc", "back"),
    ("q", "quit"),
];

const PANEL_HINTS: &[(&str, &str)] = &[
    ("Tab", "focus"),
    ("Space", "original/dithered"),
    ("+/-", "zoom"),
    ("0", "fit"),
    ("m", "map"),
    ("mouse", "drag/wheel/map"),
    ("Up/Down k/j", "select"),
    ("Left/Right h/l", "change"),
    ("r/R", "rotate"),
    ("f", "flip"),
    ("d/D", "reset"),
    ("q", "quit"),
];

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
    let Some(pixels) = app.displayed() else {
        return log.flush();
    };
    let mut overview = ratatui::buffer::Buffer::empty(geometry.area);
    if let Some(viewport::Minimap {
        frame: inset,
        map,
        viewport,
    }) = geometry.minimap
    {
        let area = Block::bordered().inner(inset);
        cells::render(
            pixels,
            app.glyphs,
            &viewport::geometry(&viewport::Viewport::default(), geometry.grid, area),
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
    let labels = Setting::ALL.map(|setting| {
        let label = Line::raw(setting.label(&app.settings, &app.library.palettes));
        if setting.is_active(&app.settings) {
            label
        } else {
            label.style(Style::default().fg(Color::DarkGray))
        }
    });
    let block = Block::bordered()
        .title("Parameters")
        .border_style(focus_style(!app.preview_focus));
    let highlight = Style::default().fg(Color::Black).bg(Color::Cyan);
    app.panel.render(
        labels,
        Some(block),
        highlight,
        layout.panel,
        frame.buffer_mut(),
    );
    frame.render_widget(
        Paragraph::new(if let Some(modal) = &app.modal {
            key_hints(&[modal.hints()])
        } else {
            key_hints(&[
                MODAL_HINTS,
                if app.preview_focus {
                    PREVIEW_HINTS
                } else {
                    PANEL_HINTS
                },
            ])
        }),
        layout.help,
    );
    frame.render_widget(
        Paragraph::new(status(app, &geometry, report)),
        layout.status,
    );
    if app.modal.is_some() {
        draw_modal(frame, app, layout);
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

fn swatch_span([r, g, b]: [u8; 3], style: Style) -> Span<'static> {
    Span::styled("  ██", style.fg(Color::Rgb(r, g, b)))
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
            let mut rows = Vec::new();
            for row in 0..=editor.draft.colors.len() {
                let prefix = if row == 0 {
                    "Name: ".into()
                } else {
                    format!("{row}: ")
                };
                let text = if let Some(input) = &editor.input
                    && editor.rows.selected(editor.draft.colors.len() + 2) == Some(row)
                {
                    editing = Some(Editing {
                        input,
                        prefix: prefix.clone(),
                        invalid: editor.invalid_hex(),
                        swatch: editor.swatch_rgb(row),
                    });
                    input.text()
                } else if row == 0 {
                    &editor.draft.name
                } else {
                    &editor.draft.colors[row - 1]
                };
                let mut spans = vec![Span::raw(prefix), Span::raw(text.to_owned())];
                if let Some(rgb) = editor.swatch_rgb(row) {
                    spans.push(swatch_span(rgb, Style::default()));
                }
                rows.push(Line::from(spans));
            }
            rows.push(Line::raw(format!(
                "Mode: {}",
                editor
                    .draft
                    .mode
                    .map_or("keep", crate::settings::mode_label)
            )));
            ModalContent {
                title: "Preset editor",
                rows,
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

// Uses terminal graphics only for Original at Fit and cell glyphs otherwise.
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
        && renderer.graphics.is_some()
        && app.show_original
        && geometry.zoom == viewport::Zoom::Fit
        && !geometry.area.is_empty();
    let approximate = if image_mode {
        false
    } else {
        let approximate = cells::render(
            pixels,
            app.glyphs,
            geometry,
            frame.buffer_mut(),
            &mut renderer.overviews,
        );
        if app.dragging() && matches!(pixels, cells::Pixels::Rgb(..)) {
            for pos in geometry.target.positions() {
                if geometry.minimap.is_some_and(|map| map.frame.contains(pos)) {
                    continue;
                }
                let cell = &mut frame.buffer_mut()[pos];
                for color in [&mut cell.fg, &mut cell.bg] {
                    if let Color::Rgb(r, g, b) = *color {
                        *color = Color::Indexed(rgb_to_ansi256([r, g, b]));
                    }
                }
            }
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
    let displayed = if let Some(name) = report.graphics_name {
        format!("Original ({name})")
    } else if app.show_original {
        "Original".into()
    } else {
        "Dithered".into()
    };
    report.graphics_error.unwrap_or_else(|| {
        format!(
            "{displayed} | {mode} | Card {}x{} mm{} | Dots {gw}x{gh} | Dot {:.2} mm | Rot {}°{}{}{}",
            app.settings.card.w_mm,
            app.settings.card.h_mm,
            if app.settings.bleed > 0 {
                format!(" (+{} mm bleed)", app.settings.bleed)
            } else {
                String::new()
            },
            f64::from(app.settings.dot) / 100.0,
            app.settings.rotation.degrees(),
            if app.settings.flip { " Flip" } else { "" },
            if placement.aw == 0 {
                " | no image area"
            } else {
                ""
            },
            app.dpi
                .and_then(|dpi| crate::print::png_size(&app.settings, dpi).ok().map(|(w, h)| {
                    format!(" | PNG {w}x{h} @{dpi}dpi")
                }))
                .unwrap_or_default(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;
    use ratatui::{Terminal, backend::TestBackend};

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

    #[test]
    fn drag_colors_preserve_minimap_dithered_and_exact_cache() {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
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
                app.show_original = original;
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
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
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
                draw_text(&mut terminal, &app, &mut renderer).contains("Z | Black/white | tone")
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
            .starts_with("Original | Fit (sextant) approximate |")
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
            .starts_with("Original (kitty) | Fit (sextant) |")
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
        app.sync(layout(Rect::new(0, 0, 100, 40)).preview);
        app.viewport.zoom = viewport::Zoom::In(1);
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut log = Vec::new();
        let geometry = app.geometry(layout(terminal.size().unwrap()).preview);
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
            assert!(cell.contains("bg: Rgb(255, 255, 255)"));
        }
        assert_eq!(frame.buffer[(map.x, map.y)].symbol(), " ");
        let outside = text
            .lines()
            .find(|line| line.starts_with(&format!("{},{} ", map.right() - 1, map.bottom() - 1)))
            .unwrap();
        let (displayed, overview) = outside.split_once(" overview=").unwrap();
        assert!(displayed.contains("fg: Reset"));
        assert!(displayed.contains("bg: Rgb(127, 127, 127)"));
        assert!(overview.contains("fg: Reset"));
        assert!(overview.contains("bg: Rgb(255, 255, 255)"));

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
        assert!(app.show_original);
        app.viewport.zoom = viewport::Zoom::In(1);
        let mut terminal = Terminal::new(TestBackend::new(180, 30)).unwrap();
        let text = draw_text(&mut terminal, &app, &mut renderer);
        let buffer = terminal.backend().buffer();
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
            "Original | 100%",
            "Card 50x90 mm",
            "Dots 250x450",
            "Dot 0.20 mm",
            "Rot 0",
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
    fn open_dialog_keeps_path_above_scrolled_entries() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use rthid_io::dir::Entry;
        let mut app = App::empty();
        app.modal = Some(Modal::open_file("/images".into(), 1));
        app.set_listing(
            "/images".into(),
            (0..30)
                .map(|i| Entry {
                    file_name: format!("folder{i:02}").into(),
                    name: format!("folder{i:02}"),
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
                assert!(text.contains("folder20/"));
                assert!(cursor.y < buffer.area.height);
                assert_eq!(buffer[(cursor.x - 1, cursor.y)].symbol(), "/");
            }
        }
    }

    #[test]
    fn help_hints_preserve_exact_text() {
        assert_eq!(
            key_hints(&[MODAL_HINTS, PREVIEW_HINTS]),
            "o: open | P: presets | T: templates | Tab: focus | Space: original/dithered | +/-: zoom | 0: fit | hjkl/arrows: pan | HJKL: page | m: map | mouse: drag/wheel/map | r/R: rotate | f: flip | d/D: reset | Esc: back | q: quit"
        );
        assert_eq!(
            key_hints(&[MODAL_HINTS, PANEL_HINTS]),
            "o: open | P: presets | T: templates | Tab: focus | Space: original/dithered | +/-: zoom | 0: fit | m: map | mouse: drag/wheel/map | Up/Down k/j: select | Left/Right h/l: change | r/R: rotate | f: flip | d/D: reset | q: quit"
        );
    }

    #[test]
    fn setting_list_keeps_scroll_offset_when_moving_within_visible_rows() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut renderer = Renderer::default();
        let mut app = App::builtin(RgbaImage::new(4, 4), None);
        let mut terminal = Terminal::new(TestBackend::new(60, 7)).unwrap();
        for _ in 0..14 {
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
        assert_eq!(app.panel.selected(Setting::ALL.len()), Some(14));
        assert_eq!(app.panel.offset(), 12);
        app.handle_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &app.geometry(layout(terminal.size().unwrap()).preview),
        );
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert_eq!(app.panel.selected(Setting::ALL.len()), Some(13));
        assert_eq!(app.panel.offset(), 12);
        for label in ["Seed", "Brightness", "Contrast"] {
            assert!(text.contains(label), "missing {label}");
        }
    }

    #[test]
    fn overview_and_exact_modes_show_their_status() {
        let mut renderer = Renderer::default();
        let mut app = App::builtin(RgbaImage::new(8, 12), None);
        app.settings = crate::settings::Settings::for_pixels(8, 12);
        app.show_original = false;
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
    }
    #[test]
    fn leaving_graphics_repaints_the_previous_image_in_one_draw() {
        use ratatui::buffer::CellDiffOption;
        use ratatui_image::picker::ProtocolType;
        for exact in [false, true] {
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
                app.show_original = exact;
                app.viewport.zoom = if exact {
                    viewport::Zoom::In(1)
                } else {
                    viewport::Zoom::Fit
                };
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
            for original in [true, false] {
                app.show_original = original;
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
                    terminal
                        .draw(|frame| {
                            let g = app.geometry(layout(frame.area()).preview);
                            let report = draw_preview(frame, &app, &g, &mut renderer);
                            assert_eq!(
                                report.graphics_name.is_some(),
                                original && zoom == viewport::Zoom::Fit
                            );
                            if !original {
                                assert!(report.graphics_name.is_none());
                            }
                        })
                        .unwrap();
                }
            }
        }
    }
    #[test]
    fn print_status_includes_bleed_and_empty_area_and_truncates() {
        let mut app = App::builtin(RgbaImage::new(8, 8), None);
        app.settings.bleed = 3;
        app.settings.rotation = crate::settings::Rotation::R90;
        app.settings.flip = true;
        let geometry = app.geometry(Rect::new(0, 0, 100, 20));
        let text = status(&app, &geometry, PreviewReport::default());
        assert!(
            text.contains(
                "Card 50x90 mm (+3 mm bleed) | Dots 280x480 | Dot 0.20 mm | Rot 90° Flip"
            )
        );
        assert!(!text.contains("PNG"));
        app.dpi = Some(600);
        assert!(
            status(&app, &geometry, PreviewReport::default()).contains("PNG 1323x2268 @600dpi")
        );
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
