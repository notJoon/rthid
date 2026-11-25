# Preview viewport

## Synopsis

```text
rthid [image] [--preview-png path] [--glyphs sextant|quadrant] [--graphics auto|off]
```

Use `rthid --help` for option details and `rthid --version` for the version.

`image` defaults to `tests/fixtures/hsl.jpg`. Use a terminal with Unicode
half-block support and support for the selected overview glyphs (sextants or
quadrants).

## Display

Original / Dithered selects the image layer; Overview / 1:1 selects how it is
shown. The preview starts with Original in Overview.

- **Original / Dithered**: Original shows the scaled, rotated source as real pixels
  via Kitty, iTerm2, or Sixel in Overview when supported. Otherwise, including
  in 1:1, it uses truecolor half blocks. The status line names the graphics protocol
  only while drawing an image. Graphics overview preserves aspect using the terminal's cell pixel size.
  Half-block overview averages pixels per half cell. Press `Space` to toggle
  between original and dithered. Dithering is deferred while Original is shown
  unless a companion PNG is enabled.
  Changing a dither parameter reveals the dithered result; scale, rotation,
  and resets keep the current view. PNG exports always use the dithered result.
- **Overview**: fits the whole grid into the preview. For Dithered,
  `approximate` means colors were reduced for terminal display; sampling may
  omit detail even without this label.
- **1:1**: shows each grid pixel as half a cell without resampling or color reduction.
  The status line shows inclusive, one-based x/y dot ranges.
- **Minimap**: appears in the top-right corner in 1:1. The visible region keeps
  its colors; the rest is dimmed. Hidden when the terminal is too small.
  Press `m` with preview focus to hide or show it.

## Keys

The parameter panel has focus initially. A cyan border marks the focused panel.

| Context | Key | Action |
| --- | --- | --- |
| Any | `Tab` | Switch focus between parameters and preview |
| Any | `Space` | Toggle original / dithered; compute a pending result if needed |
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

Panning stops at grid edges. Focus, panning, and Overview / 1:1 changes do not re-dither the image.

## Mouse

In 1:1, mouse navigation works over the preview regardless of keyboard focus.

| Input | Action |
| --- | --- |
| Left click or drag on the minimap image | Center the viewport there |
| Left drag on the main preview | Move the image with the pointer |
| Wheel or horizontal trackpad scroll | Pan six dots per step in the scroll direction |

## Options

`--graphics auto|off`: detect Kitty, iTerm2, or Sixel on startup (`auto`, the default).
Detection waits about 200 ms and is skipped on Apple Terminal.
If detection fails or no graphics protocol is available, Original uses half blocks.
`off` skips the terminal query and always uses half blocks. Dithered stays cell-rendered.

`--glyphs sextant|quadrant`: select Dithered overview and minimap glyphs:
sextants use 2×3 sub-cells; quadrants use 2×2. Defaults to `quadrant` when `TERM_PROGRAM`
is `Apple_Terminal`, otherwise `sextant`. An explicit value overrides the default.

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

## Troubleshooting

**Overview or minimap shows `?` boxes.** The terminal's font has no sextant
glyphs (U+1FB00–U+1FB3B), as in macOS Terminal.app. Use quadrants, which only
need standard block elements:

```sh
rthid photo.png --glyphs quadrant
```

To check a terminal, run `printf '\U1FB00 \U1FB0F \U1FB3B'`: boxes mean sextants
are unsupported. Quadrants are the default when `TERM_PROGRAM` is
`Apple_Terminal`; other terminals cannot be detected, so pass `--glyphs`
explicitly. Quadrants sample 2×2 dots per cell instead of 2×3, so the overview
shows less detail; 1:1 is unaffected.
