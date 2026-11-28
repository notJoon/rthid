//! List selection with scroll state retained through shared borrows.

use std::cell::Cell;

use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    layout::{Position, Rect},
    style::Style,
    text::Line,
    widgets::{
        Block, List, ListState, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget,
        Widget,
    },
};

use crate::{Hint, Outcome, outcome::is_release};

/// Selection over a list of items the caller owns and passes in on each call.
///
/// The picker stores the selected index, scroll offset, and content area of the
/// last draw. The offset and area live in [`Cell`]s so that drawing through
/// `&self` still keeps the view from jumping between frames and mouse input
/// needs no repeated layout arguments. Because the item count can change
/// between calls, selection queries and drawing clamp to its current length.
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
    area: Cell<Rect>,
    rendered_len: Cell<usize>,
}

impl ListPicker {
    /// Up, Down, k and j move the selection.
    ///
    /// ```
    /// use rthid_widgets::{ListPicker, hint_line};
    ///
    /// let line = hint_line(&[ListPicker::MOVE, ListPicker::SUBMIT], None, 80);
    /// assert_eq!(line.to_string(), "Up/Down k/j: select | Enter: choose");
    /// ```
    pub const MOVE: Hint = Hint::new("Up/Down k/j", "select");

    /// Enter chooses the selected item when the list is non-empty.
    pub const SUBMIT: Hint = Hint::new("Enter", "choose");

    /// Esc closes the picker.
    pub const CANCEL: Hint = Hint::new("Esc", "close");

    /// Clicking a row selects it.
    pub const CLICK: Hint = Hint::new("click", "select");

    /// Clicking the selected row chooses it.
    pub const CLICK_AGAIN: Hint = Hint::new("click again", "choose");

    /// Scrolling the wheel moves the selection.
    pub const WHEEL: Hint = Hint::new("wheel", "move");

    /// Creates a picker with `selected` as the initial index and no scroll.
    ///
    /// An index past the end is allowed and is clamped to the last item when
    /// queried, handled or drawn.
    ///
    /// # Examples
    ///
    /// ```
    /// use rthid_widgets::ListPicker;
    ///
    /// let picker = ListPicker::new(2);
    /// assert_eq!(picker.selected(5), Some(2));
    /// assert_eq!(picker.offset(), 0);
    /// ```
    pub fn new(selected: usize) -> Self {
        Self {
            selected,
            offset: Cell::new(0),
            area: Cell::new(Rect::ZERO),
            rendered_len: Cell::new(0),
        }
    }

    /// Returns the selected index clamped to the current list length.
    ///
    /// An empty list has no selection. This matches the highlighted row when
    /// drawing the same list, even if items were removed since the last input.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    /// use rthid_widgets::ListPicker;
    ///
    /// let mut picker = ListPicker::new(5);
    /// assert_eq!(picker.selected(3), Some(2));
    /// picker.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), 3);
    /// assert_eq!(picker.selected(3), Some(1));
    /// ```
    pub fn selected(&self, len: usize) -> Option<usize> {
        len.checked_sub(1).map(|last| self.selected.min(last))
    }

    /// Returns the selected screen row using the last draw's scroll offset.
    ///
    /// Pass the same outer area and block as drawing. Returns `None` for an
    /// empty list or a selection outside the visible content area.
    pub fn selected_row(&self, area: Rect, block: Option<&Block<'_>>, len: usize) -> Option<Rect> {
        let (area, _) = content_area(area, block, len);
        let row = self.selected(len)?.checked_sub(self.offset.get())?;
        (area.width > 0 && row < usize::from(area.height))
            .then(|| Rect::new(area.x, area.y + row as u16, area.width, 1))
    }

    /// Ignores mouse input until the next render after rows move.
    pub fn invalidate(&mut self) {
        self.area.set(Rect::ZERO);
    }

    /// Sets the preferred index, clamped when queried, handled or drawn.
    ///
    /// # Examples
    ///
    /// ```
    /// use rthid_widgets::ListPicker;
    /// let mut picker = ListPicker::new(0);
    /// picker.select(3);
    /// assert_eq!(picker.selected(5), Some(3));
    /// ```
    pub fn select(&mut self, index: usize) {
        self.selected = index;
    }

