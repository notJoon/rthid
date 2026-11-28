//! Confirmation input and visible choices for a caller supplied dialog.

use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{Hint, Outcome, outcome::is_release};

/// Yes or no question that treats every key except an unmodified lowercase y as a no.
///
/// Defaulting to cancel keeps destructive actions safe from stray keys. The
/// component holds the message so the caller can show it, for example as a
/// [`Dialog`](crate::Dialog) title, with [`Self::lines`] as its body.
///
/// # Examples
///
/// ```
/// use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
/// use rthid_widgets::{Confirm, Outcome};
///
/// let confirm = Confirm::new("Delete preset?");
/// let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
/// assert_eq!(confirm.handle_key(key('y')), Outcome::Submit(()));
/// assert_eq!(confirm.handle_key(key('n')), Outcome::Cancel);
/// ```
pub struct Confirm {
    message: String,
    yes_label: &'static str,
    detail: Option<String>,
}

impl Confirm {
    /// An unmodified lowercase y confirms the action.
    ///
    /// ```
    /// use rthid_widgets::{Confirm, hint_line};
    ///
    /// let line = hint_line(&[Confirm::YES, Confirm::CANCEL], None, 80);
    /// assert_eq!(line.to_string(), "y: yes | any other key: cancel");
    /// ```
    pub const YES: Hint = Hint::new("y", "yes");

    /// Any key other than an unmodified lowercase y cancels the action.
    pub const CANCEL: Hint = Hint::new("any other key", "cancel");

    /// Creates a question with the message the caller will display.
    ///
    /// # Examples
    ///
    /// ```
    /// use rthid_widgets::Confirm;
    ///
    /// let confirm = Confirm::new(format!("Delete {}?", "warm"));
    /// assert_eq!(confirm.message(), "Delete warm?");
    /// ```
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            yes_label: "yes",
            detail: None,
        }
    }

    /// Names the action in the choices and hints.
    ///
    /// ```
    /// use rthid_widgets::Confirm;
    /// let confirm = Confirm::new("Delete?").yes_label("delete");
    /// assert_eq!(confirm.hints()[0].label, "delete");
    /// ```
    pub fn yes_label(mut self, label: &'static str) -> Self {
        self.yes_label = label;
        self
    }

    /// Adds context above the choices.
    ///
    /// ```
    /// use rthid_widgets::Confirm;
    /// let confirm = Confirm::new("Delete?").detail("This cannot be undone.");
    /// assert_eq!(confirm.lines()[0].to_string(), "This cannot be undone.");
    /// ```
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Describes the same actions shown inside the dialog.
    pub fn hints(&self) -> [Hint; 2] {
        [Hint::new("y", self.yes_label), Self::CANCEL]
    }

    /// Body rows with bold keys, separated from optional context by a blank row.
    ///
    /// ```
    /// use rthid_widgets::Confirm;
    /// let confirm = Confirm::new("Continue?");
    /// assert_eq!(confirm.lines()[1].to_string(), "y  yes     Esc / other key  cancel");
    /// ```
    pub fn lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        if let Some(detail) = &self.detail {
            lines.push(Line::raw(detail.clone()));
        }
        lines.push(Line::default());
        let key = Style::default().add_modifier(Modifier::BOLD);
        lines.push(Line::from(vec![
            Span::styled("y", key),
            Span::raw(format!("  {}     ", self.yes_label)),
            Span::styled("Esc / other key", key),
            Span::raw("  cancel"),
        ]));
        lines
    }

    /// Returns the message given at creation.
    ///
    /// # Examples
    ///
    /// ```
    /// use rthid_widgets::Confirm;
    ///
    /// assert_eq!(Confirm::new("Overwrite?").message(), "Overwrite?");
    /// ```
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns `Submit(())` for an unmodified lowercase y and `Cancel` for any other key,
    /// including an uppercase Y, Enter and Esc.
    ///
    /// Modified keys, including Ctrl+y and Alt+y, cancel. A key release returns
    /// `Consumed` so the question survives the release of the key that opened it.
    /// Other events return `Submit` or `Cancel`, so the caller can close the question.
    ///
    /// # Examples
    ///
    /// ```
    /// use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    /// use rthid_widgets::{Confirm, Outcome};
    ///
    /// let confirm = Confirm::new("Quit?");
    /// let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    /// assert_eq!(confirm.handle_key(key(KeyCode::Char('y'))), Outcome::Submit(()));
    /// assert_eq!(confirm.handle_key(key(KeyCode::Char('Y'))), Outcome::Cancel);
    /// assert_eq!(confirm.handle_key(key(KeyCode::Enter)), Outcome::Cancel);
    /// ```
    pub fn handle_key(&self, key: KeyEvent) -> Outcome<()> {
        if is_release(&key) {
            Outcome::Consumed
        } else if key.code == KeyCode::Char('y') && key.modifiers == KeyModifiers::NONE {
            Outcome::Submit(())
        } else {
            Outcome::Cancel
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    #[test]
    fn body_has_detail_action_and_bold_keys() {
        let confirm = Confirm::new("Discard?")
            .detail("Edits will be lost.")
            .yes_label("discard changes");
        let lines = confirm.lines();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].to_string(), "Edits will be lost.");
        assert_eq!(lines[1].to_string(), "");
        assert_eq!(
            lines[2].to_string(),
            "y  discard changes     Esc / other key  cancel"
        );
        for index in [0, 2] {
            assert!(
                lines[2].spans[index]
                    .style
                    .add_modifier
                    .contains(Modifier::BOLD)
            );
        }
        assert_eq!(
            confirm.hints(),
            [Hint::new("y", "discard changes"), Confirm::CANCEL]
        );
        assert_eq!(Confirm::new("Continue?").lines().len(), 2);
    }

    #[test]
    fn confirms_only_y() {
        let confirm = Confirm::new("Continue?");
        assert_eq!(confirm.message(), "Continue?");
        for code in [
            KeyCode::Char('y'),
            KeyCode::Char('Y'),
            KeyCode::Enter,
            KeyCode::Esc,
        ] {
            let expected = if code == KeyCode::Char('y') {
                Outcome::Submit(())
            } else {
                Outcome::Cancel
            };
            assert_eq!(
                confirm.handle_key(KeyEvent::new(code, KeyModifiers::NONE)),
                expected
            );
        }
        for modifier in [
            KeyModifiers::CONTROL,
            KeyModifiers::ALT,
            KeyModifiers::SHIFT,
            KeyModifiers::SUPER,
        ] {
            assert_eq!(
                confirm.handle_key(KeyEvent::new(KeyCode::Char('y'), modifier)),
                Outcome::Cancel
            );
        }
    }
}
