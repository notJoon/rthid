# Preview viewport

## Synopsis

```text
rthid [image] [--preview-png path] [--glyphs sextant|quadrant] [--graphics auto|off] [--config path]
```

Use `rthid --help` for option details and `rthid --version` for the version.

Omit `image` to start with an empty preview. Use a terminal
with Unicode half block support and support for the selected overview glyphs
(sextants or quadrants).

## Display

Original / Dithered selects the image layer. Zoom selects its display scale.
The preview starts with Original at Fit.

- **Original / Dithered**: Original shows the scaled, rotated source as real pixels
  via Kitty, iTerm2, or Sixel at Fit when supported. The status line names the
  protocol only while drawing an image. Graphics preserve aspect using the
  terminal's cell pixel size. Otherwise Original uses truecolor half blocks,
  averaged at Fit and reduced zoom levels. Press `Space` to toggle the layer.
  Dithering is deferred while Original is shown unless a companion PNG is enabled.
  Changing a dither parameter reveals Dithered. Scale, rotation, and resets keep
  the selected layer. PNG exports always use the dithered result.
- **Fit**: shows the whole grid with terminal cell aspect correction. For Dithered,
  `Fit (sextant)` or `Fit (quadrant)` names the sampling glyphs. `approximate`
  means samples needed color reduction. Sampling may omit detail without this label.
- **Zoom levels**: Fit, 25%, 33%, 50%, 100%, 200%, 300%, and 400%.
  `+` or `=` zooms in, `-` zooms out, and `0` returns to Fit. Steps no larger
  than Fit are skipped. Zoom keeps the center dot fixed, then clamps at edges.
  At 25%, 33%, and 50%, each cell column covers four, three, or two dots.
  These levels preserve the true aspect and use the same sampling as Fit.
  At 100% (1:1), each dot occupies one cell column and one half cell in height.
  At 200%, 300%, and 400%, dots occupy two, three, or four columns and half cells.
  These crisp half blocks make dots look 12.5 percent taller.
  The status line shows inclusive, one based x/y dot ranges at every level except Fit.
- **Minimap**: appears in the upper right whenever the image overflows the preview.
  The visible region keeps its colors and the rest is dimmed. It is hidden when
  the terminal is too small. Press `m` in either focus to hide or show it.

## Keys

The parameter panel has focus initially. A cyan border marks the focused panel.

| Context | Key | Action |
| --- | --- | --- |
| Any, outside modals | `o` | Open an image |
| Any, outside modals | `P` / `T` | Manage palette presets / settings templates |
| Any | `Tab` | Switch focus between parameters and preview |
| Any | `Space` | Toggle original / dithered; compute a pending result if needed |
| Any | `Esc` | Return focus to parameters |
| Any | `r` / `R` | Rotate 90° clockwise / counterclockwise |
| Any | `d` / `D` | Reset selected parameter / all parameters and rotation |
| Any | `q` / `Ctrl+C` | Quit |
| Parameters | `↑` / `↓`, `k` / `j` | Select parameter |
| Parameters, Seed row with Noise | `Enter` | Draw a new noise pattern |
| Parameters | `←` / `→`, `h` / `l` | Change value |
| Any | `+` / `=` / `-` | Zoom in / in / out |
| Any | `0` | Return to Fit |
| Any | `m` | Toggle minimap |
| Preview | Arrows or `h` / `j` / `k` / `l` | Pan one cell worth of dots, at least one dot |
| Preview | `H` / `J` / `K` / `L` | Pan one visible page with two cells of overlap, at least one step |

Panning works whenever the image overflows, including reduced zoom levels, and
stops at grid edges. Fit does not pan. Focus, panning, zoom, and minimap changes
do not re-dither the image.

## Opening images

Start without an image to see an empty preview. Press `o` with or without an
image loaded to open the file dialog. It starts in the current image's directory,
or `.` before an image is loaded. Hidden entries are omitted. Directories appear
first with a trailing `/`, followed by supported image files, sorted by name.

Use `Up` / `Down` to select and `Enter` to open a directory or image. `..` goes
to the parent directory. Type or paste a path into the field and press `Enter`
to open it. `~` and `~/` expand to your home directory. Left and Right edit the
field, and letters including `j` and `k` always enter text. After Up or Down,
Enter opens the selected entry again. `Esc` closes the dialog. An open error
appears in the status line and leaves the dialog open. Click an entry to select
it and click it again to open it. The wheel moves the selection.

