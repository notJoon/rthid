use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Rect},
    style::Color,
    widgets::{Block, Clear, Widget},
};
use rthid::dither::PaletteColor;

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyphs {
    #[default]
    Sextant,
    Quadrant,
}

impl Glyphs {
    pub fn for_terminal(program: Option<&str>) -> Self {
        if program == Some("Apple_Terminal") {
            Self::Quadrant
        } else {
            Self::Sextant
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Sextant => "sextant",
            Self::Quadrant => "quadrant",
        }
    }

    fn subcells(self) -> (u32, u32) {
        match self {
            Self::Sextant => (2, 3),
            Self::Quadrant => (2, 2),
        }
    }

    fn glyph(self, mask: u32) -> char {
        match self {
            Self::Quadrant => [
                ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
            ][mask as usize],
            Self::Sextant => match mask {
                0 => ' ',
                21 => '▌',
                42 => '▐',
                63 => '█',
                _ => {
                    char::from_u32(0x1fb00 + mask - 1 - u32::from(mask > 21) - u32::from(mask > 42))
                        .unwrap()
                }
            },
        }
    }
}

// Fits a grid into whole cells, keeping its aspect on cells about 2.25 times taller than wide.
// The glyph set only changes how densely each cell is sampled, not the fitted shape.
pub fn fit(w: u32, h: u32, area: Rect) -> (u16, u16) {
    if area.is_empty() {
        return (0, 0);
    }
    const CELL_ASPECT: f64 = 2.25;
    let scale = (f64::from(area.width) / f64::from(w))
        .min(f64::from(area.height) * CELL_ASPECT / f64::from(h));
    (
        (f64::from(w) * scale).floor().max(1.0) as u16,
        (f64::from(h) * scale / CELL_ASPECT).floor().max(1.0) as u16,
    )
}

#[derive(Default, Clone, Copy, Debug)]
pub struct View {
    pub glyphs: Glyphs,
    pub exact: bool,
    pub hide_minimap: bool,
    pub x: u32,
    pub y: u32,
}

impl View {
    pub fn minimap(&self, w: u32, h: u32, area: Rect) -> Option<(Rect, Rect)> {
        // A bordered frame in the top-right corner; the overview fits inside the border.
        let fw = (area.width / 3).min(18);
        let frame = Rect::new(area.right() - fw, area.y, fw, (area.height / 3).min(10));
        let inner = Block::bordered().inner(frame);
        if !self.exact || self.hide_minimap || inner.width < 3 || inner.height < 3 {
            return None;
        }
        let (mw, mh) = fit(w, h, inner);
        Some((
            frame,
            inner.centered(Constraint::Length(mw), Constraint::Length(mh)),
        ))
    }

    pub fn visible(&self, w: u32, h: u32, area: Rect) -> (u32, u32) {
        (
            w.min(u32::from(area.width)),
            h.min(u32::from(area.height) * 2),
        )
    }

    pub fn clamp(&mut self, w: u32, h: u32, area: Rect) {
        let (vw, vh) = self.visible(w, h, area);
        self.x = self.x.min(w.saturating_sub(vw));
        self.y = self.y.min(h.saturating_sub(vh));
    }

    pub fn pan(&mut self, dx: i64, dy: i64, w: u32, h: u32, area: Rect) {
        self.x = i64::from(self.x)
            .saturating_add(dx)
            .clamp(0, i64::from(u32::MAX)) as u32;
        self.y = i64::from(self.y)
            .saturating_add(dy)
            .clamp(0, i64::from(u32::MAX)) as u32;
        self.clamp(w, h, area);
    }

