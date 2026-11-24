use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, List, ListState, Paragraph};
use rthid::dither;

use crate::app::{App, PALETTES, PARAMS, Param};

impl App {
    pub fn draw(&self, frame: &mut Frame) {
        let [body, help, status] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let [preview, panel] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(29)]).areas(body);
        let block = Block::bordered().title("rthid");
        self.draw_preview(frame, block.inner(preview));
        frame.render_widget(block, preview);
        let list = List::new(PARAMS.map(|param| self.label(param)))
            .block(Block::bordered().title("Parameters"))
            .highlight_symbol("> ")
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan));
        let mut state = ListState::default().with_selected(Some(self.selected));
        frame.render_stateful_widget(list, panel, &mut state);
        frame.render_widget(
            Paragraph::new(
                "Up/Down k/j: select | Left/Right h/l: change | r/R: rotate CW/CCW | d/D: reset one/all | q/Esc: quit",
            ),
            help,
        );
        let (gw, gh) = self.grid_size();
        frame.render_widget(
            Paragraph::new(format!(
                "Source {}x{} | Grid {gw}x{gh} | Rotation {}°",
                self.source.width(),
                self.source.height(),
                u16::from(self.params.rotation) * 90,
            )),
            status,
        );
    }

    // Half-block cells: fg is the top pixel, bg the bottom one. Never upscales.
    fn draw_preview(&self, frame: &mut Frame, area: Rect) {
        if area.is_empty() {
            return;
        }
        let (gw, gh) = self.grid_size();
        let fit = (f64::from(area.width) / f64::from(gw))
            .min(f64::from(area.height) * 2.0 / f64::from(gh))
            .min(1.0);
        let w = (f64::from(gw) * fit).floor().max(1.0) as usize;
        let h = (f64::from(gh) * fit).floor().max(1.0) as usize;
        let palette = &self.params.options.palette;
        let pixels = dither::rasterize(&self.indices, gw as usize, gh as usize, w, h, |i| {
            palette[usize::from(i)].rgb
        });
        let area = area.centered(
            Constraint::Length(w as u16),
            Constraint::Length(h.div_ceil(2) as u16),
        );
        let color = |x, y| {
            let i = (y * w + x) * 4;
            Color::Rgb(pixels[i], pixels[i + 1], pixels[i + 2])
        };
        for y in (0..h).step_by(2) {
            for x in 0..w {
                let bg = if y + 1 < h {
                    color(x, y + 1)
                } else {
                    Color::Reset
                };
                frame.buffer_mut()[(area.x + x as u16, area.y + (y / 2) as u16)]
                    .set_symbol("▀")
                    .set_fg(color(x, y))
                    .set_bg(bg);
            }
        }
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
    fn draws_parameters_and_truecolor_half_blocks() {
        let app = App::new(RgbaImage::from_pixel(3, 3, image::Rgba([255; 4])));
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
}
