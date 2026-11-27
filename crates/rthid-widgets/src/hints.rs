//! Width-aware key hints and references.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

/// One key and what it does now.
///
/// `key` is free-form display text shown before the colon. It is usually key
/// names with alternatives joined by `/` and groups separated by a space, such
/// as `"Up/Down k/j"`, or a short phrase such as `"any other key"`. It is not
/// parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hint {
    /// Free-form display text shown before the colon.
    pub key: &'static str,
    /// What the key does now.
    pub label: &'static str,
}

/// Titled groups of hints shown as an aligned key reference.
///
/// Empty sections are omitted. Non-empty sections are separated by one blank
/// line, and every key is aligned to the widest key in the whole reference.
///
/// # Examples
///
/// ```
/// use ratatui::{buffer::Buffer, layout::Rect};
/// use rthid_widgets::{Hint, KeyHelp};
///
/// let navigation = [Hint::new("j/k", "move"), Hint::new("Enter", "choose")];
/// let sections = [("List", navigation.as_slice())];
/// let help = KeyHelp { sections: &sections };
/// assert_eq!(help.size(), (13, 3));
/// let mut buffer = Buffer::empty(Rect::new(0, 0, 13, 3));
/// help.render(buffer.area, &mut buffer);
/// assert_eq!(buffer[(0, 1)].symbol(), "j");
/// ```
pub struct KeyHelp<'a> {
    /// Section titles and their key hints, in display order.
    pub sections: &'a [(&'a str, &'a [Hint])],
}

impl KeyHelp<'_> {
    /// Content size in cells (width, height) to pass to [`crate::Dialog::area`].
    pub fn size(&self) -> (u16, u16) {
        let lines = self.lines();
        let width = lines.iter().map(Line::width).max().unwrap_or(0);
        (
            width.min(u16::MAX as usize) as u16,
            lines.len().min(u16::MAX as usize) as u16,
        )
    }

    /// Draws titles and `key  label` rows, clipped to `area`.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(self.lines()).render(area, buf);
    }

    fn lines(&self) -> Vec<Line<'_>> {
        let key_width = self.key_width();
        let mut lines = Vec::new();
        for &(title, hints) in self.sections.iter().filter(|(_, hints)| !hints.is_empty()) {
            if !lines.is_empty() {
                lines.push(Line::default());
            }
            lines.push(Line::styled(
                title,
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.extend(hints.iter().map(|hint| {
                let width = Line::raw(hint.key).width();
                Line::raw(format!(
                    "{}{:<padding$}{}",
                    hint.key,
                    "",
                    hint.label,
                    padding = key_width - width + 2
                ))
            }));
        }
        lines
    }

    fn key_width(&self) -> usize {
        self.sections
            .iter()
            .flat_map(|(_, hints)| hints.iter())
            .map(|hint| Line::raw(hint.key).width())
            .max()
            .unwrap_or(0)
    }
}

impl Hint {
    /// Creates a key hint.
    pub const fn new(key: &'static str, label: &'static str) -> Self {
        Self { key, label }
    }
}

/// Returns a help line that fits in `width` display columns.
///
/// Hints render as `key: label`, joined by ` | `. The longest prefix that fits
/// is shown in slice order. A hint is never cut. Filling stops at the first hint
/// that does not fit, so later shorter hints are not squeezed in.
///
/// When `more` is present, its width is reserved first and it is always shown
/// last whenever it fits, even if no hint was dropped. A hint that would fit
/// alone can therefore give way to `more`. If `more` alone does not fit, or no
/// hint fits and `more` is `None`, the line is empty. Spans are unstyled; style
/// the returned line as needed.
///
/// # Examples
///
/// ```
/// use rthid_widgets::{Hint, hint_line};
///
/// let hints = [Hint::new("Enter", "submit"), Hint::new("Esc", "cancel")];
/// let line = hint_line(&hints, Some(Hint::new("?", "keys")), 23);
/// assert_eq!(line.to_string(), "Enter: submit | ?: keys");
/// ```
pub fn hint_line(hints: &[Hint], more: Option<Hint>, width: u16) -> Line<'static> {
    let width = usize::from(width);
    let more_width = more.map_or(0, hint_width);
    if more_width > width {
        return Line::default();
    }

    let separator = Span::raw(SEPARATOR).width();
    let budget = (width - more_width).saturating_sub(usize::from(more.is_some()) * separator);
    let mut line = Line::default();
    let mut used = 0;
    for &hint in hints {
        let next = used + usize::from(used > 0) * separator + hint_width(hint);
        if next > budget {
            break;
        }
        push(&mut line, hint);
        used = next;
    }
    if let Some(more) = more {
        push(&mut line, more);
    }
    line
}

