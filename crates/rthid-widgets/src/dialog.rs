//! Dialog placement and a cleared border for caller supplied content.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Line,
    widgets::{Block, Clear, Widget},
};

/// Bordered popup frame that the caller fills with any content.
///
/// [`Dialog::area`] sizes and places the popup and [`Dialog::render`] draws
/// the frame and hands back the space inside it. The dialog holds no state
/// and handles no keys.
///
/// # Examples
///
/// ```
/// use ratatui::{buffer::Buffer, layout::Rect};
/// use rthid_widgets::Dialog;
///
/// let screen = Rect::new(0, 0, 30, 9);
/// let dialog = Dialog { title: "Save", min_width: 10 };
/// let area = dialog.area(screen, screen, (6, 1));
/// let mut buf = Buffer::empty(screen);
/// let inner = dialog.render(area, &mut buf);
/// assert_eq!(area, Rect::new(8, 3, 14, 3));
/// assert_eq!(inner, Rect::new(9, 4, 12, 1));
/// ```
pub struct Dialog<'a> {
    /// Text drawn at the left of the top border. Its display width also
    /// sets a lower bound on the dialog width.
    pub title: &'a str,
    /// Smallest content width in columns, before the border and padding are
    /// added. Keeps short prompts from producing a cramped dialog.
    pub min_width: u16,
}

impl Dialog<'_> {
    /// Returns the outer rectangle for a dialog whose content needs
    /// `content` columns and rows.
    ///
    /// The width is the largest of the content width, the title display width
    /// and `min_width`, plus four columns: a border and one padding column on
    /// each side. The height is the content height plus two rows of
    /// border. Both are capped at the screen size. The dialog is centered on
    /// `anchor`, then shifted so it lies fully inside `screen`.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::layout::Rect;
    /// use rthid_widgets::Dialog;
    ///
    /// let screen = Rect::new(0, 0, 40, 10);
    /// let dialog = Dialog { title: "Rename", min_width: 10 };
    /// assert_eq!(dialog.area(screen, screen, (5, 1)), Rect::new(13, 3, 14, 3));
    /// // An anchor at the right edge is pulled back inside the screen.
    /// let anchor = Rect::new(38, 0, 2, 2);
    /// assert_eq!(dialog.area(anchor, screen, (5, 1)), Rect::new(26, 0, 14, 3));
    /// ```
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

    /// Places a popup below the anchor, or above when it will not fit, within the screen.
    pub fn anchored_area(&self, anchor: Rect, screen: Rect, content: (u16, u16)) -> Rect {
        let mut area = self.area(anchor, screen, content);
        area.x = anchor
            .x
            .clamp(screen.x, screen.right().saturating_sub(area.width));
        let y = if screen.bottom().saturating_sub(anchor.bottom()) >= area.height {
            anchor.bottom()
        } else {
            anchor.y.saturating_sub(area.height)
        };
        area.y = y.clamp(screen.y, screen.bottom().saturating_sub(area.height));
        area
    }

    /// Clears every cell in `area`, draws the border with the title and
    /// returns the rectangle inside the border.
    ///
    /// The returned rectangle includes the padding columns reserved by
    /// [`Dialog::area`], so callers inset it themselves if they want the gap.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::{buffer::Buffer, layout::Rect};
    /// use rthid_widgets::Dialog;
    ///
    /// let mut buf = Buffer::with_lines(["xxxxxxxx", "xxxxxxxx", "xxxxxxxx"]);
    /// let dialog = Dialog { title: "Hi", min_width: 0 };
    /// let inner = dialog.render(Rect::new(0, 0, 8, 3), &mut buf);
    /// assert_eq!(inner, Rect::new(1, 1, 6, 1));
    /// assert_eq!(buf, Buffer::with_lines(["┌Hi────┐", "│      │", "└──────┘"]));
    /// ```
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
    fn anchored_placement_flips_and_clamps() {
        let dialog = Dialog {
            title: "Choices",
            min_width: 0,
        };
        let screen = Rect::new(5, 3, 30, 20);
        assert_eq!(
            dialog.anchored_area(Rect::new(7, 5, 10, 1), screen, (8, 4)),
            Rect::new(7, 6, 12, 6)
        );
        assert_eq!(
            dialog.anchored_area(Rect::new(33, 21, 2, 1), screen, (8, 4)),
            Rect::new(23, 15, 12, 6)
        );
        assert_eq!(
            dialog.anchored_area(Rect::new(0, 0, 1, 1), screen, (u16::MAX, u16::MAX)),
            screen
        );
        assert_eq!(
            dialog.anchored_area(Rect::ZERO, Rect::ZERO, (8, 4)),
            Rect::ZERO
        );
    }

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
