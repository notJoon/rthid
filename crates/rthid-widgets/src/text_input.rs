//! Character based editing and a horizontally scrolled input line.

use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Position, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Widget,
};

use crate::Outcome;

/// Single line text field with a cursor and a length limit.
///
/// The limit and the cursor count Unicode scalar values (`char`), not bytes,
/// so multibyte text edits one character at a time. Drawing measures display
/// width instead, so wide characters take two columns on screen.
///
/// # Examples
///
/// ```
/// use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
/// use rthid_widgets::{Outcome, TextInput};
///
/// let mut input = TextInput::new("café", 16);
/// let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
/// input.handle_key(key(KeyCode::Backspace));
/// input.handle_key(key(KeyCode::Char('e')));
/// assert_eq!(input.handle_key(key(KeyCode::Enter)), Outcome::Submit(()));
/// assert_eq!(input.text(), "cafe");
/// ```
pub struct TextInput {
    text: String,
    cursor: usize,
    max: usize,
}

impl TextInput {
    /// Creates a field holding at most `max` characters of `text`, with the
    /// cursor after the last character.
    ///
    /// # Examples
    ///
    /// ```
    /// use rthid_widgets::TextInput;
    ///
    /// assert_eq!(TextInput::new("é界ab", 3).text(), "é界a");
    /// ```
    pub fn new(text: impl Into<String>, max: usize) -> Self {
        let text: String = text.into().chars().take(max).collect();
        Self {
            cursor: text.chars().count(),
            text,
            max,
        }
    }

    /// Returns the current value.
    ///
    /// Submitting does not clear the field, so the caller reads the value
    /// here after `Submit` and can keep the field open if it is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    /// use rthid_widgets::TextInput;
    ///
    /// let mut input = TextInput::new("warm", 16);
    /// input.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    /// assert_eq!(input.text(), "warm");
    /// ```
    pub fn text(&self) -> &str {
        &self.text
    }

    fn byte_cursor(&self) -> usize {
        self.text
            .char_indices()
            .nth(self.cursor)
            .map_or(self.text.len(), |(byte, _)| byte)
    }

    /// Applies an editing key and reports whether the field used it.
    ///
    /// - A printable character without Ctrl or Alt is inserted at the cursor.
    ///   Shift is allowed. At the limit the character is dropped but the key
    ///   still returns `Consumed`.
    /// - Backspace deletes before the cursor and Delete deletes at it.
    /// - Left, Right, Home and End move the cursor within the text.
    /// - All of the above return `Consumed`, even when nothing changes.
    /// - Enter returns `Submit(())` and Esc returns `Cancel`.
    /// - Any other key, including Tab, Up, Down, control characters and
    ///   characters with Ctrl or Alt, returns `Ignored`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    /// use rthid_widgets::{Outcome, TextInput};
    ///
    /// let mut input = TextInput::new("ab", 3);
    /// let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    /// input.handle_key(key(KeyCode::Home));
    /// assert_eq!(input.handle_key(key(KeyCode::Char('x'))), Outcome::Consumed);
    /// assert_eq!(input.handle_key(key(KeyCode::Char('y'))), Outcome::Consumed);
    /// assert_eq!(input.text(), "xab");
    /// let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
    /// assert_eq!(input.handle_key(ctrl_a), Outcome::Ignored);
    /// ```
    pub fn handle_key(&mut self, key: KeyEvent) -> Outcome<()> {
        match key.code {
            KeyCode::Char(c)
                if !c.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if self.text.chars().count() < self.max {
                    self.text.insert(self.byte_cursor(), c);
                    self.cursor += 1;
                }
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.text.remove(self.byte_cursor());
                }
            }
            KeyCode::Delete => {
                let byte = self.byte_cursor();
                if byte < self.text.len() {
                    self.text.remove(byte);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.text.chars().count()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.chars().count(),
            KeyCode::Enter => return Outcome::Submit(()),
            KeyCode::Esc => return Outcome::Cancel,
            _ => return Outcome::Ignored,
        }
        Outcome::Consumed
    }

