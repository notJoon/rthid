# Preview viewport

## Synopsis

```text
rthid [image] [--preview-png path] [--mass-svg path] [--dpi N] [--glyphs sextant|quadrant] [--mass-tones palette|mono|warm] [--mass-invert] [--graphics auto|off] [--config path]
```

Use `rthid --help` for option details and `rthid --version` for the version.

Omit `image` to start with an empty preview. Use a terminal
with Unicode half block support and support for the selected overview glyphs
(sextants or quadrants).

## Display

Original, Dithered or Mass selects the image layer. Zoom selects its display scale.
The preview starts with Original at Fit.

- **Original / Dithered**: Original shows the composed card grid, including white paper outside the image, as real pixels
  via Kitty, iTerm2, or Sixel at Fit when supported. The status line names the
  protocol only while drawing an image. Graphics preserve aspect using the
  terminal's cell pixel size. Otherwise Original uses truecolor half blocks,
  averaged at Fit and reduced zoom levels. Press `Space` to toggle the layer.
  From Mass, `Space` shows Dithered.
  Dithering is deferred while Original is shown unless a companion PNG is enabled.
  Changing a dither parameter reveals Dithered unless Mass is shown. Print controls,
  rotation, flip, and resets keep the selected layer. PNG exports always use the dithered result.
- **Mass**: a flat four tone study of the undithered card grid for judging
  composition: where the large dark and light masses sit, without the dither
  pattern. Tones run background, base, shadow and accent, from no ink to the
  densest ink. Press `a` to show it and `a` again to return to the previous layer.
  Mass uses truecolor half blocks and the status line names it `Mass`. Palette
  changes recolor its tones. The study is built only while a view shows it, and
  a build error appears in the status line while it is visible.
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
  It shows the mass study by default, so the main view keeps detail while the
  minimap shows composition. Press `M` to switch it between the mass study and the
  main layer. While it shows the mass study and the main layer is not Mass, the
  status line ends with `| map Mass`. Its frame fits the map.
  The visible region keeps its colors and the rest is dimmed. It is hidden when
  the terminal is too small. Press `m` in either focus to hide or show it.

## Print grid

Card cycles through Image, Business 50x90, Business 90x50, Euro business 55x85,
Trading 63x88, Square 65x65 and Postcard 100x148. Fixed card sizes are in millimetres.
The default is Image with 0.20 mm dots. Image keeps one dot per loaded source
pixel and follows its rotation, so the whole image is visible at Zoom 1.00x and
zero pan. Loading shrinks the source to at most 1024 pixels on its long side.
Dot changes Image's physical size, not its image area dot count. Margin and
bleed add paper around the image area, rounded together to the nearest dot on
each side. Its physical size including paper is the grid size times Dot.
Without a loaded source, settings validation uses a single pixel placeholder
for Image and the preview shows `No image`.

| Parameter | Range | Step | Default |
| --- | --- | --- | --- |
| Dot | 0.08 to 0.60 mm | 0.01 mm | 0.20 mm |
| Margin | 0 to 10 mm | 0.5 mm | 0 mm |
| Bleed | 0 to 3 mm | 1 mm | 0 mm |
| Zoom | 0.30x to 4.00x | 0.05x | 1.00x |
| Pan X / Pan Y | -1.00 to +1.00 | 0.05 | 0.00 |

On fixed cards, bleed extends every edge of the card. Each axis rounds to the nearest dot,
with halves rounded up, and has a minimum of eight dots. Grids above four
million dots are rejected. Every preset fits this limit at every allowed setting.
Margin zero fills the bleed. A positive margin reserves both bleed and margin
as paper. If either remaining image axis is at most two dots, the whole grid is
paper and the status shows `no image area`.

The source covers the image area at Zoom 1.00x. Smaller values can expose paper.
Rotation turns the source inside fixed cards and turns the Image card with the source. `f` mirrors left and right after
rotation, before cover placement. Pan directions stay relative to the card.
Positive Pan X moves toward right edge alignment and positive Pan Y toward
bottom edge alignment. Negative values align the opposite edges. Paper always uses the brightest palette color
in Dithered, regardless of tone settings. Original uses white RGB for paper.

These composition controls change the output. The preview zoom keys and preview
panning only navigate the result. `d` restores the selected control.

For Business 50x90, the status starts with `Card 50x90mm | Dots 250x450 @0.20mm | Rot 0°`, where
`@0.20mm` is the dot size. When flipped, the rotation reads, for example,
`Rot 90° Flip`. With bleed it shows, for example, `Card 50x90+3mm`.
For Image, the card dimensions include the rounded margin and bleed paper,
for example `Card 60x40mm` for a 300x200 source at 0.20 mm with no insets.
The layer and preview navigation information follow these fields.
Long status lines are clipped to the terminal width, so in 80 columns the print
fields stay visible and the layer and navigation fields are cut first.

## Keys

The parameter panel has focus initially. A cyan border marks the focused panel.

