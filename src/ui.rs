//! Screen layout and per frame drawing of the preview, parameter panel, help line
//! and status line.
//!
//! Drawing never mutates App. State that must change while drawing, such as caches
//! and scroll offsets, lives in Renderer.

use crate::{cells, graphics, viewport};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout as Split, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, List, ListState, Paragraph};

use crate::{app::App, settings::Setting};

/// Terminal regions shared by input handling and rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
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
    let [body, help, status] = Split::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(screen.into());
    let [preview_block, panel] =
        Split::horizontal([Constraint::Min(0), Constraint::Length(29)]).areas(body);
    Layout {
        preview_block,
        preview: Block::bordered().inner(preview_block),
        panel,
        help,
        status,
    }
}

/// Drawing state that lives across frames, so draw can update caches while App
/// stays borrowed immutably. Holds terminal graphics, RGB overview caches and the
/// parameter panel scroll state.
#[derive(Default)]
pub struct Renderer {
    graphics: Option<graphics::Graphics>,
    overviews: cells::OverviewCaches,
    setting_list: ListState,
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
    let mut overview = ratatui::buffer::Buffer::empty(geometry.area);
    let pixels = app.displayed();
    if let Some(viewport::Minimap {
        frame: inset,
        map,
        viewport,
    }) = geometry.minimap
    {
        let area = Block::bordered().inner(inset);
        let fitted = viewport::Viewport {
            zoom: viewport::Zoom::Fit,
            ..app.viewport
        };
        cells::render(
            pixels,
            &fitted,
            &viewport::geometry(&fitted, geometry.grid, area),
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

/// Draws the preview, parameter panel, help line and status line without changing
/// App. Expects App::sync to have run for this layout.
pub fn draw(frame: &mut Frame, app: &App, layout: &Layout, renderer: &mut Renderer) {
    let focus_style =
        |focused| Style::default().fg(if focused { Color::Cyan } else { Color::Reset });
    let block = Block::bordered()
        .title("rthid")
        .border_style(focus_style(app.preview_focus));
    let geometry = app.geometry(layout.preview);
    let report = draw_preview(frame, app, &geometry, renderer);
    frame.render_widget(block, layout.preview_block);
    let list = List::new(Setting::ALL.map(|setting| setting.label(&app.settings)))
        .block(
            Block::bordered()
                .title("Parameters")
                .border_style(focus_style(!app.preview_focus)),
        )
        .highlight_symbol("> ")
        .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan));
    renderer.setting_list.select(Some(app.selected));
    frame.render_stateful_widget(list, layout.panel, &mut renderer.setting_list);
    frame.render_widget(
            Paragraph::new(if app.preview_focus {
                "Tab: focus | Space: original/dithered | +/-: zoom | 0: fit | hjkl/arrows: pan | HJKL: page | m: map | mouse: drag/wheel/map | r/R: rotate | d/D: reset | Esc: back | q: quit"
            } else {
                "Tab: focus | Space: original/dithered | +/-: zoom | 0: fit | m: map | mouse: drag/wheel/map | Up/Down k/j: select | Left/Right h/l: change | r/R: rotate | d/D: reset | q: quit"
            }),
            layout.help,
        );
    frame.render_widget(
        Paragraph::new(status(app, &geometry, report)),
        layout.status,
    );
}

// Uses terminal graphics only for Original at Fit and cell glyphs otherwise.
// Calls present every frame so a hidden image is cleared.
fn draw_preview(
    frame: &mut Frame,
    app: &App,
    geometry: &viewport::PreviewGeometry,
    renderer: &mut Renderer,
) -> PreviewReport {
    let image_mode = renderer.graphics.is_some()
        && app.show_original
        && geometry.zoom == viewport::Zoom::Fit
        && !geometry.area.is_empty();
    let approximate = if image_mode {
        false
    } else {
        cells::render(
            app.displayed(),
            &app.viewport,
            geometry,
            frame.buffer_mut(),
            &mut renderer.overviews,
        )
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
                    app.grid_cache.grid(),
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

// A companion PNG error, then a graphics error, replaces the normal status line.
fn status(app: &App, geometry: &viewport::PreviewGeometry, report: PreviewReport) -> String {
    let (gw, gh) = geometry.grid;
    let (sw, sh) = app.grid_cache.source_size();
    let mode = if geometry.zoom == viewport::Zoom::Fit {
        format!(
            "Fit ({}){}",
            app.viewport.glyphs.name(),
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
    app.export_status()
        .map(str::to_owned)
        .or(report.graphics_error)
        .unwrap_or_else(|| {
            format!(
                "{displayed} | {mode} | Source {sw}x{sh} | Grid {gw}x{gh} | Rotation {}°",
                app.settings.rotation.degrees(),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;
    use ratatui::{Terminal, backend::TestBackend};

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
    fn status_excludes_partial_edge_dots_until_panned() {
        let mut app = App::new(RgbaImage::new(16, 16), None);
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
    fn status_uses_preview_report_and_prioritizes_export_errors() {
        let mut app = App::new(RgbaImage::new(4, 6), None);
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
                app = App::new(RgbaImage::new(4, 6), Some(dir.path().to_path_buf()));
                app.sync(layout(Rect::new(0, 0, 80, 24)).preview);
                assert!(app.finish_png().is_err());
            }
            assert_eq!(
                status(
                    &app,
                    &geometry,
                    PreviewReport {
                        graphics_error: Some("Graphics failed".to_owned()),
                        ..PreviewReport::default()
                    },
                ),
                app.export_status().unwrap_or("Graphics failed"),
            );
        }
    }

    #[test]
    fn layout_preserves_regions_and_preview_border() {
        for (screen, expected) in [
            (
                Rect::new(0, 0, 100, 40),
                Layout {
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
        let mut app = App::new(RgbaImage::from_pixel(180, 180, image::Rgba([255; 4])), None);
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
            assert!(cell.contains("fg: Rgb(255, 255, 255)"));
            assert!(cell.contains("bg: Rgb(255, 255, 255)"));
        }
        assert_eq!(frame.buffer[(map.x, map.y)].symbol(), "▀");
        let outside = text
            .lines()
            .find(|line| line.starts_with(&format!("{},{} ", map.right() - 1, map.bottom() - 1)))
            .unwrap();
        let (displayed, overview) = outside.split_once(" overview=").unwrap();
        assert!(displayed.contains("fg: Rgb(127, 127, 127)"));
        assert!(displayed.contains("bg: Rgb(127, 127, 127)"));
        assert!(overview.contains("fg: Rgb(255, 255, 255)"));
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
    fn draws_settings_and_truecolor_half_blocks() {
        let mut renderer = Renderer::default();
        let mut app = App::new(RgbaImage::from_pixel(3, 3, image::Rgba([255; 4])), None);
        assert!(app.show_original);
        app.viewport.zoom = viewport::Zoom::In(1);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let text = draw_text(&mut terminal, &app, &mut renderer);
        let buffer = terminal.backend().buffer();
        for label in [
            "Scale",
            "Mode",
            "Palette",
            "Strength",
            "Serpentine",
            "Brightness",
            "Contrast",
            "Original | 100%",
            "Source 3x3",
            "Grid 3x3",
            "Rotation 0",
        ] {
            assert!(text.contains(label), "missing {label}");
        }
        let cell = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == "▀")
            .unwrap();
        assert_eq!(cell.fg, Color::Rgb(255, 255, 255));
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
    fn setting_list_keeps_scroll_offset_when_moving_within_visible_rows() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut renderer = Renderer::default();
        let mut app = App::new(RgbaImage::new(4, 4), None);
        let mut terminal = Terminal::new(TestBackend::new(60, 7)).unwrap();
        for _ in 0..6 {
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
        assert_eq!(renderer.setting_list.selected(), Some(6));
        assert_eq!(renderer.setting_list.offset(), 4);
        app.handle_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &app.geometry(layout(terminal.size().unwrap()).preview),
        );
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert_eq!(renderer.setting_list.selected(), Some(5));
        assert_eq!(renderer.setting_list.offset(), 4);
        for label in ["Serpentine", "Brightness", "Contrast"] {
            assert!(text.contains(label), "missing {label}");
        }
    }

    #[test]
    fn overview_and_exact_modes_show_their_status() {
        let mut renderer = Renderer::default();
        let mut app = App::new(RgbaImage::new(4, 6), None);
        app.show_original = false;
        app.sync(layout(Rect::new(0, 0, 100, 24)).preview);
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert!(text.contains("Dithered | Fit (sextant)"));
        app.viewport.glyphs = cells::Glyphs::Quadrant;
        let text = draw_text(&mut terminal, &app, &mut renderer);
        assert!(text.contains("Fit (quadrant)"));
        app.viewport.zoom = viewport::Zoom::In(1);
        app.preview_focus = true;
        let text = draw_text(&mut terminal, &app, &mut renderer);
        let buffer = terminal.backend().buffer();
        assert!(text.contains("100% x 1-4 y 1-6"));
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
                let mut app = App::new(RgbaImage::new(12, 12), None);
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
            let mut app = App::new(RgbaImage::new(400, 400), None);
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
}
