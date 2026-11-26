use std::io::{self, Write};

use ratatui::{
    backend::{Backend, ClearType, CrosstermBackend, IntoCrossterm, WindowSize},
    buffer::Cell,
    crossterm::{
        cursor::MoveTo,
        queue,
        style::{
            Attribute, Color as TerminalColor, Colors, ContentStyle, Print, SetAttribute,
            SetAttributes, SetBackgroundColor, SetColors, SetForegroundColor, SetUnderlineColor,
        },
    },
    layout::{Position, Size},
    style::{Color, Modifier, Style},
};

/// A Crossterm backend that elides unchanged color channels to reduce frame output.
///
/// Foreground changes are also skipped for single spaces unless reversed, underlined or crossed
/// out. Other output semantics, including frame resets, match [`CrosstermBackend`].
pub struct ElidingBackend<W: Write> {
    inner: CrosstermBackend<W>,
}

impl<W: Write> ElidingBackend<W> {
    /// Creates a backend that writes frames and terminal commands through `writer`.
    pub const fn new(writer: W) -> Self {
        Self {
            inner: CrosstermBackend::new(writer),
        }
    }
}

impl<W: Write> Write for ElidingBackend<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.inner.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.inner)
    }
}

impl<W: Write> Backend for ElidingBackend<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut foreground = Color::Reset;
        let mut background = Color::Reset;
        let mut underline = Color::Reset;
        let mut modifiers = Modifier::empty();
        let mut previous: Option<(u16, u16)> = None;
        for (x, y, cell) in content {
            if !matches!(previous, Some((px, py)) if px.checked_add(1) == Some(x) && py == y) {
                queue!(self.inner, MoveTo(x, y))?;
            }
            previous = Some((x, y));
            if modifiers != cell.modifier {
                change_modifiers(&mut self.inner, modifiers, cell.modifier)?;
                modifiers = cell.modifier;
            }
            let fg_changed = foreground != cell.fg && needs_foreground(cell);
            let bg_changed = background != cell.bg;
            match (fg_changed, bg_changed) {
                (true, true) => queue!(
                    self.inner,
                    SetColors(Colors::new(
                        cell.fg.into_crossterm(),
                        cell.bg.into_crossterm()
                    ))
                )?,
                (true, false) => {
                    queue!(self.inner, SetForegroundColor(cell.fg.into_crossterm()))?;
                }
                (false, true) => {
                    queue!(self.inner, SetBackgroundColor(cell.bg.into_crossterm()))?;
                }
                (false, false) => {}
            }
            // A skipped space must not change the foreground remembered for later glyphs.
            if fg_changed {
                foreground = cell.fg;
            }
            background = cell.bg;
            if underline != cell.underline_color {
                queue!(
                    self.inner,
                    SetUnderlineColor(cell.underline_color.into_crossterm())
                )?;
                underline = cell.underline_color;
            }
            queue!(self.inner, Print(cell.symbol()))?;
        }
        // SGR 0 resets colors too, so it belongs after the frame's last cell.
        queue!(
            self.inner,
            SetForegroundColor(TerminalColor::Reset),
            SetBackgroundColor(TerminalColor::Reset),
            SetUnderlineColor(TerminalColor::Reset),
            SetAttribute(Attribute::Reset)
        )
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

fn needs_foreground(cell: &Cell) -> bool {
    cell.symbol() != " "
        || cell
            .modifier
            .intersects(Modifier::REVERSED | Modifier::UNDERLINED | Modifier::CROSSED_OUT)
}

