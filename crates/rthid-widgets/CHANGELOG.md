# Changelog

All notable changes to rthid-widgets are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - 2026-11-26

### Added

- `TextInput::insert_str` inserts printable paste text in one pass within the character limit

- `ListPicker::select`, `index_at` and `handle_mouse` for selection, scroll aware
  hit testing and clicking the selected item to submit

- `Outcome` key handling result (`Ignored`, `Consumed`, `Submit`, `Cancel`), so a
  parent can fall back to its own bindings for keys a component does not use
- `TextInput`: single line editing with a character limit, sideways scrolling and
  a returned cursor position
- `ListPicker`: cursor over a caller owned list with clamping after removals and a
  scroll offset kept across frames while drawing from a shared borrow
- `Confirm`: a question answered by y, with any other key cancelling
- `Dialog`: centered popup area clamped to the screen with a cleared background
  and a border
- `key_hints`: `key: action` help text joined from tables
- Key release events are ignored, so terminals that report releases do not apply
  each key twice
