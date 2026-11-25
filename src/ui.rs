use crate::view;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout as Split, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, List, Paragraph};

use crate::app::{App, PALETTES, PARAMS, Param};

/// Terminal regions shared by input handling and rendering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Preview border and title.
    pub preview_block: Rect,
    /// Preview content inside the border.
    pub preview: Rect,
    /// Parameter list including its border.
    pub panel: Rect,
    /// Key bindings below the body.
    pub help: Rect,
    /// Preview and export status.
    pub status: Rect,
}

/// Splits the screen and reserves the preview border.
pub fn layout(screen: Rect) -> Layout {
    let [body, help, status] = Split::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(screen);
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

impl App {
    /// Records minimap cells from the completed frame and its undimmed overview.
    pub fn log_minimap(
        &mut self,
        log: &mut impl std::io::Write,
        frame: &ratatui::CompletedFrame<'_>,
        layout: &Layout,
    ) -> std::io::Result<()> {
        let (w, h) = self.grid_size();
        writeln!(
            log,
            "[minimap] frame={} grid={w}x{h} view={:?} area={:?}",
            frame.count, self.view, layout.preview
        )?;
        let geometry = view::geometry(&self.view, (w, h), layout.preview);
        let mut overview = ratatui::buffer::Buffer::empty(layout.preview);
        let mut cache = std::mem::take(&mut self.minimap);
        let pixels = self.displayed();
        if let Some(view::Minimap {
            frame: inset,
            map,
            viewport,
        }) = geometry.minimap
        {
            self.view
                .render_minimap(pixels, &geometry, &mut overview, &mut cache);
            writeln!(log, "inset={inset:?} map={map:?} viewport={viewport:?}")?;
            for pos in inset.positions() {
                writeln!(
                    log,
                    "{},{} displayed={:?} overview={:?}",
                    pos.x, pos.y, frame.buffer[pos], overview[pos]
                )?;
            }
        }
        self.minimap = cache;
        log.flush()
    }

    /// Draws into the supplied regions without changing layout or view state.
    pub fn draw(&mut self, frame: &mut Frame, layout: &Layout) {
        let focus_style =
            |focused| Style::default().fg(if focused { Color::Cyan } else { Color::Reset });
        let block = Block::bordered()
            .title("rthid")
            .border_style(focus_style(self.preview_focus));
        let (gw, gh) = self.grid_size();
        let geometry = view::geometry(&self.view, (gw, gh), layout.preview);
        let approximate = self.draw_preview(frame, &geometry);
        let graphics = self
            .graphics
            .as_ref()
            .filter(|_| self.show_original && !layout.preview.is_empty());
        frame.render_widget(block, layout.preview_block);
        let list = List::new(PARAMS.map(|param| self.label(param)))
            .block(
                Block::bordered()
                    .title("Parameters")
                    .border_style(focus_style(!self.preview_focus)),
            )
            .highlight_symbol("> ")
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan));
        frame.render_stateful_widget(list, layout.panel, &mut self.parameter_list);
        frame.render_widget(
            Paragraph::new(if self.preview_focus {
                "Tab: focus | Space: original/dithered | z: overview/1:1 | hjkl/arrows: pan | HJKL: page | m: map | mouse: drag/wheel/map | r/R: rotate | d/D: reset | Esc: back | q: quit"
            } else {
                "Tab: focus | Space: original/dithered | mouse: drag/wheel/map (1:1) | Up/Down k/j: select | Left/Right h/l: change | r/R: rotate | d/D: reset | q: quit"
            }),
            layout.help,
        );
        let mode = if self.view.exact {
            let (w, h) = geometry.visible;
            if w == 0 || h == 0 {
                "1:1 empty".into()
            } else {
                format!(
                    "1:1 x {}-{} y {}-{}",
                    geometry.origin.0 + 1,
                    geometry.origin.0 + w,
                    geometry.origin.1 + 1,
                    geometry.origin.1 + h
                )
            }
        } else {
            format!(
                "Overview ({}){}",
                self.view.glyphs.name(),
                if approximate { " approximate" } else { "" },
            )
        };
        let displayed = if let Some(graphics) = graphics {
            let name = graphics.name();
            format!("Original ({name})")
        } else if self.show_original {
            "Original".into()
        } else {
            "Dithered".into()
        };
        frame.render_widget(
            Paragraph::new(
                self.png_error
                    .clone()
                    .or_else(|| graphics.and_then(|graphics| graphics.error().map(str::to_owned)))
                    .unwrap_or_else(|| {
                        format!(
                            "{displayed} | {mode} | Source {}x{} | Grid {gw}x{gh} | Rotation {}°",
                            self.pipeline.source.width(),
                            self.pipeline.source.height(),
                            u16::from(self.params.rotation) * 90,
                        )
                    }),
            ),
            layout.status,
        );
    }

    // Draws the preview, as a protocol image when Original is shown on a graphics terminal.
    fn draw_preview(&mut self, frame: &mut Frame, geometry: &view::PreviewGeometry) -> bool {
        let (gw, gh) = geometry.grid;
        let image_mode = self.show_original && !geometry.area.is_empty();
        let Some(graphics) = self.graphics.as_mut().filter(|_| image_mode) else {
            let cache = if self.view.exact {
                &mut self.minimap
            } else {
                &mut self.overview
            };
            let mut cache = std::mem::take(cache);
            let approximate = view::render(
                self.displayed(),
                gw,
                gh,
                &self.view,
                geometry.area,
                frame.buffer_mut(),
                &mut cache,
            );
            if self.view.exact {
                self.minimap = cache;
            } else {
                self.overview = cache;
            }
            if let Some(graphics) = self.graphics.as_mut() {
                graphics.hide(frame.buffer_mut());
            }
            return approximate;
        };
        graphics.draw_image(
            frame,
            self.pipeline.grid.get().expect("processed grid"),
            self.pipeline.grid.version(),
            &self.view,
            geometry,
            &mut self.minimap,
        );
        false
    }

    fn label(&self, param: Param) -> String {
        let o = &self.params.options;
        match param {
            Param::Scale => format!("Scale: {}%", self.params.scale),
            Param::Mode => format!("Mode: {:?}", o.mode),
            Param::Palette => format!("Palette: {}", PALETTES[self.params.palette].0),
            Param::Strength => format!("Strength: {:.1}", o.strength),
            Param::Serpentine => format!("Serpentine: {}", if o.serpentine { "on" } else { "off" }),
            Param::Brightness => format!("Brightness: {:+.2}", o.tone.brightness),
            Param::Contrast => format!("Contrast: {:+.2}", o.tone.contrast),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbaImage;
    use ratatui::{Terminal, backend::TestBackend};

    fn terminal_layout(terminal: &Terminal<TestBackend>) -> Layout {
        let size = terminal.size().unwrap();
        layout(Rect::new(0, 0, size.width, size.height))
    }

    #[test]
    fn rgb_overview_and_minimap_keep_separate_memos() {
        let mut app = App::new(RgbaImage::new(120, 120), None);
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        for exact in [false, true, false, true] {
            app.view.exact = exact;
            terminal
                .draw(|frame| app.draw(frame, &layout(frame.area())))
                .unwrap();
        }
        assert_eq!(app.overview.version(), 1);
        assert_eq!(app.minimap.version(), 1);
        app.params.rotation = 2;
        app.sync(&terminal_layout(&terminal));
        for exact in [false, true] {
            app.view.exact = exact;
            terminal
                .draw(|frame| app.draw(frame, &layout(frame.area())))
                .unwrap();
        }
        assert_eq!(app.overview.version(), 2);
        assert_eq!(app.minimap.version(), 2);
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
    fn draw_preserves_view_state() {
        let mut app = App::new(RgbaImage::new(120, 120), None);
        app.view.exact = true;
        app.view.x = 119;
        app.view.y = 119;
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let layout = terminal_layout(&terminal);
        terminal.draw(|frame| app.draw(frame, &layout)).unwrap();
        assert_eq!((app.view.x, app.view.y), (119, 119));
        app.sync(&layout);
        assert_eq!((app.view.x, app.view.y), (51, 48));
    }

    #[test]
    fn minimap_log_records_completed_cells_and_uncovered_reference() {
        let mut app = App::new(RgbaImage::from_pixel(180, 180, image::Rgba([255; 4])), None);
        app.view.exact = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut log = Vec::new();
        let frame = terminal.draw(|f| app.draw(f, &layout(f.area()))).unwrap();
        app.log_minimap(&mut log, &frame, &layout(frame.area))
            .unwrap();
        let text = String::from_utf8(log).unwrap();
        assert!(text.starts_with("[minimap] frame=0 grid=180x180"));
        let (inset, map) = app
            .view
            .minimap(180, 180, layout(frame.area).preview)
            .unwrap();
        assert_eq!(text.lines().count(), 2 + inset.area() as usize);
        let corner = text
            .lines()
            .find(|line| line.starts_with(&format!("{},{} ", map.x, map.y)))
            .unwrap();
        assert!(corner.contains(
            "displayed=Cell { symbol: Some(\"▀\"), fg: Rgb(255, 255, 255), bg: Rgb(255, 255, 255)"
        ));
        assert!(corner.contains(
            "overview=Cell { symbol: Some(\"▀\"), fg: Rgb(255, 255, 255), bg: Rgb(255, 255, 255)"
        ));
        assert!(corner.contains("modifier: NONE"));
        let outside = text
            .lines()
            .find(|line| line.starts_with(&format!("{},{} ", map.right() - 1, map.bottom() - 1)))
            .unwrap();
        assert!(outside.contains(
            "displayed=Cell { symbol: Some(\"▀\"), fg: Rgb(127, 127, 127), bg: Rgb(127, 127, 127)"
        ));
        assert!(outside.contains(
            "overview=Cell { symbol: Some(\"▀\"), fg: Rgb(255, 255, 255), bg: Rgb(255, 255, 255)"
        ));

        assert!(
            app.log_minimap(
                &mut std::io::Cursor::new(&mut [0u8; 1][..]),
                &frame,
                &layout(frame.area)
            )
            .is_err()
        );
    }

    #[test]
    fn draws_parameters_and_truecolor_half_blocks() {
        let mut app = App::new(RgbaImage::from_pixel(3, 3, image::Rgba([255; 4])), None);
        assert!(app.show_original);
        app.view.exact = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| app.draw(frame, &layout(frame.area())))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        for label in [
            "Scale",
            "Mode",
            "Palette",
            "Strength",
            "Serpentine",
            "Brightness",
            "Contrast",
            "Original | 1:1",
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
                .draw(|frame| app.draw(frame, &layout(frame.area())))
                .unwrap();
        }
    }
    #[test]
    fn parameter_list_keeps_scroll_offset_when_moving_within_visible_rows() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = App::new(RgbaImage::new(4, 4), None);
        let mut terminal = Terminal::new(TestBackend::new(60, 7)).unwrap();
        for _ in 0..6 {
            app.handle_key(
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                &view::geometry(
                    &app.view,
                    app.grid_size(),
                    terminal_layout(&terminal).preview,
                ),
            );
            terminal
                .draw(|frame| app.draw(frame, &layout(frame.area())))
                .unwrap();
        }
        assert_eq!(app.parameter_list.selected(), Some(6));
        assert_eq!(app.parameter_list.offset(), 4);
        app.handle_key(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &view::geometry(
                &app.view,
                app.grid_size(),
                terminal_layout(&terminal).preview,
            ),
        );
        terminal
            .draw(|frame| app.draw(frame, &layout(frame.area())))
            .unwrap();
        assert_eq!(app.parameter_list.selected(), Some(5));
        assert_eq!(app.parameter_list.offset(), 4);
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        for label in ["Serpentine", "Brightness", "Contrast"] {
            assert!(text.contains(label), "missing {label}");
        }
    }

    #[test]
    fn minimap_toggle_reveals_covered_dots_without_redithering() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let mut app = App::new(RgbaImage::new(120, 120), None);
        app.view.exact = true;
        app.preview_focus = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| app.draw(frame, &layout(frame.area())))
            .unwrap();
        let has_frame = |buffer: &ratatui::buffer::Buffer| {
            buffer
                .content
                .iter()
                .any(|c| c.fg == Color::Gray && c.symbol() == "┌")
        };
        assert!(has_frame(terminal.backend().buffer()));
        app.handle_key(
            KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
            &view::geometry(
                &app.view,
                app.grid_size(),
                terminal_layout(&terminal).preview,
            ),
        );
        assert!(!app.dirty);
        terminal
            .draw(|frame| app.draw(frame, &layout(frame.area())))
            .unwrap();
        assert!(!has_frame(terminal.backend().buffer()));
        app.handle_key(
            KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
            &view::geometry(
                &app.view,
                app.grid_size(),
                terminal_layout(&terminal).preview,
            ),
        );
        terminal
            .draw(|frame| app.draw(frame, &layout(frame.area())))
            .unwrap();
        assert!(has_frame(terminal.backend().buffer()));
    }

    #[test]
    fn overview_and_exact_modes_draw_their_glyphs_and_status() {
        let mut app = App::new(RgbaImage::new(4, 6), None);
        app.show_original = false;
        app.sync(&layout(Rect::new(0, 0, 100, 24)));
        app.pipeline.dithered.get_mut().unwrap()[0] = 1;
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| app.draw(frame, &layout(frame.area())))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(buffer.content.iter().any(|cell| {
            cell.symbol()
                .chars()
                .any(|c| ('\u{1fb00}'..='\u{1fb3b}').contains(&c))
        }));
        let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Dithered | Overview (sextant)"));
        app.view.glyphs = view::Glyphs::Quadrant;
        terminal
            .draw(|frame| app.draw(frame, &layout(frame.area())))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Overview (quadrant)"));
        assert!(
            !text
                .chars()
                .any(|c| ('\u{1fb00}'..='\u{1fb3b}').contains(&c))
        );
        app.view.exact = true;
        app.preview_focus = true;
        terminal
            .draw(|frame| app.draw(frame, &layout(frame.area())))
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert!(buffer.content.iter().any(|cell| cell.symbol() == "▀"));
        let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("1:1 x 1-4 y 1-6"));
        assert_eq!(buffer[(0, 0)].fg, Color::Cyan);
    }
}
