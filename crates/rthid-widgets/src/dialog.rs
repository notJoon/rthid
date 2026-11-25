//! Dialog placement and a cleared border for caller supplied content.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Line,
    widgets::{Block, Clear, Widget},
};

/// Places a bordered dialog within the screen.
pub struct Dialog<'a> {
    /// Text displayed in the top border.
    pub title: &'a str,
    /// Minimum content width before horizontal padding.
    pub min_width: u16,
}

impl Dialog<'_> {
    /// Centers on the anchor and moves the result inside the screen.
    pub fn area(&self, anchor: Rect, screen: Rect, content: (u16, u16)) -> Rect {
        let width = (usize::from(content.0)
            .max(Line::raw(self.title).width())
            .max(usize::from(self.min_width))
            .saturating_add(4))
        .min(usize::from(screen.width)) as u16;
        let height = content.1.saturating_add(2).min(screen.height);
        let x = (i32::from(anchor.x) + (i32::from(anchor.width) - i32::from(width)) / 2).clamp(
            i32::from(screen.x),
            i32::from(screen.right().saturating_sub(width)),
        );
        let y = (i32::from(anchor.y) + (i32::from(anchor.height) - i32::from(height)) / 2).clamp(
            i32::from(screen.y),
            i32::from(screen.bottom().saturating_sub(height)),
        );
        Rect::new(x as u16, y as u16, width, height)
    }

    /// Clears underlying cells and returns the area inside the border.
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> Rect {
        Clear.render(area, buf);
        let block = Block::bordered().title(self.title);
        let inner = block.inner(area);
        block.render(area, buf);
        inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centers_clamps_and_clears() {
        let dialog = Dialog {
            title: "界界",
            min_width: 0,
        };
        let screen = Rect::new(5, 3, 30, 10);
        assert_eq!(dialog.area(screen, screen, (1, 2)), Rect::new(16, 6, 8, 4));
        assert_eq!(
            dialog.area(Rect::new(0, 0, 2, 2), screen, (1, 2)),
            Rect::new(5, 3, 8, 4)
        );
        assert_eq!(dialog.area(screen, screen, (u16::MAX, u16::MAX)), screen);
        let area = Rect::new(0, 0, 8, 3);
        let mut buf = Buffer::with_lines(["xxxxxxxx", "xxxxxxxx", "xxxxxxxx"]);
        assert_eq!(dialog.render(area, &mut buf), Rect::new(1, 1, 6, 1));
        assert_eq!(
            buf,
            Buffer::with_lines(["┌界界──┐", "│      │", "└──────┘"])
        );
    }
}
