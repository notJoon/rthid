# rthid-widgets

Reusable UI components for the rthid terminal app, built on ratatui.

Each component holds its own state, turns key events into an `Outcome` and draws
itself. Components know nothing about the app, so the app decides what a submit or
cancel means and composes them into dialogs as needed.

| Component | Purpose |
| --- | --- |
| `TextInput` | Single line text editing with a character limit and a cursor position for the caller |
| `ListPicker` | Cursor over a list the caller owns, with scroll kept across frames |
| `Confirm` | A yes or no question answered by y or any other key |
| `Dialog` | Centered popup area with a cleared background and a border |
| `key_hints` | Formats `key: action` help text |

See the crate docs (`cargo doc -p rthid-widgets --open`) for the API and examples.
