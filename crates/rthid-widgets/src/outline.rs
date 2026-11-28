//! Independently collapsible groups backed by [`ListPicker`].

use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind},
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Block,
};

use crate::{Hint, ListPicker, Outcome};

/// A row of an outline: a group header or an item, by flat item index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// A group header, by group index.
    Header(usize),
    /// An item, numbered across all groups whether they are collapsed or not.
    Item(usize),
}

/// Groups of items the caller owns, with headers that collapse independently.
///
/// The selected visible row, retained scroll offset, and last rendered content
/// area are managed by a [`ListPicker`]. Newly observed groups start expanded.
/// As with `ListPicker`, collapsing near the end can leave blank rows until the
/// cursor moves. Collapsed state follows group position (index), not group identity.
///
/// The cursor keeps its visible position when `lens` changes. After changing
/// `lens`, call [`Outline::select`] with the row to keep. The item counts passed
/// to input must match the groups passed to [`Outline::render`]; otherwise the
/// highlight and mouse hits move onto different rows, which the widget cannot
/// detect.
///
/// # Examples
///
/// ```
/// use ratatui::{
///     buffer::Buffer,
///     crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
///     layout::Rect,
///     style::Style,
///     text::Line,
/// };
/// use rthid_widgets::{Outline, Outcome};
///
/// let area = Rect::new(0, 0, 12, 4);
/// let groups = || [
///     (Line::raw("Print"), vec![Line::raw("copies")]),
///     (Line::raw("Tone"), vec![Line::raw("gamma")]),
/// ];
/// let mut outline = Outline::new(&[1, 1]);
/// let mut buffer = Buffer::empty(area);
/// outline.render(groups(), None, Style::default(), area, &mut buffer);
/// assert_eq!(buffer[(2, 1)].symbol(), "c");
///
/// outline.select(rthid_widgets::Row::Header(0), &[1, 1]);
/// let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
/// assert_eq!(outline.handle_key(left, &[1, 1]), Outcome::Consumed);
/// assert!(outline.is_collapsed(0));
/// outline.render(groups(), None, Style::default(), area, &mut buffer);
/// assert_eq!(buffer[(0, 0)].symbol(), "▸");
/// ```
pub struct Outline {
    picker: ListPicker,
    collapsed: Vec<bool>,
}

impl Outline {
    /// Right or l expands the selected group header.
    pub const EXPAND: Hint = Hint::new("Right/l", "expand");

    /// Left or h collapses the selected group header.
    pub const COLLAPSE: Hint = Hint::new("Left/h", "collapse");

    /// Enter toggles the selected group header.
    pub const TOGGLE: Hint = Hint::new("Enter", "toggle");

    const EXPANDED_HINTS: [Hint; 2] = [Self::COLLAPSE, Self::TOGGLE];
    const COLLAPSED_HINTS: [Hint; 2] = [Self::EXPAND, Self::TOGGLE];

    /// Creates an outline with every group expanded.
    ///
    /// The cursor starts on the first item when group zero has items, or the
    /// first header otherwise.
    pub fn new(lens: &[usize]) -> Self {
        Self {
            picker: ListPicker::new(usize::from(lens.first().is_some_and(|&len| len > 0))),
            collapsed: Vec::new(),
        }
    }

    /// Returns the selected visible row, clamped to the rows described by `lens`.
    ///
    /// `lens[g]` is group `g`'s item count. Items are numbered flat across
    /// groups. Returns `None` only when there are no groups. See [`Outline`]
    /// for what happens when `lens` changes.
    pub fn selected(&self, lens: &[usize]) -> Option<Row> {
        let rows = self.rows(lens);
        self.picker.selected(rows.len()).map(|index| rows[index])
    }

