//! Confirmation input with a message for the caller to display.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use crate::Outcome;

/// Accepts only a lowercase y as confirmation.
pub struct Confirm {
    message: String,
}

impl Confirm {
    /// Stores the message for the dialog title.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// Borrows the message for display.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Confirms on y and cancels on every other key.
    pub fn handle_key(&self, key: KeyEvent) -> Outcome<()> {
        if key.code == KeyCode::Char('y') {
            Outcome::Submit(())
        } else {
            Outcome::Cancel
        }
    }

    /// Key descriptions suitable for a dialog footer.
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
