//! List selection with scroll state retained through shared borrows.

use std::cell::Cell;

use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent},
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Block, List, ListState, StatefulWidget},
};

use crate::Outcome;

/// Selection over a list of items the caller owns and passes in on each call.
///
/// The picker stores only the selected index and the scroll offset of the
/// last draw. The offset lives in a [`Cell`] so that drawing through `&self`
/// still keeps the view from jumping between frames. Because the item count
/// can change between calls, the index is clamped whenever it is used.
///
/// # Examples
///
/// ```
/// use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
/// use rthid_widgets::{ListPicker, Outcome};
///
/// let items = ["bayer", "floyd", "atkinson"];
/// let mut picker = ListPicker::new(0);
/// let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
/// assert_eq!(picker.handle_key(key(KeyCode::Char('j')), items.len()), Outcome::Consumed);
/// assert_eq!(picker.handle_key(key(KeyCode::Enter), items.len()), Outcome::Submit(1));
/// ```
pub struct ListPicker {
    selected: usize,
    offset: Cell<usize>,
}

impl ListPicker {
    /// Creates a picker with `selected` as the initial index and no scroll.
    ///
    /// An index past the end is allowed and is clamped to the last item on
    /// the next key or draw.
    ///
    /// # Examples
    ///
    /// ```
    /// use rthid_widgets::ListPicker;
    ///
    /// let picker = ListPicker::new(2);
    /// assert_eq!(picker.selected(), 2);
    /// assert_eq!(picker.offset(), 0);
    /// ```
    pub fn new(selected: usize) -> Self {
        Self {
            selected,
            offset: Cell::new(0),
        }
    }