    /// Returns the keys owned by the selected group header.
    ///
    /// Expanded headers offer collapse and toggle; collapsed headers offer
    /// expand and toggle. Items and empty outlines return no hints.
    pub fn hints(&self, lens: &[usize]) -> &'static [Hint] {
        match self.selected(lens) {
            Some(Row::Header(group)) if self.is_collapsed(group) => &Self::COLLAPSED_HINTS,
            Some(Row::Header(_)) => &Self::EXPANDED_HINTS,
            Some(Row::Item(_)) | None => &[],
        }
    }

    /// Selects `row`, clamping an out-of-range row to the last visible row.
    ///
    /// Selecting an item inside a collapsed group expands that group first.
    pub fn select(&mut self, row: Row, lens: &[usize]) {
        self.resize(lens);
        if let Row::Item(item) = row
            && let Some(group) = item_group(item, lens)
        {
            self.collapsed[group] = false;
        }
        let rows = self.rows(lens);
        let index = rows
            .iter()
            .position(|candidate| *candidate == row)
            .unwrap_or_else(|| rows.len().saturating_sub(1));
        self.picker.select(index);
    }

    /// Returns whether `group` is collapsed.
    ///
    /// Groups not yet observed in an input length slice are expanded.
    pub fn is_collapsed(&self, group: usize) -> bool {
        self.collapsed.get(group).copied().unwrap_or(false)
    }

    /// Handles keyboard input for the rows described by `lens`.
    ///
    /// Navigation, cancellation, releases, and modifier handling match
    /// [`ListPicker::handle_key`]. Enter submits a selected item by its flat
    /// index, while Enter, Left/h, and Right/l toggle, collapse, and expand a
    /// selected header. Enter on an outline with no groups is consumed.
    pub fn handle_key(&mut self, key: KeyEvent, lens: &[usize]) -> Outcome<usize> {
        self.resize(lens);
        let rows = self.rows(lens);
        match self.picker.handle_key(key, rows.len()) {
            Outcome::Submit(index) => self.activate(rows[index]),
            Outcome::Ignored
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let Some(Row::Header(group)) = self.picker.selected(rows.len()).map(|i| rows[i])
                else {
                    return Outcome::Ignored;
                };
                match key.code {
                    KeyCode::Left | KeyCode::Char('h') => self.collapsed[group] = true,
                    KeyCode::Right | KeyCode::Char('l') => self.collapsed[group] = false,
                    _ => return Outcome::Ignored,
                }
                Outcome::Consumed
            }
            outcome => outcome,
        }
    }

    /// Returns the row under a cell using the last render's content area and scroll.
    /// Borders, blank space and positions before the first render return `None`.
    pub fn row_at(&self, column: u16, row: u16, lens: &[usize]) -> Option<Row> {
        let rows = self.rows(lens);
        self.picker
            .rendered_index_at((column, row).into(), rows.len())
            .map(|i| rows[i])
    }

    /// Handles mouse input inside the content area remembered by the last render.
    ///
    /// A left click on a header selects and toggles it. Item clicks and wheel
    /// movement match [`ListPicker::handle_mouse`], with item submissions
    /// returned as flat item indices. Before the first render and outside the
    /// remembered area, input is ignored.
    pub fn handle_mouse(&mut self, mouse: MouseEvent, lens: &[usize]) -> Outcome<usize> {
        self.resize(lens);
        let rows = self.rows(lens);
        let outcome = self.picker.handle_mouse(mouse, rows.len());
        // ListPicker has selected the clicked row; a header toggles on the first click.
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && outcome != Outcome::Ignored
            && let Some(index) = self.picker.selected(rows.len())
            && let Row::Header(_) = rows[index]
        {
            return self.activate(rows[index]);
        }
        match outcome {
            Outcome::Submit(index) => self.activate(rows[index]),
            outcome => outcome,
        }
    }

    /// Toggles a header or submits an item.
    fn activate(&mut self, row: Row) -> Outcome<usize> {
        match row {
            Row::Header(group) => {
                self.collapsed[group] = !self.collapsed[group];
                Outcome::Consumed
            }
            Row::Item(item) => Outcome::Submit(item),
        }
    }

    /// Draws grouped title and item lines into `area`.
    ///
    /// Each group is a `(title, items)` pair. Its item count must match the
    /// corresponding `lens` entry passed to input (see [`Outline`]).
    /// Headers start at the content edge. Items use a two-column selection
    /// gutter: `> ` when selected and two spaces otherwise. Caller-provided
    /// line and span styles are preserved.
    /// Collapsed items are omitted, and selection, highlighting, block layout,
    /// and retained scrolling match [`ListPicker::render`].
    pub fn render<'a>(
        &self,
        groups: impl IntoIterator<Item = (Line<'a>, Vec<Line<'a>>)>,
        block: Option<Block<'a>>,
        highlight: Style,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let lines: Vec<_> = groups
            .into_iter()
            .enumerate()
            .flat_map(|(group, (mut title, items))| {
                let collapsed = self.is_collapsed(group);
                title
                    .spans
                    .insert(0, Span::raw(if collapsed { "▸ " } else { "▾ " }));
                let items = if collapsed { Vec::new() } else { items };
                std::iter::once((title, false)).chain(items.into_iter().map(|item| (item, true)))
            })
            .collect();
        let selected = self.picker.selected(lines.len());
        let lines = lines
            .into_iter()
            .enumerate()
            .map(|(index, (mut line, item))| {
                if item {
                    line.spans.insert(
                        0,
                        Span::raw(if selected == Some(index) { "> " } else { "  " }),
                    );
                }
                line
            });
        self.picker
            .render_with_highlight_symbol(lines, block, highlight, area, buf, "");
    }

    fn resize(&mut self, lens: &[usize]) {
        if lens.len() > self.collapsed.len() {
            self.collapsed.resize(lens.len(), false);
        }
    }

    fn rows(&self, lens: &[usize]) -> Vec<Row> {
        let mut rows = Vec::new();
        let mut item = 0usize;
        for (group, &len) in lens.iter().enumerate() {
            rows.push(Row::Header(group));
            if !self.is_collapsed(group) {
                rows.extend((0..len).map(|offset| Row::Item(item.saturating_add(offset))));
            }
            item = item.saturating_add(len);
        }
        rows
    }
}

