# Changelog

All notable changes to rthid-widgets are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `Outline` and `Row` provide independently collapsible groups with an unindented
  header, a shared item/selection gutter, first-item startup from the constructor's
  lens, and header actions through `hints`, `EXPAND`, `COLLAPSE`, and `TOGGLE`
- `Hint`, width-aware `hint_line`, and key hint constants on input widgets
- `KeyHelp` renders titled groups of display-width-aligned key references
- `ListPicker` mouse hint constants for click, repeated click, and wheel actions

### Breaking

- `key_hints(&[pairs])` -> `hint_line(&hints, None, width)` with `Hint` values
- `Confirm::HINTS` -> `[Confirm::YES, Confirm::CANCEL]`

## [0.2.0] - 2026-11-28

### Breaking

- `picker.selected()` -> `picker.selected(items.len())`, returning `Option<usize>` with `None` for an empty list
- Key releases returning `Ignored` -> `Consumed`, preventing parent fallback bindings
- `picker.index_at(area, position, len)` -> `picker.index_at(position, block, area, len)` with the drawing block and outer area
- `picker.handle_mouse(mouse, area, len)` -> `picker.handle_mouse(mouse, block, area, len)` with the drawing block and outer area
- Ctrl or Alt list navigation -> `Ignored` so parent bindings can handle these keys, with Shift still allowed

### Changed

- `TextInput::new` removes control characters before applying the length limit,
  matching pasted text
- `Confirm` accepts only an unmodified lowercase y; modified keys cancel

### Fixed

- The composition example relies on consumed key releases to protect fallback bindings
- `Dialog::area` documentation clarifies its four extra columns
- `TextInput` documentation explains that combining marks and ZWJ sequences can be split while editing

### Added

- `ListPicker::selected_row(area, block, len)` returns the visible selected screen row using the last draw
- Path dependency setup and a complete input example, compiled as a documentation test

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