    /// Returns the selected index as stored.
    ///
    /// Only [`ListPicker::handle_key`] clamps the stored index. Drawing clamps
    /// a copy, so this may exceed the item count until the next key.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    /// use rthid_widgets::ListPicker;
    ///
    /// let mut picker = ListPicker::new(5);
    /// assert_eq!(picker.selected(), 5);
    /// picker.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), 3);
    /// assert_eq!(picker.selected(), 1);
    /// ```
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Handles a key for a list of `len` items.
    ///
    /// The stored index is first clamped to the last item, for every key.
    /// Then:
    ///
    /// - Up or k moves up and Down or j moves down, stopping at either end,
    ///   and return `Consumed`.
    /// - Enter returns `Submit` with the selected index, or `Consumed` when
    ///   the list is empty.
    /// - Esc returns `Cancel`.
    /// - Any other key returns `Ignored`.
    ///
    /// Modifiers are not checked and there is no wrapping, paging, Home or End.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    /// use rthid_widgets::{ListPicker, Outcome};
    ///
    /// let mut picker = ListPicker::new(0);
    /// let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    /// assert_eq!(picker.handle_key(key(KeyCode::Char('k')), 3), Outcome::Consumed);
    /// assert_eq!(picker.selected(), 0);
    /// assert_eq!(picker.handle_key(key(KeyCode::Char('x')), 3), Outcome::Ignored);
    /// assert_eq!(picker.handle_key(key(KeyCode::Enter), 0), Outcome::Consumed);
    /// assert_eq!(picker.handle_key(key(KeyCode::Esc), 3), Outcome::Cancel);
    /// ```
    pub fn handle_key(&mut self, key: KeyEvent, len: usize) -> Outcome<usize> {
        let last = len.saturating_sub(1);
        self.selected = self.selected.min(last);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = self.selected.saturating_add(1).min(last)
            }
            KeyCode::Enter if len > 0 => return Outcome::Submit(self.selected),
            KeyCode::Enter => {}
            KeyCode::Esc => return Outcome::Cancel,
            _ => return Outcome::Ignored,
        }
        Outcome::Consumed
    }

    /// Forgets the retained scroll so the next draw starts from the first item.
    ///
    /// The draw still scrolls as far as needed to show the selection. Useful
    /// when the picker is reopened or its items are replaced.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::{buffer::Buffer, layout::Rect, style::Style, text::Line};
    /// use rthid_widgets::ListPicker;
    ///
    /// let area = Rect::new(0, 0, 5, 2);
    /// let mut buf = Buffer::empty(area);
    /// let picker = ListPicker::new(3);
    /// picker.render(["a", "b", "c", "d"].map(Line::raw), None, Style::default(), area, &mut buf);
    /// assert_eq!(picker.offset(), 2);
    /// picker.reset_scroll();
    /// assert_eq!(picker.offset(), 0);
    /// ```
    pub fn reset_scroll(&self) {
        self.offset.set(0);
    }

    /// Returns the index of the first visible item after the last draw, or
    /// zero before any draw or after [`ListPicker::reset_scroll`].
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::{buffer::Buffer, layout::Rect, style::Style, text::Line};
    /// use rthid_widgets::ListPicker;
    ///
    /// let area = Rect::new(0, 0, 5, 2);
    /// let mut buf = Buffer::empty(area);
    /// let picker = ListPicker::new(4);
    /// assert_eq!(picker.offset(), 0);
    /// picker.render(["a", "b", "c", "d", "e"].map(Line::raw), None, Style::default(), area, &mut buf);
    /// assert_eq!(picker.offset(), 3);
    /// ```
    pub fn offset(&self) -> usize {
        self.offset.get()
    }

    /// Draws `items` into `area`, inside `block` when given.
    ///
    /// The selected row is prefixed with `> ` and styled with `highlight`,
    /// and other rows are indented to match. The list scrolls only as far as
    /// needed to keep the selection visible, starting from the retained
    /// offset, and the new offset is stored for the next draw. An index past
    /// the end is drawn on the last item without changing the stored index,
    /// and an empty list draws no selection.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::{buffer::Buffer, layout::Rect, style::Style, text::Line};
    /// use rthid_widgets::ListPicker;
    ///
    /// let area = Rect::new(0, 0, 5, 2);
    /// let mut buf = Buffer::empty(area);
    /// let picker = ListPicker::new(2);
    /// picker.render(["a", "b", "c"].map(Line::raw), None, Style::default(), area, &mut buf);
    /// assert_eq!(buf, Buffer::with_lines(["  b  ", "> c  "]));
    /// ```
    pub fn render<'a>(
        &self,
        items: impl IntoIterator<Item = Line<'a>>,
        block: Option<Block<'a>>,
        highlight: Style,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let items: Vec<_> = items.into_iter().collect();
        let selected = if items.is_empty() {
            None
        } else {
            Some(self.selected.min(items.len() - 1))
        };
        let mut state = ListState::default()
            .with_selected(selected)
            .with_offset(self.offset.get());
        let mut list = List::new(items)
            .highlight_symbol("> ")
            .highlight_style(highlight);
        if let Some(block) = block {
            list = list.block(block);
        }
        StatefulWidget::render(list, area, buf, &mut state);
        self.offset.set(state.offset());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn navigates_clamps_and_ignores_other_keys() {
        let mut picker = ListPicker::new(0);
        for code in [KeyCode::Up, KeyCode::Char('k')] {
            assert_eq!(picker.handle_key(key(code), 3), Outcome::Consumed);
            assert_eq!(picker.selected(), 0);
        }
        for code in [KeyCode::Down, KeyCode::Char('j'), KeyCode::Down] {
            assert_eq!(picker.handle_key(key(code), 3), Outcome::Consumed);
        }
        assert_eq!(picker.selected(), 2);
        assert_eq!(
            picker.handle_key(key(KeyCode::Enter), 1),
            Outcome::Submit(0)
        );
        for code in [
            KeyCode::Char('h'),
            KeyCode::Char('l'),
            KeyCode::Char('a'),
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Home,
            KeyCode::End,
        ] {
            assert_eq!(picker.handle_key(key(code), 3), Outcome::Ignored);
        }
        assert_eq!(picker.handle_key(key(KeyCode::Enter), 0), Outcome::Consumed);
        assert_eq!(picker.selected(), 0);
        assert_eq!(picker.handle_key(key(KeyCode::Esc), 0), Outcome::Cancel);
    }

    #[test]
    fn retains_render_offset_and_resets_it() {
        let mut picker = ListPicker::new(4);
        let area = Rect::new(0, 0, 5, 2);
        let mut buf = Buffer::empty(area);
        let draw = |picker: &ListPicker, buf: &mut Buffer| {
            picker.render(
                ["a", "b", "c", "d", "e"].map(Line::raw),
                None,
                Style::default(),
                area,
                buf,
            );
        };
        draw(&picker, &mut buf);
        assert_eq!(picker.offset(), 3);
        assert_eq!(buf, Buffer::with_lines(["  d  ", "> e  "]));
        picker.handle_key(key(KeyCode::Up), 5);
        draw(&picker, &mut buf);
        assert_eq!(picker.offset(), 3);
        picker.reset_scroll();
        assert_eq!(picker.offset(), 0);
        picker.handle_key(key(KeyCode::Enter), 1);
        draw(&picker, &mut buf);
        assert_eq!(picker.offset(), 0);
        picker.render([], None, Style::default(), area, &mut buf);
        assert_eq!(picker.offset(), 0);
    }
}