    /// Draws `prefix` unstyled followed by the text in `style` on the first
    /// row of `area`, and returns where the terminal cursor belongs.
    ///
    /// When the line is wider than `area`, it scrolls left just enough to keep
    /// the cursor cell inside the last column. Characters cut by either edge
    /// are skipped rather than split, and cells without text are not cleared.
    /// The returned position is in buffer coordinates, ready for
    /// `Frame::set_cursor_position`. An empty `area` draws nothing and
    /// returns `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::{buffer::Buffer, layout::{Position, Rect}, style::Style};
    /// use rthid_widgets::TextInput;
    ///
    /// let area = Rect::new(0, 0, 6, 1);
    /// let mut buf = Buffer::empty(area);
    /// let input = TextInput::new("hello", 16);
    /// let cursor = input.render("> ", Style::default(), area, &mut buf);
    /// // "> hello" plus the cursor needs 8 columns, so two scroll away.
    /// assert_eq!(cursor, Some(Position::new(5, 0)));
    /// assert_eq!(buf, Buffer::with_lines(["hello "]));
    /// ```
    pub fn render(
        &self,
        prefix: &str,
        style: Style,
        area: Rect,
        buf: &mut Buffer,
    ) -> Option<Position> {
        if area.is_empty() {
            return None;
        }
        let cursor =
            Line::raw(prefix).width() + Line::raw(&self.text[..self.byte_cursor()]).width();
        let scroll = cursor.saturating_sub(usize::from(area.width) - 1);
        let line = Line::from(vec![Span::raw(prefix), Span::styled(&self.text, style)]);
        let mut column = 0;
        for grapheme in line.styled_graphemes(Style::default()) {
            let span = Span::styled(grapheme.symbol, grapheme.style);
            let width = span.width();
            let end = column + width;
            if column >= scroll {
                let x = column - scroll;
                if x + width > usize::from(area.width) {
                    break;
                }
                span.render(Rect::new(area.x + x as u16, area.y, width as u16, 1), buf);
            }
            column = end;
        }
        Some(Position::new(area.x + (cursor - scroll) as u16, area.y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn edits_multibyte_characters_with_a_character_limit() {
        let mut input = TextInput::new("é界ab", 3);
        assert_eq!(input.text(), "é界a");
        assert_eq!(input.handle_key(key(KeyCode::Char('x'))), Outcome::Consumed);
        assert_eq!(input.text(), "é界a");
        for code in [KeyCode::Left, KeyCode::Backspace, KeyCode::Char('猫')] {
            assert_eq!(input.handle_key(key(code)), Outcome::Consumed);
        }
        assert_eq!(input.text(), "é猫a");
        input.handle_key(key(KeyCode::Home));
        input.handle_key(key(KeyCode::Delete));
        input.handle_key(KeyEvent::new(KeyCode::Char('É'), KeyModifiers::SHIFT));
        assert_eq!(input.text(), "É猫a");
        input.handle_key(key(KeyCode::End));
        input.handle_key(key(KeyCode::Right));
        input.handle_key(key(KeyCode::Backspace));
        assert_eq!(input.text(), "É猫");
        for code in [KeyCode::Home, KeyCode::Left, KeyCode::Backspace] {
            assert_eq!(input.handle_key(key(code)), Outcome::Consumed);
        }
        let mut empty = TextInput::new("", 0);
        for code in [KeyCode::Delete, KeyCode::Right, KeyCode::Char('a')] {
            assert_eq!(empty.handle_key(key(code)), Outcome::Consumed);
        }
        assert_eq!(empty.text(), "");
    }

    #[test]
    fn submits_cancels_and_ignores_unowned_keys() {
        let mut input = TextInput::new("keep", 10);
        assert_eq!(input.handle_key(key(KeyCode::Enter)), Outcome::Submit(()));
        assert_eq!(input.handle_key(key(KeyCode::Esc)), Outcome::Cancel);
        for event in [
            key(KeyCode::Tab),
            key(KeyCode::Char('\n')),
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT),
        ] {
            assert_eq!(input.handle_key(event), Outcome::Ignored);
        }
        assert_eq!(input.text(), "keep");
    }

    #[test]
    fn scrolls_by_display_width_and_returns_buffer_coordinates() {
        let mut input = TextInput::new("é界ab", 10);
        let style = Style::default().fg(Color::Green);
        let area = Rect::new(2, 1, 5, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 3));
        assert_eq!(
            input.render("> ", style, area, &mut buf),
            Some(Position::new(6, 1))
        );
        let mut expected = Buffer::with_lines(["        ", "  界ab  ", "        "]);
        expected[(2, 1)].set_style(style);
        expected[(4, 1)].set_style(style);
        expected[(5, 1)].set_style(style);
        assert_eq!(buf, expected);
        input.handle_key(key(KeyCode::Home));
        let mut buf = Buffer::empty(area);
        assert_eq!(
            input.render("> ", style, area, &mut buf),
            Some(Position::new(4, 1))
        );
        assert_eq!(buf[(2, 1)].style().fg, Some(Color::Reset));
        assert_eq!(buf[(4, 1)].style().fg, Some(Color::Green));
        assert_eq!(input.render("", style, Rect::ZERO, &mut buf), None);
        let mut narrow = Buffer::empty(Rect::new(0, 0, 1, 1));
        input.handle_key(key(KeyCode::End));
        assert_eq!(
            input.render("", style, narrow.area, &mut narrow),
            Some(Position::new(0, 0))
        );
    }
}