    /// Returns the item at a position using the last draw's scroll offset.
    ///
    /// Pass the same block and outer area as drawing. Positions outside the
    /// content area or beyond `len` return `None`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::layout::{Position, Rect};
    /// use rthid_widgets::ListPicker;
    /// let picker = ListPicker::new(0);
    /// assert_eq!(picker.index_at(Position::new(2, 4), None, Rect::new(2, 3, 10, 4), 3), Some(1));
    /// ```
    pub fn index_at(
        &self,
        position: Position,
        block: Option<&Block<'_>>,
        area: Rect,
        len: usize,
    ) -> Option<usize> {
        self.row_at(position, content_area(area, block, len).0, len)
    }

    /// Uses the content area and scroll offset remembered by the last draw.
    pub(crate) fn rendered_index_at(&self, position: Position, len: usize) -> Option<usize> {
        self.row_at(position, self.area.get(), len)
    }

    fn row_at(&self, position: Position, content: Rect, len: usize) -> Option<usize> {
        if !content.contains(position) {
            return None;
        }
        let index = self
            .offset
            .get()
            .saturating_add(usize::from(position.y - content.y));
        (index < len).then_some(index)
    }

    /// Selects a clicked item or submits it when already selected.
    ///
    /// Uses the content area remembered by the last draw. Before the first draw,
    /// clicks and wheel events are ignored, as are counts changed since drawing.
    /// Wheel events inside the content area move one item.
    /// Borders and other events are ignored.
    /// Clicking the selected item stands in for a double click in terminals.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::{buffer::Buffer, layout::Rect, style::Style, text::Line, crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind}};
    /// use rthid_widgets::{ListPicker, Outcome};
    /// let mut picker = ListPicker::new(0);
    /// let area = Rect::new(0, 0, 10, 3);
    /// picker.render(["a", "b", "c"].map(Line::raw), None, Style::default(), area, &mut Buffer::empty(area));
    /// let click = MouseEvent { kind: MouseEventKind::Down(MouseButton::Left), column: 0, row: 1, modifiers: KeyModifiers::NONE };
    /// assert_eq!(picker.handle_mouse(click, 3), Outcome::Consumed);
    /// assert_eq!(picker.handle_mouse(click, 3), Outcome::Submit(1));
    /// ```
    pub fn handle_mouse(&mut self, mouse: MouseEvent, len: usize) -> Outcome<usize> {
        if self.rendered_len.get() != len {
            return Outcome::Ignored;
        }
        let area = self.area.get();
        let position = Position::new(mouse.column, mouse.row);
        if !area.contains(position) {
            return Outcome::Ignored;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(index) = self.row_at(position, area, len) else {
                    return Outcome::Ignored;
                };
                if Some(index) == self.selected(len) {
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
    /// For keys without Ctrl or Alt, the stored index is clamped to the last item.
    /// Then:
    ///
    /// - Up or k moves up and Down or j moves down, stopping at either end,
    ///   and return `Consumed`.
    /// - Enter returns `Submit` with the selected index, or `Consumed` when
    ///   the list is empty.
    /// - Esc returns `Cancel`.
    /// - Any other key returns `Ignored`.
    ///
    /// Key releases return `Consumed` without changing selection. Keys with Ctrl
    /// or Alt return `Ignored`. Shift is allowed. There is no wrapping, paging,
    /// Home or End.
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
    /// assert_eq!(picker.selected(3), Some(0));
    /// assert_eq!(picker.handle_key(key(KeyCode::Char('x')), 3), Outcome::Ignored);
    /// assert_eq!(picker.handle_key(key(KeyCode::Enter), 0), Outcome::Consumed);
    /// assert_eq!(picker.handle_key(key(KeyCode::Esc), 3), Outcome::Cancel);
    /// ```
    pub fn handle_key(&mut self, key: KeyEvent, len: usize) -> Outcome<usize> {
        if is_release(&key) {
            return Outcome::Consumed;
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
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
    /// and an empty list draws no selection. When the items do not fit, a
    /// vertical scrollbar is drawn on the block's right border, or in the
    /// area's last column when there is no block.
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
        self.render_with_highlight_symbol(items, block, highlight, area, buf, "> ");
    }

    pub(crate) fn render_with_highlight_symbol<'a>(
        &self,
        items: impl IntoIterator<Item = Line<'a>>,
        block: Option<Block<'a>>,
        highlight: Style,
        area: Rect,
        buf: &mut Buffer,
        highlight_symbol: &'static str,
    ) {
        let items: Vec<_> = items.into_iter().collect();
        let len = items.len();
        let (list_area, scrollbar_area) = content_area(area, block.as_ref(), len);
        self.area.set(list_area);
        self.rendered_len.set(len);
        let selected = self.selected(len);
        let mut state = ListState::default()
            .with_selected(selected)
            .with_offset(self.offset.get());
        let list = List::new(items)
            .highlight_symbol(highlight_symbol)
            .highlight_style(highlight);
        if let Some(block) = block {
            Widget::render(block, area, buf);
        }
        StatefulWidget::render(list, list_area, buf, &mut state);
        self.offset.set(state.offset());
        if let Some(scrollbar_area) = scrollbar_area {
            let viewport = usize::from(list_area.height);
            let scroll_positions = len.saturating_sub(viewport).saturating_add(1);
            let mut scrollbar_state = ScrollbarState::new(scroll_positions)
                .position(state.offset())
                .viewport_content_length(viewport);
            StatefulWidget::render(
                Scrollbar::new(ScrollbarOrientation::VerticalRight),
                scrollbar_area,
                buf,
                &mut scrollbar_state,
            );
        }
    }
}

fn content_area(area: Rect, block: Option<&Block<'_>>, len: usize) -> (Rect, Option<Rect>) {
    let mut content = block.map_or(area, |block| block.inner(area));
    let scrollbar = len > usize::from(content.height) && content.height > 2 && area.width > 0;
    if !scrollbar {
        return (content, None);
    }
    let scrollbar_area = Rect::new(area.right() - 1, content.y, 1, content.height);
    if content.right() >= area.right() {
        content.width = content.width.saturating_sub(1);
    }
    (content, Some(scrollbar_area))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        Terminal, backend::TestBackend, crossterm::event::KeyModifiers, style::Color,
        widgets::Borders,
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn changed_counts_and_invalidated_rows_ignore_mouse_until_rendered() {
        let area = Rect::new(0, 0, 10, 4);
        let mut picker = ListPicker::new(0);
        let render = |picker: &ListPicker, len| {
            picker.render(
                vec![Line::raw("item"); len],
                None,
                Style::default(),
                area,
                &mut Buffer::empty(area),
            )
        };
        render(&picker, 2);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 1,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(picker.handle_mouse(click, 3), Outcome::Ignored);
        render(&picker, 3);
        assert_eq!(picker.handle_mouse(click, 3), Outcome::Submit(0));
        picker.invalidate();
        assert_eq!(picker.handle_mouse(click, 3), Outcome::Ignored);
        render(&picker, 3);
        assert_eq!(picker.handle_mouse(click, 3), Outcome::Submit(0));
    }

    #[test]
    fn mouse_uses_block_content_and_ignores_border() {
        let area = Rect::new(2, 3, 10, 4);
        let block = Block::bordered();
        let mut picker = ListPicker::new(1);
        let mut buf = Buffer::empty(area);
        picker.render(
            ["a", "b"].map(Line::raw),
            Some(block.clone()),
            Style::default(),
            area,
            &mut buf,
        );
        assert_eq!(buf[(area.x + 3, area.y + 1)].symbol(), "a");
        let mut mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x + 3,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(
            picker.index_at(
                Position::new(mouse.column, mouse.row),
                Some(&block),
                area,
                2
            ),
            None
        );
        assert_eq!(picker.handle_mouse(mouse, 2), Outcome::Ignored);
        assert_eq!(picker.selected(2), Some(1));
        mouse.row += 1;
        assert_eq!(
            picker.index_at(
                Position::new(mouse.column, mouse.row),
                Some(&block),
                area,
                2
            ),
            Some(0)
        );
        assert_eq!(picker.handle_mouse(mouse, 2), Outcome::Consumed);
        assert_eq!(picker.selected(2), Some(0));
    }

    #[test]
    fn mouse_is_ignored_until_a_nonempty_area_is_rendered() {
        let mut picker = ListPicker::new(0);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        assert_eq!(picker.handle_mouse(click, 1), Outcome::Ignored);
    }

    #[test]
    fn scrollbar_is_absent_when_items_fit() {
        let area = Rect::new(0, 0, 8, 5);
        let picker = ListPicker::new(0);
        let mut plain = Buffer::empty(area);
        picker.render(
            ["a", "b"].map(Line::raw),
            None,
            Style::default(),
            area,
            &mut plain,
        );
        assert_eq!(plain[(area.right() - 1, area.y)].symbol(), " ");

        let mut bordered = Buffer::empty(area);
        picker.render(
            ["a", "b"].map(Line::raw),
            Some(Block::bordered()),
            Style::default(),
            area,
            &mut bordered,
        );
        assert_eq!(bordered[(area.right() - 1, area.y + 1)].symbol(), "│");
    }

    #[test]
    fn scrollbar_tracks_first_and_last_offsets_without_covering_list_text() {
        let cases = [
            (None, Rect::new(0, 0, 8, 5), 0, 7),
            (Some(Block::bordered()), Rect::new(0, 0, 10, 7), 1, 9),
        ];
        for (block, area, text_x, scrollbar_x) in cases {
            let mut picker = ListPicker::new(0);
            let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
            terminal
                .draw(|frame| {
                    picker.render(
                        (0..8).map(|_| Line::raw("abcd")),
                        block.clone(),
                        Style::default().bg(Color::Red),
                        area,
                        frame.buffer_mut(),
                    );
                })
                .unwrap();
            assert_eq!(picker.offset(), 0);
            let buf = terminal.backend().buffer();
            let scrollbar_top = area.y + u16::from(block.is_some());
            let scrollbar_bottom = area.bottom() - u16::from(block.is_some());
            assert_eq!(
                buf[(text_x, area.y + u16::from(block.is_some()))].symbol(),
                ">"
            );
            assert_eq!(buf[(scrollbar_x, scrollbar_top + 1)].symbol(), "█");
            assert_ne!(buf[(scrollbar_x, scrollbar_top + 1)].bg, Color::Red);
            if block.is_some() {
                assert_eq!(buf[(scrollbar_x, area.y)].symbol(), "┐");
                assert_eq!(buf[(scrollbar_x, area.bottom() - 1)].symbol(), "┘");
            }

            picker.select(7);
            terminal
                .draw(|frame| {
                    picker.render(
                        (0..8).map(|_| Line::raw("abcd")),
                        block.clone(),
                        Style::default().bg(Color::Red),
                        area,
                        frame.buffer_mut(),
                    );
                })
                .unwrap();
            assert_eq!(picker.offset(), 3);
            let buf = terminal.backend().buffer();
            assert_eq!(buf[(scrollbar_x, scrollbar_bottom - 2)].symbol(), "█");
            let selected_y = area.bottom() - 1 - u16::from(block.is_some());
            assert_eq!(buf[(text_x + 2, selected_y)].symbol(), "a");
            assert_eq!(buf[(text_x + 2, selected_y)].bg, Color::Red);
        }
    }

    #[test]
    fn scrollbar_reserves_a_non_border_column_for_drawing_and_hit_testing() {
        let area = Rect::new(0, 0, 8, 6);
        let block = Block::new().borders(Borders::TOP);
        let mut picker = ListPicker::new(0);
        let mut buf = Buffer::empty(area);
        picker.render(
            (0..8).map(|_| Line::raw("abcde")),
            Some(block.clone()),
            Style::default(),
            area,
            &mut buf,
        );

        let scrollbar = Position::new(area.right() - 1, area.y + 1);
        assert_eq!(buf[(area.right() - 2, area.y + 1)].symbol(), "e");
        assert_eq!(buf[(scrollbar.x, scrollbar.y)].symbol(), "▲");
        assert_eq!(picker.index_at(scrollbar, Some(&block), area, 8), None);
        assert_eq!(
            picker.selected_row(area, Some(&block), 8),
            Some(Rect::new(area.x, area.y + 1, area.width - 1, 1))
        );
        assert_eq!(
            picker.handle_mouse(
                MouseEvent {
                    kind: MouseEventKind::Down(MouseButton::Left),
                    column: scrollbar.x,
                    row: scrollbar.y,
                    modifiers: KeyModifiers::NONE,
                },
                8,
            ),
            Outcome::Ignored
        );
    }

    #[test]
    fn scrollbar_tolerates_tiny_areas() {
        let picker = ListPicker::new(3);
        for area in [Rect::ZERO, Rect::new(0, 0, 1, 1)] {
            picker.render(
                ["a", "b", "c", "d"].map(Line::raw),
                None,
                Style::default(),
                area,
                &mut Buffer::empty(area),
            );
            picker.render(
                ["a", "b", "c", "d"].map(Line::raw),
                Some(Block::bordered()),
                Style::default(),
                area,
                &mut Buffer::empty(area),
            );
        }

        let area = Rect::new(0, 0, 6, 2);
        let mut buf = Buffer::empty(area);
        ListPicker::new(0).render(
            ["abcdef", "abcdef", "abcdef"].map(Line::raw),
            None,
            Style::default(),
            area,
            &mut buf,
        );
        assert_eq!(buf[(area.right() - 1, area.y)].symbol(), "d");
    }

    #[test]
    fn selected_row_uses_last_draw_and_excludes_hidden_selections() {
        let area = Rect::new(2, 3, 10, 4);
        let block = Block::bordered();
        let mut picker = ListPicker::new(4);
        picker.render(
            ["a", "b", "c", "d", "e"].map(Line::raw),
            Some(block.clone()),
            Style::default(),
            area,
            &mut Buffer::empty(area),
        );
        assert_eq!(picker.offset(), 3);
        assert_eq!(
            picker.selected_row(area, Some(&block), 5),
            Some(Rect::new(3, 5, 8, 1))
        );
        assert_eq!(picker.selected_row(area, Some(&block), 0), None);
        picker.select(2);
        assert_eq!(picker.selected_row(area, Some(&block), 5), None);
        picker.reset_scroll();
        assert_eq!(picker.selected_row(area, Some(&block), 5), None);
        picker.select(0);
        assert_eq!(
            picker.selected_row(area, None, 5),
            Some(Rect::new(2, 3, 9, 1))
        );
        assert_eq!(picker.selected_row(Rect::ZERO, None, 5), None);
    }

    #[test]
    fn ignores_ctrl_and_alt_but_allows_shift() {
        let mut picker = ListPicker::new(1);
        for modifiers in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ] {
            for code in [
                KeyCode::Char('k'),
                KeyCode::Down,
                KeyCode::Enter,
                KeyCode::Esc,
            ] {
                assert_eq!(
                    picker.handle_key(KeyEvent::new(code, modifiers), 3),
                    Outcome::Ignored
                );
                assert_eq!(picker.selected(3), Some(1));
            }
        }
        assert_eq!(
            picker.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT), 3),
            Outcome::Consumed
        );
        assert_eq!(picker.selected(3), Some(2));
    }

