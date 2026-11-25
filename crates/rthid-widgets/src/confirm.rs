//! Confirmation input with a message for the caller to display.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::{Outcome, outcome::is_release};

/// Yes or no question that treats every key except a lowercase y as a no.
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

    /// Returns `Submit(())` for a lowercase y and `Cancel` for any other key,
    /// including an uppercase Y, Enter and Esc.
    ///
    /// Modifiers are not checked. A key release returns `Ignored` so the
    /// question survives the release of the key that opened it. Otherwise the
    /// result is never `Ignored` or `Consumed`, so the caller should close the
    /// question after any key.
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
            Outcome::Ignored
        } else if key.code == KeyCode::Char('y') {
            Outcome::Submit(())
        } else {
            Outcome::Cancel
        }
    }

    /// Key and action pairs that describe [`Confirm::handle_key`], ready to
    /// pass to [`key_hints`](crate::key_hints).
    ///
    /// # Examples
    ///
    /// ```
    /// use rthid_widgets::{Confirm, key_hints};
    ///
    /// assert_eq!(key_hints(&[Confirm::HINTS]), "y: yes | any other key: cancel");
    /// ```
    pub const HINTS: &'static [(&'static str, &'static str)] =
        &[("y", "yes"), ("any other key", "cancel")];
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
    }
}
