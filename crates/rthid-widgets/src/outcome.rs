//! Shared key handling results for parent components.

/// Result of handing a key to a component.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome<T> {
    /// The parent may handle this key.
    Ignored,
    /// The component handled the key without finishing.
    Consumed,
    /// The caller may accept or reject this value.
    Submit(T),
    /// The user dismissed the component.
    Cancel,
}
