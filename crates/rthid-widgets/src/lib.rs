//! Reusable input components for ratatui applications.
//!
//! Each component owns its state, turns key events into an [`Outcome`] and
//! draws itself into a ratatui [`Buffer`](ratatui::buffer::Buffer). None of
//! them know anything about the application that hosts them.
//!
//! Components compose through a small contract:
//!
//! - A key handler returns an [`Outcome`]. `Ignored` means the component did
//!   not use the key, so the parent should try its own bindings.
//! - The application decides what `Submit` and `Cancel` mean, such as
//!   validating a value, closing a dialog or asking a follow up question.
//! - Drawing takes `&self`, so it works from an immutable borrow of the app
//!   inside a draw closure.
//! - [`TextInput::render`] returns the cursor [`Position`](ratatui::layout::Position)
//!   instead of moving the cursor. Callers pass it to
//!   `Frame::set_cursor_position`.
//!
//! [`Dialog`] places and frames a popup and [`key_hints`] formats help text.
//!
//! # Examples
//!
//! A modal list where d asks before deleting the selected item, and keys the
//! modal does not use fall back to the caller.
//!
//! ```
//! use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
//! use rthid_widgets::{Confirm, ListPicker, Outcome};
//!
//! struct Modal {
//!     picker: ListPicker,
//!     confirm: Option<Confirm>,
//! }
//!
//! /// Returns false when the caller should handle the key.
//! fn on_key(modal: &mut Modal, items: &mut Vec<&str>, key: KeyEvent) -> bool {
//!     if let Some(confirm) = modal.confirm.take() {
//!         if confirm.handle_key(key) == Outcome::Submit(()) {
//!             items.remove(modal.picker.selected());
//!         }
//!         return true;
//!     }
//!     match modal.picker.handle_key(key, items.len()) {
//!         Outcome::Ignored if key.code == KeyCode::Char('d') && !items.is_empty() => {
//!             let name = items[modal.picker.selected()];
//!             modal.confirm = Some(Confirm::new(format!("Delete {name}?")));
//!             true
//!         }
//!         Outcome::Ignored => false,
//!         Outcome::Consumed | Outcome::Submit(_) | Outcome::Cancel => true,
//!     }
//! }
//!
//! let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
//! let mut items = vec!["warm", "cool"];
//! let mut modal = Modal { picker: ListPicker::new(0), confirm: None };
//! assert!(on_key(&mut modal, &mut items, key('j')));
//! assert!(on_key(&mut modal, &mut items, key('d')));
//! assert_eq!(modal.confirm.as_ref().map(Confirm::message), Some("Delete cool?"));
//! assert!(on_key(&mut modal, &mut items, key('y')));
//! assert_eq!(items, ["warm"]);
//! assert!(!on_key(&mut modal, &mut items, key('q')));
//! ```

mod confirm;
mod dialog;
mod key_hints;
mod list_picker;
mod outcome;
mod text_input;

pub use confirm::Confirm;
pub use dialog::Dialog;
pub use key_hints::key_hints;
pub use list_picker::ListPicker;
pub use outcome::Outcome;
pub use text_input::TextInput;