You can also drag and drop an image outside a dialog when your terminal sends
bracketed paste. Quoted paths, escaped spaces and local file URIs are accepted.
Only the first path is opened. Pasting into a dialog inserts text in its active
field. Opening an image keeps processing settings, resets rotation and navigation,
and shows Original at Fit. A failed open keeps the current image.

## Presets and templates

`P` opens presets: arrows or `j`/`k` select, `Enter` applies, `n` creates,
`e` edits a copy, and `x` deletes a user preset after confirmation. In the
editor, `Enter` edits a field, `a` adds a color, `x` removes a color, and `s` saves.
`Esc` cancels an edit or closes the dialog.

`T` opens templates: arrows or `j`/`k` select, `Enter` restores settings and
shows Dithered while keeping rotation, `n` saves current settings under a name
of up to 32 characters, and `x` asks to delete. Names must be nonempty and
cannot match a built-in preset. Reusing a template name asks to overwrite with
`y`; any other key returns to the name prompt. `Esc` returns from naming to
the list or closes the list. Click to select a preset, template, or editor row,
then click again to apply or edit it. The wheel moves one row. Mouse input is
ignored during confirmations and while editing a name or editor field. Modal
keys appear in the help line.

The config path lookup order is `--config path`, `RTHID_CONFIG`,
`$XDG_CONFIG_HOME/rthid/config.toml` when `XDG_CONFIG_HOME` is absolute, then
`$HOME/.config/rthid/config.toml`. Changes save automatically with atomic
replacement. Load and save errors appear in the status line.

Example `config.toml` (omitted template settings use defaults):

```toml
version = 1

[[palette]]
name = "Paper"
colors = ["#123456", "#fff"]
mode = "color"

[[template]]
name = "Zine"
palette = "Paper"
colors = ["#123456", "#fff"]
mode = "color"
algorithm = "bayer4"
seed = 0
strength = 0.8
```

Algorithm names: `atkinson`, `floyd-steinberg`, `jarvis-judice-ninke`,
`stucki`, `sierra-lite`, `bayer2`, `bayer4`, `bayer8`, `noise`.

Strength and Serpentine apply to diffusion algorithms. Seed applies to Noise.
Unused rows stay visible and dimmed, and their values survive algorithm changes.

Templates keep their own colors, so later preset edits or removal do not alter them.

## Mouse

When the image overflows the preview, mouse navigation works over it at any
zoom regardless of keyboard focus.

| Input | Action |
| --- | --- |
| Left click or drag on the minimap image | Center the viewport there |
| Left drag on the main preview | Move the image with the pointer |
| Wheel or horizontal trackpad scroll | Pan six cells worth of dots per step in the scroll direction |

## Options

`--graphics auto|off`: detect Kitty, iTerm2, or Sixel on startup (`auto`, the default).
Detection waits about 200 ms and is skipped on Apple Terminal.
iTerm2 always uses its own image protocol, because its Kitty support leaves the
placeholder cells rthid draws blank.
If detection fails or no graphics protocol is available, Original uses half blocks.
`off` skips the terminal query and always uses half blocks. Dithered stays cell-rendered.

`--glyphs sextant|quadrant`: select Dithered sampling and minimap glyphs:
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
rendering issues. `[timing]` lines record elapsed milliseconds for `decode` and
`list` with their paths, `sync` when the grid or dither rebuilt, `save` for config
writes and `encode` for completed background PNG writes. Enable only while reproducing an issue: logs can grow quickly
and slow rendering. An inaccessible path prevents startup; a write failure
exits the application.

```sh
RTHID_DEBUG_LOG=/tmp/rthid-minimap.log rthid photo.png
```

To report a rendering issue, reproduce it in 1:1, quit, and share the log with
the pan direction and terminal size.

## Troubleshooting

**Fit, reduced zoom, or minimap shows `?` boxes.** The terminal's font has no sextant
glyphs (U+1FB00–U+1FB3B), as in macOS Terminal.app. Use quadrants, which only
need standard block elements:

```sh
rthid photo.png --glyphs quadrant
```

To check a terminal, run `printf '\U1FB00 \U1FB0F \U1FB3B'`: boxes mean sextants
are unsupported. Quadrants are the default when `TERM_PROGRAM` is
`Apple_Terminal`; other terminals cannot be detected, so pass `--glyphs`
explicitly. Quadrants sample 2×2 dots per cell instead of 2×3, so the overview
shows less detail. Zoom levels from 100% upward are unaffected.
