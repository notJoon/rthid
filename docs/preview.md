# Preview viewport

## Synopsis

```text
rthid [image] [--preview-png path]
```

`image` defaults to `tests/fixtures/hsl.jpg`. Use a terminal with Unicode
sextant and half-block support.

## Display

- **Overview** (default): fits the whole dither grid into the preview.
  `approximate` means colors were reduced for terminal display; sampling may
  omit detail even without this label.
- **1:1**: shows each dither dot without resampling or color reduction.
  The status line shows inclusive, one-based x/y dot ranges.
- **Minimap**: appears in the top-right corner in 1:1. The visible region keeps
  its colors; the rest is dimmed. Hidden when the terminal is too small.
  Press `m` with preview focus to hide or show it.

## Keys

The parameter panel has focus initially. A cyan border marks the focused panel.

| Context | Key | Action |
| --- | --- | --- |
| Any | `Tab` | Switch focus between parameters and preview |
| Any | `Esc` | Return focus to parameters |
| Any | `r` / `R` | Rotate 90° clockwise / counterclockwise |
| Any | `d` / `D` | Reset selected parameter / all parameters and rotation |
| Any | `q` / `Ctrl+C` | Quit |
| Parameters | `↑` / `↓`, `k` / `j` | Select parameter |
| Parameters | `←` / `→`, `h` / `l` | Change value |
| Preview | `z` | Toggle Overview / 1:1 |
| Preview | `m` | Toggle minimap |
| Preview, 1:1 | Arrows or `h` / `j` / `k` / `l` | Pan one dot left / down / up / right |
| Preview, 1:1 | `H` / `J` / `K` / `L` | Pan one page left / down / up / right, with two dots of overlap |

Panning stops at grid edges. Focus and view changes do not re-dither the image.

## Mouse

In 1:1, mouse navigation works over the preview regardless of keyboard focus.

| Input | Action |
| --- | --- |
| Left click or drag on the minimap image | Center the viewport there |
| Left drag on the main preview | Move the image with the pointer |
| Wheel or horizontal trackpad scroll | Pan six dots per step in the scroll direction |

## Options

`--preview-png path`: save a PNG on startup and after each re-dither, replacing
any existing file at `path`. The destination directory must exist; the path
must differ from the source image. Each dot becomes a solid square at an
integer scale of 1–4, keeping the long side within 2048 px unless the grid
itself is larger. Write errors appear in the status line; the next re-dither
retries. Quit waits for the final export and reports failures with a nonzero
exit status. No viewer is opened.

Example:

```sh
rthid photo.png --preview-png /tmp/preview.png
```

## Environment

`RTHID_DEBUG_LOG`: append frame and input diagnostics to this file for terminal
rendering issues. Enable only while reproducing an issue: logs can grow quickly
and slow rendering. An inaccessible path prevents startup; a write failure
exits the application.

```sh
RTHID_DEBUG_LOG=/tmp/rthid-minimap.log rthid photo.png
```

To report a rendering issue, reproduce it in 1:1, quit, and share the log with
the pan direction and terminal size.