    #[test]
    fn selection_matches_drawing_after_items_are_removed() {
        let picker = ListPicker::new(4);
        let area = Rect::new(0, 0, 5, 2);
        assert_eq!(picker.selected(5), Some(4));
        let mut buf = Buffer::empty(area);
        picker.render(
            ["a", "b"].map(Line::raw),
            None,
            Style::default(),
            area,
            &mut buf,
        );
        assert_eq!(picker.selected(2), Some(1));
        assert_eq!(buf, Buffer::with_lines(["  a  ", "> b  "]));
        picker.render([], None, Style::default(), area, &mut Buffer::empty(area));
        assert_eq!(picker.selected(0), None);
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
        assert_eq!(picker.handle_mouse(mouse, 5), Outcome::Consumed);
        assert_eq!(picker.selected(5), Some(2));
        assert_eq!(picker.handle_mouse(mouse, 5), Outcome::Submit(2));
        mouse.kind = MouseEventKind::ScrollDown;
        assert_eq!(picker.handle_mouse(mouse, 5), Outcome::Consumed);
        assert_eq!(picker.selected(5), Some(3));
        mouse.kind = MouseEventKind::ScrollUp;
        assert_eq!(picker.handle_mouse(mouse, 5), Outcome::Consumed);
        assert_eq!(picker.selected(5), Some(2));
        mouse.column = 1;
        assert_eq!(picker.handle_mouse(mouse, 5), Outcome::Ignored);
        mouse.column = area.right() - 1;
        assert_eq!(picker.handle_mouse(mouse, 5), Outcome::Ignored);
        mouse.column = 2;
        mouse.row = 5;
        mouse.kind = MouseEventKind::Down(MouseButton::Left);
        assert_eq!(picker.handle_mouse(mouse, 3), Outcome::Ignored);
        assert_eq!(picker.handle_mouse(mouse, 0), Outcome::Ignored);
    }

    #[test]
    fn navigates_clamps_and_ignores_other_keys() {
        let mut picker = ListPicker::new(0);
        for code in [KeyCode::Up, KeyCode::Char('k')] {
            assert_eq!(picker.handle_key(key(code), 3), Outcome::Consumed);
            assert_eq!(picker.selected(3), Some(0));
        }
        for code in [KeyCode::Down, KeyCode::Char('j'), KeyCode::Down] {
            assert_eq!(picker.handle_key(key(code), 3), Outcome::Consumed);
        }
        assert_eq!(picker.selected(3), Some(2));
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
        assert_eq!(picker.selected(0), None);
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