    pub fn render_minimap(
        &self,
        indices: &[u8],
        w: u32,
        h: u32,
        palette: &[PaletteColor],
        area: Rect,
        buffer: &mut Buffer,
    ) -> Option<(Rect, Rect, Rect)> {
        let (frame, map) = self.minimap(w, h, area)?;
        // Clear letterboxing so the 1:1 dots cannot show through the inset.
        Clear.render(frame, buffer);
        render_overview(
            indices,
            w,
            h,
            palette,
            self.glyphs,
            Block::bordered().inner(frame),
            buffer,
        );
        Some((
            frame,
            map,
            self.viewport_rect(w, h, self.visible(w, h, area), map),
        ))
    }

    pub fn center_on_minimap(&mut self, point: (u16, u16), w: u32, h: u32, area: Rect, map: Rect) {
        let (vw, vh) = self.visible(w, h, area);
        let center = |cell: u16, cells: u16, dots: u32| {
            ((u64::from(cell) * 2 + 1) * u64::from(dots) / (u64::from(cells) * 2)) as i64
        };
        self.pan(
            center(point.0 - map.x, map.width, w) - i64::from(vw / 2) - i64::from(self.x),
            center(point.1 - map.y, map.height, h) - i64::from(vh / 2) - i64::from(self.y),
            w,
            h,
            area,
        );
    }

    pub fn viewport_rect(&self, w: u32, h: u32, visible: (u32, u32), map: Rect) -> Rect {
        let project = |start: u32, length: u32, total: u32, cells: u16| {
            let first = (u64::from(start) * u64::from(cells) / u64::from(total)) as u16;
            let end =
                (u64::from(start + length) * u64::from(cells)).div_ceil(u64::from(total)) as u16;
            (first.min(cells - 1), end.min(cells).max(first + 1) - first)
        };
        let (x, width) = project(self.x, visible.0, w, map.width);
        let (y, height) = project(self.y, visible.1, h, map.height);
        Rect::new(map.x + x, map.y + y, width, height)
    }
}

pub fn render(
    indices: &[u8],
    w: u32,
    h: u32,
    palette: &[PaletteColor],
    view: &View,
    area: Rect,
    buffer: &mut Buffer,
) -> bool {
    if area.is_empty() {
        return false;
    }
    let mut view = *view;
    view.clamp(w, h, area);
    if view.exact {
        render_exact(indices, w, h, palette, &view, area, buffer);
        false
    } else {
        render_overview(indices, w, h, palette, view.glyphs, area, buffer)
    }
}

fn render_exact(
    indices: &[u8],
    w: u32,
    h: u32,
    palette: &[PaletteColor],
    view: &View,
    area: Rect,
    buffer: &mut Buffer,
) {
    let color = |i: u8| {
        let [r, g, b] = palette[usize::from(i)].rgb;
        Color::Rgb(r, g, b)
    };
    let (vw, vh) = view.visible(w, h, area);
    for y in (0..vh).step_by(2) {
        for x in 0..vw {
            let index = ((view.y + y) * w + view.x + x) as usize;
            let bg = if y + 1 < vh {
                color(indices[index + w as usize])
            } else {
                Color::Reset
            };
            buffer[(area.x + x as u16, area.y + (y / 2) as u16)]
                .set_symbol("▀")
                .set_fg(color(indices[index]))
                .set_bg(bg);
        }
    }
    if let Some((frame, map, viewport)) = view.render_minimap(indices, w, h, palette, area, buffer)
    {
        Block::bordered()
            .border_style(Color::Gray)
            .render(frame, buffer);
        for pos in map.positions().filter(|&pos| !viewport.contains(pos)) {
            let cell = &mut buffer[pos];
            for color in [&mut cell.fg, &mut cell.bg] {
                if let Color::Rgb(r, g, b) = *color {
                    *color = Color::Rgb(r / 2, g / 2, b / 2);
                }
            }
        }
    }
}

