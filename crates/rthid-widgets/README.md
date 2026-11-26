# rthid-widgets

Reusable UI components for the rthid terminal app, built on ratatui.

## Quick start

This crate is not published to crates.io. Add these dependencies to your app's
`Cargo.toml`, adjusting the path to your checkout:

```toml
[dependencies]
ratatui = "0.30.2"
rthid-widgets = { path = "../rthid/crates/rthid-widgets" }
```

Put this in `src/main.rs` and run `cargo run` in an interactive terminal. Type a
value, press Enter to submit it, or Esc to cancel:

```rust,no_run
use std::io;
use ratatui::{crossterm::event::{self, Event}, style::Style};
use rthid_widgets::{Outcome, TextInput};

fn main() -> io::Result<()> {
    let mut input = TextInput::new("", 80);
    let submitted = ratatui::run(|terminal| -> io::Result<Option<String>> {
        loop {
            terminal.draw(|frame| {
                let area = frame.area();
                if let Some(cursor) = input.render("> ", Style::default(), area, frame.buffer_mut()) {
                    frame.set_cursor_position(cursor);
                }
            })?;
            if let Event::Key(key) = event::read()? {
                match input.handle_key(key) {
                    Outcome::Submit(()) => return Ok(Some(input.text().to_owned())),
                    Outcome::Cancel => return Ok(None),
                    Outcome::Consumed | Outcome::Ignored => {}
                }
            }
        }
    })?;
    if let Some(text) = submitted {
        println!("Submitted: {text}");
    }
    Ok(())
}
```

The terminal session belongs to the app. Widgets render into a ratatui buffer;
`TextInput` returns the cursor position for the app to apply. Text limits count
Unicode scalar values, and rendering measures terminal display width.

## Composition

Each component holds its own state, turns key events into an `Outcome` and draws
itself. Components know nothing about the app, so the app decides what a submit or
cancel means and composes them into dialogs as needed.

Keep a component open on `Ignored` and `Consumed`. In particular, key releases
return `Ignored`; they must not dismiss a confirmation. Validate values on
`Submit` before closing the component. An invalid value can stay open for editing.

| Component | Purpose |
| --- | --- |
| `TextInput` | Single line text editing with a character limit and a cursor position for the caller |
| `ListPicker` | Cursor over a list the caller owns, with scroll kept across frames |
| `Confirm` | A yes or no question answered by y or any other key |
| `Dialog` | Centered popup area with a cleared background and a border |
| `key_hints` | Formats `key: action` help text |

## List selection

Lists stay owned by the app. Pass their current length when handling input or
querying selection. `selected(len)` returns `None` for an empty list and clamps
to the last item after removals, matching the selection shown by `render`:

```rust
use rthid_widgets::ListPicker;

let picker = ListPicker::new(4);
let items = ["warm", "cool"];
assert_eq!(picker.selected(items.len()), Some(1));
assert_eq!(picker.selected(0), None);
```

`TextInput` filters control characters from initial and pasted text. `Confirm`
approves only an unmodified lowercase `y`; other pressed keys cancel.

See the crate docs (`cargo doc -p rthid-widgets --open`) for the API and examples.
