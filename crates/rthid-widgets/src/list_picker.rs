//! List selection with scroll state retained through shared borrows.

use std::cell::Cell;

use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind},
    layout::{Position, Rect},
    style::Style,
    text::Line,
    widgets::{Block, List, ListState, StatefulWidget},
};

use crate::{Outcome, outcome::is_release};

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
    /// Keys and handled mouse events clamp the stored index. Drawing clamps
    /// a copy, so this may exceed the item count until the next input.
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

    /// Sets the index, clamped on the next key or draw as with [`Self::new`].
    ///
    /// # Examples
    ///
    /// ```
    /// use rthid_widgets::ListPicker;
    /// let mut picker = ListPicker::new(0);
    /// picker.select(3);
    /// assert_eq!(picker.selected(), 3);
    /// ```
    pub fn select(&mut self, index: usize) {
        self.selected = index;
    }

    /// Returns the item at a position using the last draw's scroll offset.
    ///
    /// `area` must be the list content rectangle without a block.
    /// Positions outside the area or beyond `len` return `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::layout::{Position, Rect};
    /// use rthid_widgets::ListPicker;
    /// let picker = ListPicker::new(0);
    /// assert_eq!(picker.index_at(Rect::new(2, 3, 10, 4), Position::new(2, 4), 3), Some(1));
    /// ```
    pub fn index_at(&self, area: Rect, position: Position, len: usize) -> Option<usize> {
        if !area.contains(position) {
            return None;
        }
        let index = self
            .offset
            .get()
            .saturating_add(usize::from(position.y - area.y));
        (index < len).then_some(index)
    }

    /// Selects a clicked item or submits it when already selected.
    ///
    /// Wheel events inside `area` move one item. Other events are ignored.
    /// Clicking the selected item stands in for a double click in terminals.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::{layout::Rect, crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind}};
    /// use rthid_widgets::{ListPicker, Outcome};
    /// let mut picker = ListPicker::new(0);
    /// let click = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 0, row: 1, modifiers: KeyModifiers::NONE };
    /// assert_eq!(picker.handle_mouse(click, Rect::new(0, 0, 10, 3), 3), Outcome::Consumed);
    /// assert_eq!(picker.handle_mouse(click, Rect::new(0, 0, 10, 3), 3), Outcome::Submit(1));
    /// ```
    pub fn handle_mouse(&mut self, mouse: MouseEvent, area: Rect, len: usize) -> Outcome<usize> {
        let position = Position::new(mouse.column, mouse.row);
        if !area.contains(position) {
            return Outcome::Ignored;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(index) = self.index_at(area, position, len) else {
                    return Outcome::Ignored;
                };
                if index == self.selected.min(len.saturating_sub(1)) {
                    self.select(index);
                    return Outcome::Submit(index);
                }
                self.select(index);
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.selected = self.selected.min(len.saturating_sub(1));
                self.selected = if mouse.kind == MouseEventKind::ScrollUp {
                    self.selected.saturating_sub(1)
                } else {
                    self.selected.saturating_add(1).min(len.saturating_sub(1))
                };
            }
            _ => return Outcome::Ignored,
        }
        Outcome::Consumed
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
    /// A key release returns `Ignored` before anything else, so each press
    /// moves once.
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
        if is_release(&key) {
            return Outcome::Ignored;
        }
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
    fn mouse_uses_scrolled_rows_and_ignores_empty_space() {
        let area = Rect::new(2, 3, 10, 3);
        let mut picker = ListPicker::new(4);
        picker.render(
            ["a", "b", "c", "d", "e"].map(Line::raw),
            None,
            Style::default(),
            area,
            &mut Buffer::empty(area),
        );
        assert_eq!(picker.offset(), 2);
        let mut mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 3,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(picker.handle_mouse(mouse, area, 5), Outcome::Consumed);
        assert_eq!(picker.selected(), 2);
        assert_eq!(picker.handle_mouse(mouse, area, 5), Outcome::Submit(2));
        mouse.kind = MouseEventKind::ScrollDown;
        assert_eq!(picker.handle_mouse(mouse, area, 5), Outcome::Consumed);
        assert_eq!(picker.selected(), 3);
        mouse.kind = MouseEventKind::ScrollUp;
        assert_eq!(picker.handle_mouse(mouse, area, 5), Outcome::Consumed);
        assert_eq!(picker.selected(), 2);
        mouse.column = 1;
        assert_eq!(picker.handle_mouse(mouse, area, 5), Outcome::Ignored);
        mouse.column = 2;
        mouse.row = 5;
        mouse.kind = MouseEventKind::Down(MouseButton::Left);
        assert_eq!(picker.handle_mouse(mouse, area, 3), Outcome::Ignored);
        assert_eq!(picker.handle_mouse(mouse, area, 0), Outcome::Ignored);
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
