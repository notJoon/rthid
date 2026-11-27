//! Confirmation input with a message for the caller to display.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{Hint, Outcome, outcome::is_release};

/// Yes or no question that treats every key except an unmodified lowercase y as a no.
///
/// Defaulting to cancel keeps destructive actions safe from stray keys. The
/// component holds the message so the caller can show it, for example as a
/// [`Dialog`](crate::Dialog) title, but draws nothing itself.
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
        }
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

    /// Key and action pairs that describe [`Confirm::handle_key`], ready to
    /// pass to [`key_hints`](crate::key_hints).
    /// [`Self::YES`] and [`Self::CANCEL`] provide the same hints for
    /// [`hint_line`](crate::hint_line).
    ///
    /// # Examples
    ///
    /// ```
    /// use rthid_widgets::{Confirm, key_hints};
    ///
    /// assert_eq!(key_hints(&[Confirm::HINTS]), "y: yes | any other key: cancel");
    /// ```
    pub const HINTS: &'static [(&'static str, &'static str)] = &[
        (Self::YES.key, Self::YES.label),
        (Self::CANCEL.key, Self::CANCEL.label),
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

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
