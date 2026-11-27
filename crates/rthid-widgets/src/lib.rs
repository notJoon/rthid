#![doc = include_str!("../README.md")]

//! # Composing a list with confirmation
//!
//! A modal list where d asks before deleting the selected item, and keys the
//! modal does not use fall back to the caller.
//!
//! ```
//! use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
//! use rthid_widgets::{Confirm, ListPicker, Outcome};
//!
//! struct Modal {
//!     picker: ListPicker,
//!     confirm: Option<Confirm>,
//! }
//!
//! /// Returns false when the caller should handle the key.
//! fn on_key(modal: &mut Modal, items: &mut Vec<&str>, key: KeyEvent) -> bool {
//!     if let Some(confirm) = &modal.confirm {
//!         match confirm.handle_key(key) {
//!             Outcome::Submit(()) => {
//!                 if let Some(index) = modal.picker.selected(items.len()) {
//!                     items.remove(index);
//!                 }
//!                 modal.confirm = None;
//!             }
//!             Outcome::Cancel => modal.confirm = None,
//!             Outcome::Ignored | Outcome::Consumed => {}
//!         }
//!         return true;
//!     }
//!     match modal.picker.handle_key(key, items.len()) {
//!         Outcome::Ignored
//!             if key.code == KeyCode::Char('d')
//!                 && key.modifiers.is_empty() => {
//!             let Some(index) = modal.picker.selected(items.len()) else { return false };
//!             let name = items[index];
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
//! let mut release = key('d');
//! release.kind = KeyEventKind::Release;
//! assert!(on_key(&mut modal, &mut items, release));
//! assert!(modal.confirm.is_none());
//! assert!(on_key(&mut modal, &mut items, key('j')));
//! assert!(on_key(&mut modal, &mut items, key('d')));
//! assert_eq!(modal.confirm.as_ref().map(Confirm::message), Some("Delete cool?"));
//! assert!(on_key(&mut modal, &mut items, release));
//! assert!(modal.confirm.is_some());
//! assert!(on_key(&mut modal, &mut items, key('y')));
//! assert_eq!(items, ["warm"]);
//! assert!(!on_key(&mut modal, &mut items, key('q')));
//! ```

mod confirm;
mod dialog;
mod hints;
mod list_picker;
mod outcome;
mod text_input;

pub use confirm::Confirm;
pub use dialog::Dialog;
pub use hints::{Hint, KeyHelp, hint_line};
pub use list_picker::ListPicker;
pub use outcome::Outcome;
pub use text_input::TextInput;