fn render_overview(
    indices: &[u8],
    w: u32,
    h: u32,
    palette: &[PaletteColor],
    glyphs: Glyphs,
    area: Rect,
    buffer: &mut Buffer,
) -> bool {
    let color = |i: u8| {
        let [r, g, b] = palette[usize::from(i)].rgb;
        Color::Rgb(r, g, b)
    };
    let (cw, ch) = fit(w, h, area);
    let target = area.centered(Constraint::Length(cw), Constraint::Length(ch));
    let (cols, rows) = glyphs.subcells();
    let mut approximate = false;
    for cy in 0..ch {
        for cx in 0..cw {
            let mut samples = [0u8; 6];
            let samples = &mut samples[..(cols * rows) as usize];
            for (n, sample) in samples.iter_mut().enumerate() {
                let sx = u32::from(cx) * cols + n as u32 % cols;
                let sy = u32::from(cy) * rows + n as u32 / cols;
                let x = ((u64::from(sx) * 2 + 1) * u64::from(w)
                    / (u64::from(cw) * u64::from(cols) * 2)) as u32;
                let y = ((u64::from(sy) * 2 + 1) * u64::from(h)
                    / (u64::from(ch) * u64::from(rows) * 2)) as u32;
                *sample = indices[(y * w + x) as usize];
            }
            let mut counts = [0u8; 256];
            for &i in samples.iter() {
                counts[usize::from(i)] += 1;
            }
            let mut ranked = [0u8; 6];
            let ranked = &mut ranked[..samples.len()];
            ranked.copy_from_slice(samples);
            ranked.sort_by_key(|&i| (std::cmp::Reverse(counts[usize::from(i)]), i));
            let bg = ranked[0];
            let fg = ranked.iter().copied().find(|&i| i != bg).unwrap_or(bg);
            let distance = |a: u8, b: u8| -> i32 {
                palette[usize::from(a)]
                    .rgb
                    .iter()
                    .zip(palette[usize::from(b)].rgb)
                    .map(|(&a, b)| (i32::from(a) - i32::from(b)).pow(2))
                    .sum()
            };
            let mut mask = 0u32;
            for (n, &i) in samples.iter().enumerate() {
                let kept = if i == fg || (i != bg && distance(i, fg) < distance(i, bg)) {
                    fg
                } else {
                    bg
                };
                approximate |= palette[usize::from(i)].rgb != palette[usize::from(kept)].rgb;
                if kept == fg && fg != bg {
                    mask |= 1 << n;
                }
            }
            let glyph = glyphs.glyph(mask);
            buffer[(target.x + cx, target.y + cy)]
                .set_char(glyph)
                .set_fg(color(fg))
                .set_bg(color(bg));
        }
    }
    approximate
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, layout::Rect, style::Color};
    use rthid::dither::make_palette;

    #[test]
    fn quadrant_masks_use_row_major_bits() {
        let expected = [
            ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
        ];
        for (mask, glyph) in expected.into_iter().enumerate() {
            assert_eq!(Glyphs::Quadrant.glyph(mask as u32), glyph, "mask {mask}");
        }
    }

    #[test]
    fn glyph_default_depends_only_on_terminal_program() {
        for (program, expected) in [
            (Some("Apple_Terminal"), Glyphs::Quadrant),
            (Some("Zed"), Glyphs::Sextant),
            (Some("iTerm.app"), Glyphs::Sextant),
            (Some(""), Glyphs::Sextant),
            (None, Glyphs::Sextant),
        ] {
            assert_eq!(Glyphs::for_terminal(program), expected);
        }
    }

    #[test]
    fn quadrant_overview_and_minimap_use_only_block_elements() {
        let (w, h, palette, indices, mut view) = minimap_fixture();
        view.glyphs = Glyphs::Quadrant;
        let area = Rect::new(2, 3, 60, 30);
        let mut buffer = Buffer::empty(area);
        view.exact = false;
        render(&indices, w, h, &palette, &view, area, &mut buffer);
        let is_block = |cell: &ratatui::buffer::Cell| {
            cell.symbol()
                .chars()
                .all(|c| c == ' ' || ('\u{2580}'..='\u{259f}').contains(&c))
        };
        assert!(buffer.content.iter().all(is_block));
        assert!(buffer.content.iter().any(|cell| cell.symbol() != " "));
        view.exact = true;
        let (_, map, _) = view
            .render_minimap(&indices, w, h, &palette, area, &mut buffer)
            .unwrap();
        assert!(map.positions().all(|pos| is_block(&buffer[pos])));
        assert!(map.positions().any(|pos| buffer[pos].symbol() != " "));
    }

    fn minimap_fixture() -> (u32, u32, Vec<PaletteColor>, Vec<u8>, View) {
        let (w, h) = (180, 180);
        let palette = make_palette(&["#123", "#abc", "#e64", "#fff"]).unwrap();
        let indices = (0..w * h)
            .map(|i| ((i * 17 + i / w * 13 + i / 7) % 4) as u8)
            .collect();
        let view = View {
            exact: true,
            ..View::default()
        };
        (w, h, palette, indices, view)
    }

    #[test]
    fn minimap_pan_preserves_symbols_and_dims_only_outside_viewport() {
        let area = Rect::new(2, 3, 60, 30);
        let (w, h, palette, indices, mut view) = minimap_fixture();
        let mut baseline = Buffer::empty(area);
        let (frame, map, _) = view
            .render_minimap(&indices, w, h, &palette, area, &mut baseline)
            .unwrap();
        Block::bordered()
            .border_style(Color::Gray)
            .render(frame, &mut baseline);
        let mut current = Buffer::empty(area);
        for (x, y) in [(0, 0), (60, 60), (1, 1), (120, 120), (0, 0), (1, 0), (1, 1)] {
            view.x = x;
            view.y = y;
            let viewport = view.viewport_rect(w, h, view.visible(w, h, area), map);
            render(&indices, w, h, &palette, &view, area, &mut current);
            for pos in frame.positions() {
                let original = &baseline[pos];
                let cell = &current[pos];
                assert_eq!(
                    cell.symbol(),
                    original.symbol(),
                    "symbol at {pos:?}, pan {x},{y}"
                );
                let mut expected = original.clone();
                if map.contains(pos) && !viewport.contains(pos) {
                    let dim = |color| match color {
                        Color::Rgb(17, 34, 51) => Color::Rgb(8, 17, 25),
                        Color::Rgb(170, 187, 204) => Color::Rgb(85, 93, 102),
                        Color::Rgb(238, 102, 68) => Color::Rgb(119, 51, 34),
                        Color::Rgb(255, 255, 255) => Color::Rgb(127, 127, 127),
                        _ => panic!("unexpected overview color: {color:?}"),
                    };
                    expected.set_fg(dim(original.fg)).set_bg(dim(original.bg));
                }
                assert_eq!(*cell, expected, "colors at {pos:?}, pan {x},{y}");
            }
        }
    }

    #[test]
    fn minimap_pan_restores_cells_and_emits_colors() {
        use ratatui::{
            Terminal, TerminalOptions, Viewport,
            backend::{Backend, CrosstermBackend, TestBackend},
        };
        ratatui::crossterm::style::force_color_output(true);
        let area = Rect::new(0, 0, 60, 30);
        let (w, h, palette, indices, mut view) = minimap_fixture();
        let mut baseline = Buffer::empty(area);
        let (frame, map, _) = view
            .render_minimap(&indices, w, h, &palette, area, &mut baseline)
            .unwrap();
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        let mut previous: Option<(Buffer, Rect)> = None;
        let mut bytes = Vec::<u8>::new();
        let mut ansi_terminal = Terminal::with_options(
            CrosstermBackend::new(&mut bytes),
            TerminalOptions {
                viewport: Viewport::Fixed(area),
            },
        )
        .unwrap();
        for (x, y) in [(0, 0), (60, 60), (1, 1), (120, 120), (0, 0), (1, 0), (1, 1)] {
            view.x = x;
            view.y = y;
            let viewport = view.viewport_rect(w, h, view.visible(w, h, area), map);
            terminal
                .draw(|f| {
                    render(&indices, w, h, &palette, &view, area, f.buffer_mut());
                })
                .unwrap();
            let current = terminal.backend().buffer().clone();
            let completed = ansi_terminal
                .draw(|f| {
                    render(&indices, w, h, &palette, &view, area, f.buffer_mut());
                })
                .unwrap();
            assert_eq!(completed.buffer, &current);
            if let Some((prev, old)) = &previous {
                let mut restored = 0;
                for pos in frame.positions() {
                    if map.contains(pos) && viewport.contains(pos) && !old.contains(pos) {
                        restored += 1;
                    }
                }
                let emitted = prev.diff(&current);
                let expected: Vec<_> = frame
                    .positions()
                    .filter(|&p| current[p] != prev[p])
                    .collect();
                let actual: Vec<_> = emitted
                    .iter()
                    .filter(|(x, y, _)| frame.contains((*x, *y).into()))
                    .map(|(x, y, _)| ratatui::layout::Position::new(*x, *y))
                    .collect();
                assert_eq!(
                    actual, expected,
                    "minimap diff must emit every changed cell exactly once"
                );
                let mut map_bytes = Vec::<u8>::new();
                let mut map_backend = CrosstermBackend::new(&mut map_bytes);
                map_backend
                    .draw(
                        prev.diff(&current)
                            .into_iter()
                            .filter(|(x, y, _)| frame.contains((*x, *y).into())),
                    )
                    .unwrap();
                assert!(map_bytes.ends_with(b"\x1b[39m\x1b[49m\x1b[59m\x1b[0m"));
                if restored > 0 {
                    assert!(map_bytes.windows(6).any(|s| s == b"\x1b[38;2"));
                    assert!(
                        map_bytes.windows(6).any(|s| s == b";48;2;"),
                        "RGB backgrounds must be explicit"
                    );
                }
            }
            previous = Some((current, viewport));
        }
        drop(ansi_terminal);
        let output = String::from_utf8(bytes).unwrap();
        let second_frame = output.split("\x1b[0m").nth(1).unwrap();
        assert!(second_frame.contains("\x1b[38;2;255;255;255;48;2;238;102;68m🬣"));
        assert!(second_frame.contains("\x1b[38;2;85;93;102;48;2;119;51;34m🬱"));
    }

    #[test]
    fn minimap_glyphs_are_single_width() {
        use ratatui::buffer::CellWidth;
        for ch in ('\u{1fb00}'..='\u{1fb3b}').chain(" ▘▝▀▖▌▞▛▗▚▐▜▄▙▟█┌┐└┘─│".chars())
        {
            assert_eq!(ch.to_string().cell_width(), 1, "{ch:?}");
        }
    }

    #[test]
    fn glyph_set_does_not_change_fitted_shape() {
        let map = |glyphs| {
            let view = View {
                glyphs,
                exact: true,
                ..View::default()
            };
            view.minimap(120, 120, Rect::new(0, 0, 48, 18)).unwrap().1
        };
        assert_eq!(map(Glyphs::Quadrant), map(Glyphs::Sextant));
        assert_eq!(fit(120, 120, Rect::new(0, 0, 0, 5)), (0, 0));
        assert_eq!(fit(120, 120, Rect::new(0, 0, 5, 0)), (0, 0));
    }

    #[test]
    fn overview_fit_corrects_subcell_aspect() {
        assert_eq!(fit(120, 120, Rect::new(0, 0, 60, 40)), (60, 26));
        assert_eq!(fit(120, 120, Rect::new(0, 0, 20, 5)), (11, 5));
        assert_eq!(fit(1, 1000, Rect::new(0, 0, 1, 1)), (1, 1));
    }
    #[test]
    fn clamps_pan_and_projects_minimap_viewport() {
        let mut view = View {
            exact: true,
            hide_minimap: false,
            x: 99,
            y: 99,
            ..View::default()
        };
        view.clamp(100, 80, Rect::new(0, 0, 20, 10));
        assert_eq!((view.x, view.y), (80, 60));
        assert_eq!(view.visible(100, 80, Rect::new(0, 0, 20, 10)), (20, 20));
        assert_eq!(
            view.viewport_rect(100, 80, (20, 20), Rect::new(3, 4, 10, 8)),
            Rect::new(11, 10, 2, 2)
        );
        assert_eq!(
            view.viewport_rect(10_000, 10_000, (1, 1), Rect::new(3, 4, 10, 8)),
            Rect::new(3, 4, 1, 1)
        );
        view.clamp(2, 3, Rect::new(0, 0, 20, 10));
        assert_eq!((view.x, view.y), (0, 0));
    }

    #[test]
    fn render_samples_centers_and_reduces_only_extra_colors() {
        let palette = make_palette(&["#000", "#333", "#fff"]).unwrap();
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        // Sample columns 1 and 3 of each row; column 0 must not participate.
        assert!(!render(
            &[2, 0, 2, 1, 2, 0, 2, 1, 2, 0, 2, 1],
            4,
            3,
            &palette,
            &View::default(),
            area,
            &mut buffer
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▐");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(51, 51, 51));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(0, 0, 0));
        assert!(render(
            &[0, 2, 0, 2, 1, 2],
            2,
            3,
            &palette,
            &View::default(),
            area,
            &mut buffer
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▌");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(0, 0, 0));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(255, 255, 255));
    }

    #[test]
    fn quadrant_samples_centers_and_reduces_only_extra_colors() {
        let palette = make_palette(&["#000", "#333", "#fff"]).unwrap();
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        let view = View {
            glyphs: Glyphs::Quadrant,
            ..View::default()
        };
        assert!(!render(
            &[2, 2, 2, 2, 2, 0, 2, 1, 2, 2, 2, 2, 2, 0, 2, 1],
            4,
            4,
            &palette,
            &view,
            area,
            &mut buffer,
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▐");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(51, 51, 51));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(0, 0, 0));
        assert!(render(
            &[0, 2, 1, 2],
            2,
            2,
            &palette,
            &view,
            area,
            &mut buffer
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▌");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(0, 0, 0));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(255, 255, 255));
    }

    #[test]
    fn exact_view_keeps_each_dot_and_odd_bottom_row() {
        let palette = make_palette(&["#000", "#333", "#fff"]).unwrap();
        let area = Rect::new(0, 0, 1, 2);
        let mut buffer = Buffer::empty(area);
        assert!(!render(
            &[0, 1, 2],
            1,
            3,
            &palette,
            &View {
                exact: true,
                ..View::default()
            },
            area,
            &mut buffer
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▀");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(0, 0, 0));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(51, 51, 51));
        assert_eq!(buffer[(0, 1)].fg, Color::Rgb(255, 255, 255));
        assert_eq!(buffer[(0, 1)].bg, Color::Reset);
    }
    #[test]
    fn minimap_fills_the_same_rectangle_used_for_projection() {
        let area = Rect::new(0, 0, 48, 18);
        let mut buffer = Buffer::empty(area);
        let palette = make_palette(&["#000", "#fff"]).unwrap();
        let view = View {
            exact: true,
            ..View::default()
        };
        render(
            &vec![0; 120 * 120],
            120,
            120,
            &palette,
            &view,
            area,
            &mut buffer,
        );
        // A 16x6 frame at x 32; the square grid fits 9x4 cells inside its 14x4 border.
        assert_eq!(buffer[(32, 0)].symbol(), "┌");
        assert_eq!(buffer[(47, 5)].symbol(), "┘");
        assert_eq!(buffer[(33, 2)].symbol(), " ");
        assert_eq!(buffer[(33, 2)].bg, Color::Reset);
        let (_, map) = view.minimap(120, 120, area).unwrap();
        assert_eq!(buffer[(map.x, map.y)].fg, Color::Rgb(0, 0, 0));
    }
}