| Context | Key | Action |
| --- | --- | --- |
| Any, outside modals | `o` | Open an image |
| Any, outside modals | `P` / `T` | Manage palette presets / settings templates |
| Any | `Tab` | Switch focus between parameters and preview |
| Any | `Space` | Toggle original / dithered, or show dithered from mass; compute a pending result if needed |
| Any | `a` | Show mass / return to the previous layer |
| Any | `Esc` | Return focus to parameters |
| Any | `r` / `R` | Rotate 90° clockwise / counterclockwise |
| Any, outside modals | `f` | Flip left and right on the card |
| Any | `d` | Reset selected parameter |
| Any | `q` / `Ctrl+C` | Quit |
| Parameters | `↑` / `↓`, `k` / `j` | Select parameter |
| Parameters, Seed row with Noise | `Enter` | Draw a new noise pattern |
| Parameters | `←` / `→`, `h` / `l` | Change value |
| Any | `+` / `=` / `-` | Zoom in / in / out |
| Any | `0` | Return to Fit |
| Any | `m` | Toggle minimap |
| Any | `M` | Switch minimap between mass study and main layer |
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
field. Opening an image keeps processing settings, resets rotation, flip, image
zoom, image pan and preview navigation, and shows Original at Fit. A failed open keeps the current image.

## Presets and templates

`P` opens presets: arrows or `j`/`k` select, `Enter` applies, `n` creates,
`e` edits a copy, and `x` deletes a user preset after confirmation. In the
editor, `Enter` edits a field, `a` adds a color, `x` removes a color, and `s` saves.
`Esc` cancels an edit or closes the dialog.

`T` opens templates: arrows or `j`/`k` select, `Enter` restores settings and
shows Dithered while keeping rotation, flip, image zoom and image pan. These
composition values are not stored in templates. `n` saves current settings under
a name of up to 32 characters, and `x` asks to delete. Names must be nonempty and
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
card = "business-portrait"
dot_mm = 0.2
margin_mm = 1.5
bleed_mm = 3
```

Algorithm names: `atkinson`, `floyd-steinberg`, `jarvis-judice-ninke`,
`stucki`, `sierra-lite`, `bayer2`, `bayer4`, `bayer8`, `noise`.

Strength and Serpentine apply to diffusion algorithms. Seed applies to Noise.
Unused rows stay visible and dimmed, and their values survive algorithm changes.

Templates keep their own colors, card, dot size, margin and bleed, so later
preset edits or removal do not alter them. Card ids are `image`, `business-portrait`,
`business-landscape`, `euro-business`, `trading`, `square` and `postcard`.
Unknown ids use Image. Numeric print values round to their stored
units, the margin snaps to its 0.5 mm step, and all clamp to the control bounds. Old `scale` fields are ignored and missing
print fields use defaults. The config version remains 1.

## Mouse

When the image overflows the preview, mouse navigation works over it at any
zoom regardless of keyboard focus.

| Input | Action |
| --- | --- |
| Left click or drag on the minimap image | Center the viewport there; a drag that leaves the map pauses until it returns |
| Left drag on the main preview | Move the image with the pointer, including over the minimap |
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

`--mass-tones palette|mono|warm`: select the Mass tones. `palette`, the default,
samples four tones along the dither palette sorted by luma, so the study follows
the selected palette. `mono` and `warm` are fixed sets.

`--mass-invert`: treat light pixels as ink in the mass study, for light subjects
on dark backgrounds. With `palette` tones the darkest palette color becomes the
background. Fixed tone sets keep their order.

`--preview-png path`: save a PNG on startup and after each re-dither, replacing
any existing file at `path`. The destination directory must exist; the path
must differ from the source image. Write errors appear in the status line and
another re-dither retries. Quit waits for the final export and reports failures
with a nonzero exit status. No viewer is opened.

`--mass-svg path`: save a self-contained mass-study analysis sheet on startup
and after each mass rebuild. It embeds the original preview, vector tone layers,
fitted accent ellipses, and settings. The path must differ from the source and
from `--preview-png`; quit waits for the final write.

## PNG sizing

Without `--dpi`, each dot becomes a solid square at an integer scale of 1 to 4,
keeping the long side within 2048 pixels unless the grid itself is larger.
The PNG contains no resolution metadata and retains the existing encoder bytes.

`--dpi N` requires `--preview-png` and accepts integers from 1 to 4800. Each PNG axis is
`round((card mm + 2 * bleed mm) / 25.4 * N)` for fixed cards. Dot size does not
change their pixel dimensions. For Image, each axis is
`round(grid dots * dot mm / 25.4 * N)`, including the paper insets, so Dot
changes the PNG size. Nearest sampling keeps dot boundaries, including when pixels
per dot are not integers. The PNG records resolution in pixels per metre on both
axes, rounded from `N / 0.0254`.

At 600 DPI, the Business 50x90 mm card produces 1181x2126 pixels. With 3 mm bleed
it produces 1323x2268 pixels. The status includes `PNG 1181x2126 @600dpi` when DPI
is set. Output sides must be positive and at most `i32::MAX`, and output area
must not exceed 268435456 pixels. Invalid sizes appear as export errors before
any job is submitted. The source image still shrinks to 1024 pixels on its long
side when loaded.

```sh
rthid photo.png --preview-png /tmp/preview.png
rthid photo.png --preview-png /tmp/print.png --dpi 600
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