fn change_modifiers(writer: &mut impl Write, from: Modifier, to: Modifier) -> io::Result<()> {
    let removed = from - to;
    let mut added = to - from;
    for (modifier, attribute) in [
        (Modifier::REVERSED, Attribute::NoReverse),
        (Modifier::BOLD | Modifier::DIM, Attribute::NormalIntensity),
        (Modifier::ITALIC, Attribute::NoItalic),
        (Modifier::UNDERLINED, Attribute::NoUnderline),
        (Modifier::CROSSED_OUT, Attribute::NotCrossedOut),
        (Modifier::HIDDEN, Attribute::NoHidden),
        (
            Modifier::SLOW_BLINK | Modifier::RAPID_BLINK,
            Attribute::NoBlink,
        ),
    ] {
        if removed.intersects(modifier) {
            queue!(writer, SetAttribute(attribute))?;
            // Intensity and blink resets clear both members, so retain either requested member.
            if matches!(attribute, Attribute::NormalIntensity | Attribute::NoBlink) {
                added |= to & modifier;
            }
        }
    }
    let style: ContentStyle = Style::new().add_modifier(added).into_crossterm();
    queue!(writer, SetAttributes(style.attributes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, layout::Rect};

    struct Screen {
        cells: Buffer,
        cursor: (u16, u16),
        pen: Cell,
    }

    impl Screen {
        fn new(area: Rect) -> Self {
            Self {
                cells: Buffer::empty(area),
                cursor: (0, 0),
                pen: Cell::default(),
            }
        }

        fn replay(&mut self, bytes: &[u8]) {
            let mut text = std::str::from_utf8(bytes).unwrap();
            while !text.is_empty() {
                if let Some(sequence) = text.strip_prefix("\x1b[") {
                    let end = sequence.find(|c: char| ('@'..='~').contains(&c)).unwrap();
                    let values: Vec<u16> = sequence[..end]
                        .split(';')
                        .map(|value| value.parse().unwrap())
                        .collect();
                    match sequence.as_bytes()[end] {
                        b'H' => self.cursor = (values[1] - 1, values[0] - 1),
                        b'm' => self.sgr(&values),
                        command => panic!("unexpected CSI command {command}"),
                    }
                    text = &sequence[end + 1..];
                } else {
                    let symbol = text.chars().next().unwrap();
                    self.pen.set_char(symbol);
                    self.cells[self.cursor] = self.pen.clone();
                    self.cursor.0 += 1;
                    text = &text[symbol.len_utf8()..];
                }
            }
        }

        fn sgr(&mut self, mut values: &[u16]) {
            while let Some((&code, rest)) = values.split_first() {
                values = rest;
                match code {
                    0 => self.pen = Cell::default(),
                    1 => self.pen.modifier.insert(Modifier::BOLD),
                    2 => self.pen.modifier.insert(Modifier::DIM),
                    3 => self.pen.modifier.insert(Modifier::ITALIC),
                    4 => self.pen.modifier.insert(Modifier::UNDERLINED),
                    5 => self.pen.modifier.insert(Modifier::SLOW_BLINK),
                    6 => self.pen.modifier.insert(Modifier::RAPID_BLINK),
                    7 => self.pen.modifier.insert(Modifier::REVERSED),
                    8 => self.pen.modifier.insert(Modifier::HIDDEN),
                    9 => self.pen.modifier.insert(Modifier::CROSSED_OUT),
                    22 => self.pen.modifier.remove(Modifier::BOLD | Modifier::DIM),
                    23 => self.pen.modifier.remove(Modifier::ITALIC),
                    24 => self.pen.modifier.remove(Modifier::UNDERLINED),
                    25 => self
                        .pen
                        .modifier
                        .remove(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK),
                    27 => self.pen.modifier.remove(Modifier::REVERSED),
                    28 => self.pen.modifier.remove(Modifier::HIDDEN),
                    29 => self.pen.modifier.remove(Modifier::CROSSED_OUT),
                    39 => self.pen.fg = Color::Reset,
                    49 => self.pen.bg = Color::Reset,
                    59 => self.pen.underline_color = Color::Reset,
                    38 | 48 | 58 => {
                        let (color, count) = match values {
                            [5, index, ..] => (Color::Indexed(*index as u8), 2),
                            [2, r, g, b, ..] => (Color::Rgb(*r as u8, *g as u8, *b as u8), 4),
                            _ => panic!("unexpected color {values:?}"),
                        };
                        values = &values[count..];
                        match code {
                            38 => self.pen.fg = color,
                            48 => self.pen.bg = color,
                            58 => self.pen.underline_color = color,
                            _ => unreachable!(),
                        }
                    }
                    _ => panic!("unexpected SGR {code}"),
                }
            }
        }

        fn assert_same(&self, other: &Self) {
            for (index, (expected, actual)) in self
                .cells
                .content
                .iter()
                .zip(&other.cells.content)
                .enumerate()
            {
                assert_eq!(actual.symbol(), expected.symbol(), "symbol at {index}");
                assert_eq!(actual.bg, expected.bg, "background at {index}");
                assert_eq!(actual.modifier, expected.modifier, "modifiers at {index}");
                if needs_foreground(expected) {
                    assert_eq!(actual.fg, expected.fg, "foreground at {index}");
                }
                assert_eq!(
                    actual.underline_color, expected.underline_color,
                    "underline at {index}"
                );
            }
            assert_eq!(self.cursor, other.cursor);
            for pen in [&self.pen, &other.pen] {
                assert_eq!(pen.fg, Color::Reset);
                assert_eq!(pen.bg, Color::Reset);
                assert_eq!(pen.underline_color, Color::Reset);
                assert!(pen.modifier.is_empty());
            }
        }
    }

    #[test]
    fn frames_preserve_visible_cells_and_reset_styles() {
        ratatui::crossterm::style::Colored::set_ansi_color_disabled(false);
        let area = Rect::new(0, 0, 32, 16);
        let mut first = Buffer::empty(area);
        let colors = [Color::Rgb(12, 34, 56), Color::Indexed(201), Color::Reset];
        for (i, cell) in first.content.iter_mut().enumerate() {
            cell.set_symbol(if i % 3 == 0 { " " } else { "▀" });
            cell.fg = colors[i % colors.len()];
            cell.bg = colors[(i / 2) % colors.len()];
            cell.underline_color = colors[(i / 3) % colors.len()];
            cell.modifier = Modifier::from_bits_truncate(i as u16);
        }
        let mut second = first.clone();
        for (i, cell) in second.content.iter_mut().enumerate() {
            if i % 3 != 1 {
                cell.set_symbol(if i % 2 == 0 { "x" } else { " " });
                cell.fg = colors[(i + 1) % colors.len()];
                cell.bg = colors[(i + 2) % colors.len()];
                cell.modifier = Modifier::from_bits_truncate(511 - i as u16);
            }
        }
        let mut expected = Screen::new(area);
        let mut actual = Screen::new(area);
        let mut previous = Buffer::empty(area);
        let mut standard_bytes = Vec::new();
        let mut elided_bytes = Vec::new();
        for next in [&first, &second, &second, &Buffer::empty(area)] {
            standard_bytes.clear();
            elided_bytes.clear();
            // Separate draws avoid losing retained blink attributes in the standard backend.
            for cell in previous.diff(next) {
                CrosstermBackend::new(&mut standard_bytes)
                    .draw(std::iter::once(cell))
                    .unwrap();
            }
            ElidingBackend::new(&mut elided_bytes)
                .draw(previous.diff(next).into_iter())
                .unwrap();
            expected.replay(&standard_bytes);
            actual.replay(&elided_bytes);
            expected.assert_same(&actual);
            previous = next.clone();
        }
    }

    #[test]
    fn removing_one_blink_preserves_the_other() {
        ratatui::crossterm::style::Colored::set_ansi_color_disabled(false);
        let area = Rect::new(0, 0, 2, 1);
        let mut frame = Buffer::empty(area);
        frame[(0, 0)].set_symbol("x");
        frame[(0, 0)].modifier = Modifier::SLOW_BLINK | Modifier::RAPID_BLINK;
        frame[(1, 0)].set_symbol("x");
        frame[(1, 0)].modifier = Modifier::RAPID_BLINK;
        let mut standard_bytes = Vec::new();
        // Separate draws reset styles so each expected cell has its requested blink attributes.
        for x in 0..2 {
            CrosstermBackend::new(&mut standard_bytes)
                .draw(std::iter::once((x, 0, &frame[(x, 0)])))
                .unwrap();
        }
        let mut elided_bytes = Vec::new();
        ElidingBackend::new(&mut elided_bytes)
            .draw(Buffer::empty(area).diff(&frame).into_iter())
            .unwrap();
        let mut expected = Screen::new(area);
        let mut actual = Screen::new(area);
        expected.replay(&standard_bytes);
        actual.replay(&elided_bytes);
        expected.assert_same(&actual);
    }

    #[test]
    fn plain_spaces_neither_emit_nor_replace_the_remembered_foreground() {
        ratatui::crossterm::style::Colored::set_ansi_color_disabled(false);
        let mut frame = Buffer::empty(Rect::new(0, 0, 3, 1));
        for cell in &mut frame.content {
            cell.set_symbol("x");
            cell.fg = Color::Indexed(200);
        }
        frame[(1, 0)].set_symbol(" ");
        let mut expected = Vec::new();
        ElidingBackend::new(&mut expected)
            .draw(Buffer::empty(frame.area).diff(&frame).into_iter())
            .unwrap();
        frame[(1, 0)].fg = Color::Rgb(1, 2, 3);
        let mut actual = Vec::new();
        ElidingBackend::new(&mut actual)
            .draw(Buffer::empty(frame.area).diff(&frame).into_iter())
            .unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn changing_only_background_writes_fewer_bytes() {
        ratatui::crossterm::style::Colored::set_ansi_color_disabled(false);
        let mut frame = Buffer::empty(Rect::new(0, 0, 32, 1));
        for (i, cell) in frame.content.iter_mut().enumerate() {
            cell.set_symbol("▀");
            cell.fg = Color::Indexed(200);
            cell.bg = Color::Indexed(i as u8);
        }
        let mut standard = Vec::new();
        let mut elided = Vec::new();
        let content = || {
            frame
                .content
                .iter()
                .enumerate()
                .map(|(x, cell)| (x as u16, 0, cell))
        };
        CrosstermBackend::new(&mut standard)
            .draw(content())
            .unwrap();
        ElidingBackend::new(&mut elided).draw(content()).unwrap();
        assert!(
            elided.len() < standard.len(),
            "{} vs {} bytes",
            elided.len(),
            standard.len()
        );
    }
}