const SEPARATOR: &str = " | ";

fn spans(hint: Hint) -> [Span<'static>; 3] {
    [Span::raw(hint.key), Span::raw(": "), Span::raw(hint.label)]
}

fn hint_width(hint: Hint) -> usize {
    spans(hint).iter().map(Span::width).sum()
}

fn push(line: &mut Line<'static>, hint: Hint) {
    if !line.spans.is_empty() {
        line.spans.push(Span::raw(SEPARATOR));
    }
    line.spans.extend(spans(hint));
}

#[cfg(test)]
mod tests {
    use super::*;

    const HINTS: &[Hint] = &[
        Hint::new("j/k", "select"),
        Hint::new("Enter", "열기"),
        Hint::new("q", "quit"),
    ];

    #[test]
    fn never_exceeds_the_available_display_width() {
        for width in 0..=80 {
            assert!(hint_line(HINTS, Some(Hint::new("?", "more")), width).width() <= width.into());
            assert!(hint_line(HINTS, None, width).width() <= width.into());
        }
    }

    #[test]
    fn wide_char_hint_fits_at_its_exact_boundary() {
        assert_eq!(
            hint_line(&HINTS[..2], None, 25).to_string(),
            "j/k: select | Enter: 열기"
        );
        assert_eq!(hint_line(&HINTS[..2], None, 24).to_string(), "j/k: select");
    }

    #[test]
    fn stops_at_the_first_hint_that_does_not_fit() {
        let hints = [
            Hint::new("a", "first"),
            Hint::new("very-long-key", "second"),
            Hint::new("c", "third"),
        ];
        assert_eq!(hint_line(&hints, None, 20).to_string(), "a: first");
    }

    #[test]
    fn reserves_more_and_always_renders_it_last() {
        let more = Hint::new("?", "more");
        assert_eq!(
            hint_line(HINTS, Some(more), 80).to_string(),
            "j/k: select | Enter: 열기 | q: quit | ?: more"
        );
        assert_eq!(hint_line(HINTS, Some(more), 7).to_string(), "?: more");
        assert_eq!(hint_line(HINTS, Some(more), 6).to_string(), "");
    }

    #[test]
    fn wider_lines_never_show_fewer_hints() {
        for more in [None, Some(Hint::new("?", "more"))] {
            let mut previous = 0;
            for width in 0..=80 {
                let shown = hint_line(HINTS, more, width)
                    .spans
                    .iter()
                    .filter(|span| span.content.as_ref() == ": ")
                    .count();
                assert!(shown >= previous);
                previous = shown;
            }
        }
    }

    #[test]
    fn empty_or_too_narrow_lines_are_empty() {
        assert_eq!(hint_line(&[], None, 80).to_string(), "");
        assert_eq!(
            hint_line(&[Hint::new("Enter", "open")], None, 10).to_string(),
            ""
        );
    }

    #[test]
    fn output_depends_only_on_inputs() {
        let first = hint_line(HINTS, Some(Hint::new("?", "more")), 30);
        let second = hint_line(HINTS, Some(Hint::new("?", "more")), 30);
        assert_eq!(first, second);
    }

    #[test]
    fn key_help_size_matches_aligned_wide_rendering() {
        let first = [Hint::new("한", "열기"), Hint::new("x", "close")];
        let second = [Hint::new("Enter", "choose")];
        let sections = [("한국어", first.as_slice()), ("Other", second.as_slice())];
        let help = KeyHelp {
            sections: &sections,
        };
        assert_eq!(help.size(), (13, 6));
        let mut buf = Buffer::empty(Rect::new(0, 0, 13, 6));
        help.render(buf.area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "한");
        assert_eq!(buf[(0, 1)].symbol(), "한");
        assert_eq!(buf[(7, 1)].symbol(), "열");
        assert_eq!(buf[(7, 2)].symbol(), "c");
        assert_eq!(buf[(12, 5)].symbol(), "e");
    }

    #[test]
    fn key_help_omits_empty_sections_and_clips_tiny_areas() {
        let hints = [Hint::new("Enter", "choose")];
        let sections = [("Empty", &[][..]), ("List", hints.as_slice())];
        let help = KeyHelp {
            sections: &sections,
        };
        assert_eq!(help.size(), (13, 2));
        let mut empty = Buffer::empty(Rect::default());
        help.render(Rect::default(), &mut empty);
        let mut tiny = Buffer::empty(Rect::new(0, 0, 2, 1));
        help.render(tiny.area, &mut tiny);
        assert_eq!(
            tiny.content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>(),
            "Li"
        );
    }
}
