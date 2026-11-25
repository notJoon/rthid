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

/// Keeps selection and the last rendered scroll offset.
pub struct ListPicker {
    selected: usize,
    offset: Cell<usize>,
}

impl ListPicker {
    /// Starts at the supplied index, clamped when items are handled or drawn.
    pub fn new(selected: usize) -> Self {
        Self {
            selected,
            offset: Cell::new(0),
        }
    }

    /// Returns the index last updated by input handling.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Clamps selection to the current items before handling navigation.
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

    /// Starts the next draw from the top before keeping selection visible.
    pub fn reset_scroll(&self) {
        self.offset.set(0);
    }

    /// Returns the offset retained by the last draw.
    pub fn offset(&self) -> usize {
        self.offset.get()
    }

    /// Draws with a selection marker and retains ratatui's adjusted offset.
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
