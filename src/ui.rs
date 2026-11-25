use crate::view;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, List, Paragraph};

use crate::app::{App, GraphicsKey, KITTY_IMAGE_ID, PALETTES, PARAMS, Param};
use ratatui::buffer::CellDiffOption;
use ratatui_image::{
    Resize, StatefulImage,
    picker::{Capability, ProtocolType},
    protocol::{StatefulProtocol, StatefulProtocolType, kitty::StatefulKitty},
};

impl App {
    pub fn log_minimap(
        &self,
        log: &mut impl std::io::Write,
        frame: &ratatui::CompletedFrame<'_>,
    ) -> std::io::Result<()> {
        let (w, h) = self.grid_size();
        writeln!(
            log,
            "[minimap] frame={} grid={w}x{h} view={:?} area={:?}",
            frame.count, self.view, self.preview_area
        )?;
        let mut overview = ratatui::buffer::Buffer::empty(self.preview_area);
        let pixels = self.displayed();
        if let Some((inset, map, viewport)) =
            self.view
                .render_minimap(pixels, w, h, self.preview_area, &mut overview)
        {
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

    pub fn draw(&mut self, frame: &mut Frame) {
        let [body, help, status] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [preview, panel] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(29)]).areas(body);
        let focus_style =
            |focused| Style::default().fg(if focused { Color::Cyan } else { Color::Reset });
        let block = Block::bordered()
            .title("rthid")
            .border_style(focus_style(self.preview_focus));
        self.preview_area = block.inner(preview);
        let (gw, gh) = self.grid_size();
        self.view.clamp(gw, gh, self.preview_area);
        let graphics =
            self.show_original && self.graphics.is_some() && !self.preview_area.is_empty();
        let approximate = if graphics {
            let picker = self.graphics.as_ref().unwrap();
            let font = picker.font_size();
            let key = GraphicsKey {
                generation: self.generation,
                x: self.view.x,
                y: self.view.y,
                exact: self.view.exact,
                area: self.preview_area,
                font: (font.width, font.height),
            };
            if self
                .graphics_cache
                .as_ref()
                .is_none_or(|(old, _, _)| *old != key)
            {
                let (rect, image) = view::original_image(
                    &self.original,
                    gw,
                    gh,
                    &self.view,
                    self.preview_area,
                    key.font,
                );
                let protocol = if picker.protocol_type() == ProtocolType::Kitty {
                    StatefulProtocol::new(
                        image.into(),
                        font,
                        None,
                        StatefulProtocolType::Kitty(StatefulKitty::new(
                            KITTY_IMAGE_ID,
                            picker.tmux_detected(),
                            picker
                                .capabilities()
                                .contains(&Capability::KittyCompression),
                        )),
                    )
                } else {
                    picker.new_resize_protocol(image.into())
                };
                self.graphics_cache = Some((key, rect, protocol));
                self.graphics_error = None;
            }
            let (_, rect, protocol) = self.graphics_cache.as_mut().unwrap();
            frame.render_stateful_widget(
                StatefulImage::new().resize(Resize::Fit(None)),
                *rect,
                protocol,
            );
            if let Some(result) = protocol.last_encoding_result() {
                self.graphics_error = result.err().map(|error| format!("Graphics: {error}"));
            }
            if self.view.exact {
                view::render_minimap(
                    self.displayed(),
                    gw,
                    gh,
                    &self.view,
                    self.preview_area,
                    frame.buffer_mut(),
                );
                if matches!(
                    picker.protocol_type(),
                    ProtocolType::Iterm2 | ProtocolType::Sixel
                ) && let Some((inset, _)) = self.view.minimap(gw, gh, self.preview_area)
                {
                    for pos in inset.positions() {
                        frame.buffer_mut()[pos].set_diff_option(CellDiffOption::AlwaysUpdate);
                    }
                }
            }
            false
        } else {
            view::render(
                self.displayed(),
                gw,
                gh,
                &self.view,
                self.preview_area,
                frame.buffer_mut(),
            )
        };
        frame.render_widget(block, preview);
        let list = List::new(PARAMS.map(|param| self.label(param)))
            .block(
                Block::bordered()
                    .title("Parameters")
                    .border_style(focus_style(!self.preview_focus)),
            )
            .highlight_symbol("> ")
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan));
        frame.render_stateful_widget(list, panel, &mut self.parameter_list);
        frame.render_widget(
            Paragraph::new(if self.preview_focus {
                "Tab: focus | Space: original/dithered | z: overview/1:1 | hjkl/arrows: pan | HJKL: page | m: map | mouse: drag/wheel/map | r/R: rotate | d/D: reset | Esc: back | q: quit"
            } else {
                "Tab: focus | Space: original/dithered | mouse: drag/wheel/map (1:1) | Up/Down k/j: select | Left/Right h/l: change | r/R: rotate | d/D: reset | q: quit"
            }),
            help,
        );
        let mode = if self.view.exact {
            let (w, h) = self.view.visible(gw, gh, self.preview_area);
            if w == 0 || h == 0 {
                "1:1 empty".into()
            } else {
                format!(
                    "1:1 x {}-{} y {}-{}",
                    self.view.x + 1,
                    self.view.x + w,
                    self.view.y + 1,
                    self.view.y + h
                )
            }
        } else {
            format!(
                "Overview ({}){}",
                self.view.glyphs.name(),
                if approximate { " approximate" } else { "" },
            )
        };
        let displayed = if graphics {
            let name = match self.graphics.as_ref().unwrap().protocol_type() {
                ProtocolType::Kitty => "kitty",
                ProtocolType::Iterm2 => "iTerm2",
                ProtocolType::Sixel => "sixel",
                ProtocolType::Halfblocks => "halfblocks",
            };
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
                    .or_else(|| graphics.then(|| self.graphics_error.clone()).flatten())
                    .unwrap_or_else(|| {
                        format!(
                            "{displayed} | {mode} | Source {}x{} | Grid {gw}x{gh} | Rotation {}°",
                            self.source.width(),
                            self.source.height(),
                            u16::from(self.params.rotation) * 90,
                        )
                    }),
            ),
            status,
        );
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

    #[test]
    fn minimap_log_records_completed_cells_and_uncovered_reference() {
        let mut app = App::new(RgbaImage::from_pixel(180, 180, image::Rgba([255; 4])), None);
        app.view.exact = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut log = Vec::new();
        let frame = terminal.draw(|f| app.draw(f)).unwrap();
        app.log_minimap(&mut log, &frame).unwrap();
        let text = String::from_utf8(log).unwrap();
        assert!(text.starts_with("[minimap] frame=0 grid=180x180"));
        let (inset, map) = app.view.minimap(180, 180, app.preview_area).unwrap();
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
            app.log_minimap(&mut std::io::Cursor::new(&mut [0u8; 1][..]), &frame)
                .is_err()
        );
    }

    #[test]
    fn draws_parameters_and_truecolor_half_blocks() {
        let mut app = App::new(RgbaImage::from_pixel(3, 3, image::Rgba([255; 4])), None);
        assert!(app.show_original);
        app.view.exact = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
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
            terminal.draw(|frame| app.draw(frame)).unwrap();
        }
    }
    #[test]
    fn parameter_list_keeps_scroll_offset_when_moving_within_visible_rows() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = App::new(RgbaImage::new(4, 4), None);
        let mut terminal = Terminal::new(TestBackend::new(60, 7)).unwrap();
        for _ in 0..6 {
            app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
            terminal.draw(|frame| app.draw(frame)).unwrap();
        }
        assert_eq!(app.parameter_list.selected(), Some(6));
        assert_eq!(app.parameter_list.offset(), 4);
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        terminal.draw(|frame| app.draw(frame)).unwrap();
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
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let has_frame = |buffer: &ratatui::buffer::Buffer| {
            buffer
                .content
                .iter()
                .any(|c| c.fg == Color::Gray && c.symbol() == "┌")
        };
        assert!(has_frame(terminal.backend().buffer()));
        app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        assert!(!app.dirty);
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert!(!has_frame(terminal.backend().buffer()));
        app.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert!(has_frame(terminal.backend().buffer()));
    }

    #[test]
    fn overview_and_exact_modes_draw_their_glyphs_and_status() {
        let mut app = App::new(RgbaImage::new(4, 6), None);
        app.show_original = false;
        app.indices = vec![0; 24];
        app.indices[0] = 1;
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        assert!(buffer.content.iter().any(|cell| {
            cell.symbol()
                .chars()
                .any(|c| ('\u{1fb00}'..='\u{1fb3b}').contains(&c))
        }));
        let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("Dithered | Overview (sextant)"));
        app.view.glyphs = view::Glyphs::Quadrant;
        terminal.draw(|frame| app.draw(frame)).unwrap();
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
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        assert!(buffer.content.iter().any(|cell| cell.symbol() == "▀"));
        let text: String = buffer.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("1:1 x 1-4 y 1-6"));
        assert_eq!(buffer[(0, 0)].fg, Color::Cyan);
    }
}
