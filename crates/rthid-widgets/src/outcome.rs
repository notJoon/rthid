//! Shared key handling results for parent components.

/// Result of handing a key to a component.
///
/// Components never act on `Submit` or `Cancel` themselves. The caller
/// decides what they mean, for example whether a submitted value is valid,
/// and falls back to its own key bindings on `Ignored`.
///
/// # Examples
///
/// ```
/// use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
/// use rthid_widgets::{Outcome, TextInput};
///
/// let mut input = TextInput::new("", 8);
/// let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);
/// let handled_by_app = match input.handle_key(tab) {
///     Outcome::Ignored => true,
///     Outcome::Consumed | Outcome::Submit(()) | Outcome::Cancel => false,
/// };
/// assert!(handled_by_app);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome<T> {
    /// The component does not use this key, so the parent may handle it.
    Ignored,
    /// The component used the key and stays open.
    Consumed,
    /// The user asked to finish with this value. The caller may accept it
    /// and close the component, or reject it and keep the component open.
    Submit(T),
    /// The user asked to dismiss the component without a value.
    Cancel,
}