fn item_group(item: usize, lens: &[usize]) -> Option<usize> {
    let mut first = 0usize;
    for (group, &len) in lens.iter().enumerate() {
        if (first..first + len).contains(&item) {
            return Some(group);
        }
        first += len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{
        crossterm::event::{KeyEventKind, KeyEventState},
        style::Color,
    };

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn hit_rows_follow_rendered_scroll_and_exclude_borders_and_blanks() {
        let lens = [3, 0];
        let mut outline = Outline::new(&lens);
        assert_eq!(outline.row_at(1, 1, &lens), None);
        let area = Rect::new(0, 0, 10, 4);
        outline.select(Row::Header(1), &lens);
        outline.render(
            [
                (Line::raw("A"), vec![Line::raw("a"); 3]),
                (Line::raw("B"), vec![]),
            ],
            Some(Block::bordered()),
            Style::default(),
            area,
            &mut Buffer::empty(area),
        );
        assert_eq!(outline.row_at(1, 0, &lens), None);
        assert_eq!(outline.row_at(0, 1, &lens), None);
        assert_eq!(outline.row_at(1, 1, &lens), Some(Row::Item(2)));
        assert_eq!(outline.row_at(1, 2, &lens), Some(Row::Header(1)));
    }

    #[test]
    fn navigation_moves_through_headers_and_items_without_wrapping() {
        let lens = [2, 1];
        let mut outline = Outline::new(&lens);
        let expected = [Row::Item(0), Row::Item(1), Row::Header(1), Row::Item(2)];
        for row in expected {
            assert_eq!(outline.selected(&lens), Some(row));
            assert_eq!(
                outline.handle_key(key(KeyCode::Down), &lens),
                Outcome::Consumed
            );
        }
        assert_eq!(outline.selected(&lens), Some(Row::Item(2)));
        assert_eq!(
            outline.handle_key(key(KeyCode::Home), &lens),
            Outcome::Ignored
        );
        assert_eq!(
            outline.handle_key(key(KeyCode::End), &lens),
            Outcome::Ignored
        );
        for _ in 0..expected.len() {
            assert_eq!(
                outline.handle_key(key(KeyCode::Char('k')), &lens),
                Outcome::Consumed
            );
        }
        assert_eq!(outline.selected(&lens), Some(Row::Header(0)));
    }

    #[test]
    fn header_keys_toggle_and_follow_modifier_rules() {
        let lens = [1];
        let mut outline = Outline::new(&lens);
        outline.select(Row::Header(0), &lens);
        for code in [KeyCode::Left, KeyCode::Char('h')] {
            assert_eq!(outline.handle_key(key(code), &lens), Outcome::Consumed);
            assert!(outline.is_collapsed(0));
        }
        for code in [KeyCode::Right, KeyCode::Char('l')] {
            assert_eq!(outline.handle_key(key(code), &lens), Outcome::Consumed);
            assert!(!outline.is_collapsed(0));
        }
        assert_eq!(
            outline.handle_key(key(KeyCode::Enter), &lens),
            Outcome::Consumed
        );
        assert!(outline.is_collapsed(0));
        for code in [KeyCode::Char('H'), KeyCode::Char('L')] {
            assert_eq!(outline.handle_key(key(code), &lens), Outcome::Ignored);
        }
        assert_eq!(
            outline.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT), &lens),
            Outcome::Consumed
        );
        assert!(!outline.is_collapsed(0));
        for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            assert_eq!(
                outline.handle_key(KeyEvent::new(KeyCode::Left, modifiers), &lens),
                Outcome::Ignored
            );
            assert!(!outline.is_collapsed(0));
        }
        let release = KeyEvent::new_with_kind_and_state(
            KeyCode::Left,
            KeyModifiers::NONE,
            KeyEventKind::Release,
            KeyEventState::NONE,
        );
        assert_eq!(outline.handle_key(release, &lens), Outcome::Consumed);
        assert!(!outline.is_collapsed(0));
    }

    #[test]
    fn item_keys_submit_cancel_or_fall_through() {
        let lens = [1];
        let mut outline = Outline::new(&lens);
        assert_eq!(
            outline.handle_key(key(KeyCode::Enter), &lens),
            Outcome::Submit(0)
        );
        for code in [
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Char('h'),
            KeyCode::Char('l'),
        ] {
            assert_eq!(outline.handle_key(key(code), &lens), Outcome::Ignored);
        }
        assert_eq!(
            outline.handle_key(key(KeyCode::Esc), &lens),
            Outcome::Cancel
        );
    }

    #[test]
    fn new_starts_on_the_first_item_or_first_header() {
        let outline = Outline::new(&[2, 1]);
        assert_eq!(outline.selected(&[2, 1]), Some(Row::Item(0)));

        let outline = Outline::new(&[0, 1]);
        assert_eq!(outline.selected(&[0, 1]), Some(Row::Header(0)));
    }

    #[test]
    fn hints_are_owned_only_for_the_selected_header_state() {
        let lens = [1];
        let mut outline = Outline::new(&lens);
        assert_eq!(outline.hints(&lens), &[]);
        outline.select(Row::Header(0), &lens);
        assert_eq!(outline.hints(&lens), &[Outline::COLLAPSE, Outline::TOGGLE]);
        outline.handle_key(key(KeyCode::Left), &lens);
        assert_eq!(outline.hints(&lens), &[Outline::EXPAND, Outline::TOGGLE]);
        assert_eq!(Outline::new(&[]).hints(&[]), &[]);
    }

    #[test]
    fn collapsing_and_expanding_keeps_the_cursor_on_its_header() {
        let lens = [2, 1];
        let mut outline = Outline::new(&lens);
        outline.select(Row::Header(1), &lens);
        assert_eq!(
            outline.handle_key(key(KeyCode::Enter), &lens),
            Outcome::Consumed
        );
        assert_eq!(outline.selected(&lens), Some(Row::Header(1)));
        assert!(outline.is_collapsed(1));
        assert_eq!(
            outline.handle_key(key(KeyCode::Right), &lens),
            Outcome::Consumed
        );
        assert_eq!(outline.selected(&lens), Some(Row::Header(1)));
        assert!(!outline.is_collapsed(1));
    }

    #[test]
    fn mouse_toggles_headers_submits_items_and_ignores_borders() {
        let area = Rect::new(2, 3, 12, 5);
        let block = Block::bordered();
        let lens = [1, 1];
        let mut outline = Outline::new(&lens);
        outline.render(
            [
                (Line::raw("A"), vec![Line::raw("a")]),
                (Line::raw("B"), vec![Line::raw("b")]),
            ],
            Some(block.clone()),
            Style::default(),
            area,
            &mut Buffer::empty(area),
        );
        let border = mouse(MouseEventKind::Down(MouseButton::Left), 3, 3);
        assert_eq!(outline.handle_mouse(border, &lens), Outcome::Ignored);
        let selected_header = mouse(MouseEventKind::Down(MouseButton::Left), 3, 4);
        assert_eq!(
            outline.handle_mouse(selected_header, &lens),
            Outcome::Consumed
        );
        assert!(outline.is_collapsed(0));
        assert_eq!(
            outline.handle_mouse(selected_header, &lens),
            Outcome::Consumed
        );
        assert!(!outline.is_collapsed(0));
        let header = mouse(MouseEventKind::Down(MouseButton::Left), 3, 6);
        assert_eq!(outline.handle_mouse(header, &lens), Outcome::Consumed);
        assert_eq!(outline.selected(&lens), Some(Row::Header(1)));
        assert!(outline.is_collapsed(1));
        let item = mouse(MouseEventKind::Down(MouseButton::Left), area.right() - 2, 5);
        assert_eq!(outline.handle_mouse(item, &lens), Outcome::Consumed);
        assert_eq!(outline.handle_mouse(item, &lens), Outcome::Submit(0));
        let wheel = mouse(MouseEventKind::ScrollDown, 3, 4);
        assert_eq!(outline.handle_mouse(wheel, &lens), Outcome::Consumed);
        assert_eq!(outline.selected(&lens), Some(Row::Header(1)));
    }

    #[test]
    fn render_preserves_styles_indents_and_hides_collapsed_items() {
        let area = Rect::new(0, 0, 12, 4);
        let groups = || {
            [
                (
                    Line::from(Span::styled("A", Style::default().fg(Color::Red))),
                    vec![Line::from(Span::styled(
                        "a",
                        Style::default().fg(Color::Blue),
                    ))],
                ),
                (Line::raw("B"), vec![Line::raw("b")]),
            ]
        };
        let mut outline = Outline::new(&[1, 1]);
        outline.select(Row::Header(1), &[1, 1]);
        outline.handle_key(key(KeyCode::Left), &[1, 1]);
        let mut buf = Buffer::empty(area);
        outline.render(
            groups(),
            None,
            Style::default().bg(Color::Green),
            area,
            &mut buf,
        );
        assert_eq!(buf[(0, 0)].symbol(), "▾");
        assert_eq!(buf[(2, 0)].fg, Color::Red);
        assert_eq!(buf[(2, 1)].symbol(), "a");
        assert_eq!(buf[(2, 1)].fg, Color::Blue);
        assert_eq!(buf[(0, 2)].symbol(), "▸");
        assert_eq!(buf[(0, 2)].bg, Color::Green);
        assert_eq!(buf[(2, 3)].symbol(), " ");

        outline.select(Row::Item(0), &[1, 1]);
        outline.render(
            groups(),
            None,
            Style::default().bg(Color::Green),
            area,
            &mut buf,
        );
        assert_eq!(buf[(0, 1)].symbol(), ">");
        assert_eq!(buf[(2, 1)].symbol(), "a");
        assert_eq!(buf[(2, 1)].fg, Color::Blue);
        assert_eq!(buf[(2, 1)].bg, Color::Green);
    }

    #[test]
    fn empty_and_changing_groups_are_safe_and_selection_expands_items() {
        let mut outline = Outline::new(&[]);
        assert_eq!(outline.selected(&[]), None);
        assert_eq!(
            outline.handle_key(key(KeyCode::Enter), &[]),
            Outcome::Consumed
        );
        assert_eq!(outline.handle_key(key(KeyCode::Esc), &[]), Outcome::Cancel);
        assert_eq!(
            outline.handle_key(key(KeyCode::Enter), &[0]),
            Outcome::Consumed
        );
        assert!(outline.is_collapsed(0));
        outline.select(Row::Header(1), &[0, 2]);
        assert_eq!(
            outline.handle_key(key(KeyCode::Left), &[0, 2]),
            Outcome::Consumed
        );
        assert!(outline.is_collapsed(1));
        outline.select(Row::Header(0), &[0]);
        assert!(outline.is_collapsed(1));
        outline.select(Row::Item(1), &[0, 2]);
        assert_eq!(outline.selected(&[0, 2]), Some(Row::Item(1)));
        assert!(!outline.is_collapsed(1));
        outline.select(Row::Item(99), &[0, 2]);
        assert_eq!(outline.selected(&[0, 2]), Some(Row::Item(1)));
        outline.select(Row::Header(99), &[1]);
        assert_eq!(outline.selected(&[1]), Some(Row::Header(0)));
        outline.render(
            [(Line::raw("A"), vec![Line::raw("a")])],
            None,
            Style::default(),
            Rect::ZERO,
            &mut Buffer::empty(Rect::ZERO),
        );
    }

    #[test]
    fn retains_scroll_offset_across_frames() {
        let area = Rect::new(0, 0, 8, 2);
        let groups = || {
            [
                (
                    Line::raw("A"),
                    vec![Line::raw("a"), Line::raw("b"), Line::raw("c")],
                ),
                (Line::raw("B"), vec![]),
            ]
        };
        let lens = [3, 0];
        let mut outline = Outline::new(&lens);
        outline.select(Row::Header(1), &lens);
        let mut buf = Buffer::empty(area);
        outline.render(groups(), None, Style::default(), area, &mut buf);
        assert_eq!(buf[(2, 0)].symbol(), "c");
        assert_eq!(buf[(0, 1)].symbol(), "▾");

        let item = mouse(MouseEventKind::Down(MouseButton::Left), 0, 0);
        assert_eq!(outline.handle_mouse(item, &lens), Outcome::Consumed);
        assert_eq!(outline.selected(&lens), Some(Row::Item(2)));
        outline.render(groups(), None, Style::default(), area, &mut buf);
        assert_eq!(buf[(2, 0)].symbol(), "c");

        let header = mouse(MouseEventKind::Down(MouseButton::Left), 0, 1);
        assert_eq!(outline.handle_mouse(header, &lens), Outcome::Consumed);
        assert_eq!(outline.selected(&lens), Some(Row::Header(1)));
        assert!(outline.is_collapsed(1));
    }
}
