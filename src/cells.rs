//! Cell rendering of the Overview, 1:1 view and minimap.
//!
//! Paints into a ratatui buffer with half blocks or Sextant and Quadrant glyphs.
//! The minimap is also called the inset.

use crate::{
    memo::Memo,
    viewport::{CELL_ASPECT, Minimap, PreviewGeometry, Viewport, fit_aspect},
};
use image::RgbaImage;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Rect},
    style::Color,
    widgets::{Block, Clear, Widget},
};
use rthid::dither::PaletteColor;

/// Grid version and fitted cell dimensions for an RGB overview.
#[derive(PartialEq)]
pub struct OverviewKey {
    grid: u64,
    cols: u16,
    rows: u16,
}

/// One RGB overview retained for its grid version and cell dimensions.
pub type OverviewCache = Memo<OverviewKey, Vec<Color>>;

/// RGB overview colors for the main Overview and the minimap.
///
/// They are kept separate so toggling 1:1 does not rebuild either one.
#[derive(Default)]
pub struct OverviewCaches {
    /// Main Overview, shown when 1:1 is off.
    pub full: OverviewCache,
    /// Minimap overview, shown over the 1:1 view.
    pub inset: OverviewCache,
}

/// Borrowed pixels of the Original or Dithered layer.
#[derive(Clone, Copy)]
pub enum Pixels<'a> {
    /// Dithered layer as palette indices and their colors.
    Indexed {
        /// One palette index per grid pixel.
        indices: &'a [u8],
        /// Colors referenced by the indices.
        palette: &'a [PaletteColor],
    },
    /// Original layer as the scaled and rotated grid and its build version.
    /// The version keys the RGB overview caches.
    // TODO: Name the fields { image, version }.
    Rgb(&'a RgbaImage, u64),
}

impl Pixels<'_> {
    fn rgb(self, i: usize) -> [u8; 3] {
        match self {
            Self::Indexed { indices, palette } => palette[usize::from(indices[i])].rgb,
            Self::Rgb(pixels, _) => pixels.as_raw()[i * 4..i * 4 + 3].try_into().unwrap(),
        }
    }
}

/// Glyph set for the Dithered Overview and the Dithered minimap.
///
/// The Original layer and the 1:1 view always use half blocks.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyphs {
    /// 2x3 subcells from Symbols for Legacy Computing (U+1FB00 block). Sharper, but
    /// missing from some fonts.
    #[default]
    Sextant,
    /// 2x2 subcells from the standard block elements, supported almost everywhere.
    Quadrant,
}

impl Glyphs {
    /// Default glyph set for the terminal named by `TERM_PROGRAM`, used when the
    /// command line does not pick one.
    pub fn for_terminal(program: Option<&str>) -> Self {
        if program == Some("Apple_Terminal") {
            Self::Quadrant
        } else {
            Self::Sextant
        }
    }

    /// Returns the glyph label used in status text.
    pub fn name(self) -> &'static str {
        match self {
            Self::Sextant => "sextant",
            Self::Quadrant => "quadrant",
        }
    }

    fn subcells(self) -> (u32, u32) {
        match self {
            Self::Sextant => (2, 3),
            Self::Quadrant => (2, 2),
        }
    }

    // Glyph whose filled subcells match the mask. Bit n is subcell n in row major order.
    fn glyph(self, mask: u32) -> char {
        match self {
            Self::Quadrant => [
                ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
            ][mask as usize],
            Self::Sextant => match mask {
                0 => ' ',
                21 => '▌',
                42 => '▐',
                63 => '█',
                _ => {
                    char::from_u32(0x1fb00 + mask - 1 - u32::from(mask > 21) - u32::from(mask > 42))
                        .unwrap()
                }
            },
        }
    }
}

impl Viewport {
    // Clears the minimap frame and paints the fitted overview inside its border.
    // The border and dimming are drawn by draw_minimap.
    fn render_minimap(
        &self,
        pixels: Pixels<'_>,
        geometry: &PreviewGeometry,
        buffer: &mut Buffer,
        cache: &mut OverviewCache,
    ) {
        let Some(Minimap { frame, .. }) = geometry.minimap else {
            return;
        };
        // Clear letterboxing so the 1:1 dots cannot show through the inset.
        Clear.render(frame, buffer);
        render_overview(
            pixels,
            geometry.grid,
            self.glyphs,
            Block::bordered().inner(frame),
            buffer,
            cache,
        );
    }
}

/// Paints the preview and reports whether the Dithered Overview approximated colors.
///
/// The 1:1 view draws one half block per two dots plus the minimap and never reports
/// approximation.
pub fn render(
    pixels: Pixels<'_>,
    viewport: &Viewport,
    geometry: &PreviewGeometry,
    buffer: &mut Buffer,
    caches: &mut OverviewCaches,
) -> bool {
    let area = geometry.area;
    if area.is_empty() {
        return false;
    }
    if viewport.exact {
        render_exact(pixels, viewport, geometry, buffer, &mut caches.inset);
        false
    } else {
        render_overview(
            pixels,
            geometry.grid,
            viewport.glyphs,
            area,
            buffer,
            &mut caches.full,
        )
    }
}

fn render_exact(
    pixels: Pixels<'_>,
    viewport: &Viewport,
    geometry: &PreviewGeometry,
    buffer: &mut Buffer,
    cache: &mut OverviewCache,
) {
    let (w, _) = geometry.grid;
    let area = geometry.area;
    let (ox, oy) = geometry.origin;
    let color = |i| Color::from(pixels.rgb(i));
    let (vw, vh) = geometry.visible;
    for y in (0..vh).step_by(2) {
        for x in 0..vw {
            let index = ((oy + y) * w + ox + x) as usize;
            let bg = if y + 1 < vh {
                color(index + w as usize)
            } else {
                Color::Reset
            };
            buffer[(area.x + x as u16, area.y + (y / 2) as u16)]
                .set_symbol("▀")
                .set_fg(color(index))
                .set_bg(bg);
        }
    }
    draw_minimap(pixels, viewport, geometry, buffer, cache);
}

// Draws the minimap overview and its gray border, then dims map cells outside the
// viewport to half brightness.
fn draw_minimap(
    pixels: Pixels<'_>,
    viewport: &Viewport,
    geometry: &PreviewGeometry,
    buffer: &mut Buffer,
    cache: &mut OverviewCache,
) {
    let Some(Minimap {
        frame,
        map,
        viewport: viewport_rect,
    }) = geometry.minimap
    else {
        return;
    };
    viewport.render_minimap(pixels, geometry, buffer, cache);
    Block::bordered()
        .border_style(Color::Gray)
        .render(frame, buffer);
    for pos in map.positions().filter(|&pos| !viewport_rect.contains(pos)) {
        let cell = &mut buffer[pos];
        for color in [&mut cell.fg, &mut cell.bg] {
            if let Color::Rgb(r, g, b) = *color {
                *color = Color::Rgb(r / 2, g / 2, b / 2);
            }
        }
    }
}

// Draws the grid fitted and centered in `area`. Returns whether any Dithered cell
// was approximated.
fn render_overview(
    pixels: Pixels<'_>,
    grid: (u32, u32),
    glyphs: Glyphs,
    area: Rect,
    buffer: &mut Buffer,
    cache: &mut OverviewCache,
) -> bool {
    let (w, h) = grid;
    let (cw, ch) = fit_aspect(w, h, area, CELL_ASPECT);
    let target = area.centered(Constraint::Length(cw), Constraint::Length(ch));
    match pixels {
        Pixels::Rgb(pixels, version) => {
            render_overview_rgb(pixels, version, target, buffer, cache);
            false
        }
        Pixels::Indexed { indices, palette } => {
            render_overview_indexed(indices, palette, grid, glyphs, target, buffer)
        }
    }
}

// Draws averaged half blocks. Colors rebuild only when the grid version or target
// size changes.
fn render_overview_rgb(
    pixels: &RgbaImage,
    version: u64,
    target: Rect,
    buffer: &mut Buffer,
    cache: &mut OverviewCache,
) {
    let colors = cache.get_or_build(
        OverviewKey {
            grid: version,
            cols: target.width,
            rows: target.height,
        },
        || average_rgb(pixels, target.width, target.height),
    );
    let cw = usize::from(target.width);
    for cy in 0..target.height {
        for cx in 0..target.width {
            let index = usize::from(cy) * 2 * cw + usize::from(cx);
            buffer[(target.x + cx, target.y + cy)]
                .set_symbol("▀")
                .set_fg(colors[index])
                .set_bg(colors[index + cw]);
        }
    }
}

// Samples one dot at each subcell center without averaging and draws one glyph per
// cell. Returns whether any cell was approximated.
fn render_overview_indexed(
    indices: &[u8],
    palette: &[PaletteColor],
    grid: (u32, u32),
    glyphs: Glyphs,
    target: Rect,
    buffer: &mut Buffer,
) -> bool {
    let (w, h) = grid;
    let (cw, ch) = (target.width, target.height);
    let color = |i: u8| Color::from(palette[usize::from(i)].rgb);
    let (cols, rows) = glyphs.subcells();
    let mut approximate = false;
    for cy in 0..ch {
        for cx in 0..cw {
            let mut samples = [0u8; 6];
            let samples = &mut samples[..(cols * rows) as usize];
            for (n, sample) in samples.iter_mut().enumerate() {
                let sx = u32::from(cx) * cols + n as u32 % cols;
                let sy = u32::from(cy) * rows + n as u32 / cols;
                let x = ((u64::from(sx) * 2 + 1) * u64::from(w)
                    / (u64::from(cw) * u64::from(cols) * 2)) as u32;
                let y = ((u64::from(sy) * 2 + 1) * u64::from(h)
                    / (u64::from(ch) * u64::from(rows) * 2)) as u32;
                *sample = indices[(y * w + x) as usize];
            }
            let (mask, fg, bg, reduced) = reduce_cell(samples, palette);
            approximate |= reduced;
            let glyph = glyphs.glyph(mask);
            buffer[(target.x + cx, target.y + cy)]
                .set_char(glyph)
                .set_fg(color(fg))
                .set_bg(color(bg));
        }
    }
    approximate
}

// Reduces a cell to two colors. The most frequent sample becomes the background and
// the next ranked distinct one the foreground, with count ties going to the lower
// palette index. Other samples map to the nearer of the two, ties to the background.
// Returns the foreground mask, foreground, background, and whether any sample
// changed its RGB color.
fn reduce_cell(samples: &[u8], palette: &[PaletteColor]) -> (u32, u8, u8, bool) {
    let mut approximate = false;
    let mut counts = [0u8; 256];
    for &i in samples {
        counts[usize::from(i)] += 1;
    }
    let mut ranked = [0u8; 6];
    let ranked = &mut ranked[..samples.len()];
    ranked.copy_from_slice(samples);
    ranked.sort_unstable_by_key(|&i| (std::cmp::Reverse(counts[usize::from(i)]), i));
    let bg = ranked[0];
    let fg = ranked.iter().copied().find(|&i| i != bg).unwrap_or(bg);
    let distance = |a: u8, b: u8| -> i32 {
        palette[usize::from(a)]
            .rgb
            .iter()
            .zip(palette[usize::from(b)].rgb)
            .map(|(&a, b)| (i32::from(a) - i32::from(b)).pow(2))
            .sum()
    };
    let mut mask = 0u32;
    for (n, &i) in samples.iter().enumerate() {
        let kept = if i == fg || (i != bg && distance(i, fg) < distance(i, bg)) {
            fg
        } else {
            bg
        };
        approximate |= palette[usize::from(i)].rgb != palette[usize::from(kept)].rgb;
        if kept == fg && fg != bg {
            mask |= 1 << n;
        }
    }
    (mask, fg, bg, approximate)
}

// Averages the dots under each half cell. Returns two color rows per cell row, in
// row major order.
fn average_rgb(pixels: &RgbaImage, cw: u16, ch: u16) -> Vec<Color> {
    let (w, h) = pixels.dimensions();
    let average = |cx: u16, half_row: u32| {
        let x0 = u64::from(cx) * u64::from(w) / u64::from(cw);
        let x1 = ((u64::from(cx) + 1) * u64::from(w) / u64::from(cw)).max(x0 + 1);
        let y0 = u64::from(half_row) * u64::from(h) / (u64::from(ch) * 2);
        let y1 = ((u64::from(half_row) + 1) * u64::from(h) / (u64::from(ch) * 2)).max(y0 + 1);
        let mut sum = [0u64; 3];
        for y in y0..y1 {
            for x in x0..x1 {
                let [r, g, b, _] = pixels.get_pixel(x as u32, y as u32).0;
                sum[0] += u64::from(r);
                sum[1] += u64::from(g);
                sum[2] += u64::from(b);
            }
        }
        Color::from(sum.map(|total| (total / ((x1 - x0) * (y1 - y0))) as u8))
    };
    (0..u32::from(ch) * 2)
        .flat_map(|row| (0..cw).map(move |col| average(col, row)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewport::geometry;
    use image::Rgba;
    use ratatui::{buffer::Buffer, layout::Rect, style::Color};
    use rthid::dither::make_palette;

    #[test]
    fn cell_reduction_preserves_ties_masks_and_color_approximation() {
        let palette = make_palette(&["#000", "#333", "#fff"]).unwrap();
        for (samples, expected) in [
            (&[1, 1, 1, 1][..], (0, 1, 1, false)),
            (&[1, 0, 1, 0][..], (5, 1, 0, false)),
            (&[0, 2, 0, 2, 1, 2][..], (21, 0, 2, true)),
            (&[0, 1, 2, 2][..], (3, 0, 2, true)),
        ] {
            assert_eq!(reduce_cell(samples, &palette), expected);
        }
        let duplicate = make_palette(&["#000", "#000", "#fff"]).unwrap();
        assert_eq!(reduce_cell(&[0, 1, 2, 2], &duplicate), (3, 0, 2, false));
    }

    fn indexed<'a>(indices: &'a [u8], palette: &'a [PaletteColor]) -> Pixels<'a> {
        Pixels::Indexed { indices, palette }
    }

    fn rgb(w: u32, h: u32, pixels: &[[u8; 3]]) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let [r, g, b] = pixels[(y * w + x) as usize];
            Rgba([r, g, b, 255])
        })
    }

    #[test]
    fn glyph_masks_use_row_major_bits() {
        for (mask, glyph) in [(1, '▘'), (2, '▝'), (4, '▖'), (8, '▗')] {
            assert_eq!(Glyphs::Quadrant.glyph(mask), glyph, "mask {mask}");
        }
        for (mask, glyph) in [
            (1, '\u{1fb00}'),
            (21, '▌'),
            (22, '\u{1fb14}'),
            (42, '▐'),
            (63, '█'),
        ] {
            assert_eq!(Glyphs::Sextant.glyph(mask), glyph, "mask {mask}");
        }
    }

    #[test]
    fn glyph_default_depends_only_on_terminal_program() {
        for (program, expected) in [
            (Some("Apple_Terminal"), Glyphs::Quadrant),
            (Some("Zed"), Glyphs::Sextant),
            (Some("iTerm.app"), Glyphs::Sextant),
            (Some(""), Glyphs::Sextant),
            (None, Glyphs::Sextant),
        ] {
            assert_eq!(Glyphs::for_terminal(program), expected);
        }
    }

    #[test]
    fn quadrant_overview_and_minimap_use_only_block_elements() {
        let (w, h, palette, indices, mut viewport) = minimap_fixture();
        viewport.glyphs = Glyphs::Quadrant;
        let area = Rect::new(2, 3, 60, 30);
        let mut buffer = Buffer::empty(area);
        viewport.exact = false;
        render(
            indexed(&indices, &palette),
            &viewport,
            &crate::viewport::geometry(&viewport, (w, h), area),
            &mut buffer,
            &mut OverviewCaches::default(),
        );
        let is_block = |cell: &ratatui::buffer::Cell| {
            cell.symbol()
                .chars()
                .all(|c| c == ' ' || ('\u{2580}'..='\u{259f}').contains(&c))
        };
        assert!(buffer.content.iter().all(is_block));
        assert!(buffer.content.iter().any(|cell| cell.symbol() != " "));
        viewport.exact = true;
        let geometry = geometry(&viewport, (w, h), area);
        let Minimap { map, .. } = geometry.minimap.unwrap();
        viewport.render_minimap(
            indexed(&indices, &palette),
            &geometry,
            &mut buffer,
            &mut Memo::default(),
        );
        assert!(map.positions().all(|pos| is_block(&buffer[pos])));
        assert!(map.positions().any(|pos| buffer[pos].symbol() != " "));
    }

    fn minimap_fixture() -> (u32, u32, Vec<PaletteColor>, Vec<u8>, Viewport) {
        let (w, h) = (180, 180);
        let palette = make_palette(&["#123", "#abc", "#e64", "#fff"]).unwrap();
        let indices = (0..w * h)
            .map(|i| ((i * 17 + i / w * 13 + i / 7) % 4) as u8)
            .collect();
        let viewport = Viewport {
            exact: true,
            ..Viewport::default()
        };
        (w, h, palette, indices, viewport)
    }

    #[test]
    fn minimap_pan_preserves_symbols_and_dims_only_outside_viewport() {
        let area = Rect::new(2, 3, 60, 30);
        let (w, h, palette, indices, mut viewport) = minimap_fixture();
        let mut baseline = Buffer::empty(area);
        let geometry = geometry(&viewport, (w, h), area);
        let Minimap { frame, map, .. } = geometry.minimap.unwrap();
        viewport.render_minimap(
            indexed(&indices, &palette),
            &geometry,
            &mut baseline,
            &mut Memo::default(),
        );
        Block::bordered()
            .border_style(Color::Gray)
            .render(frame, &mut baseline);
        let mut current = Buffer::empty(area);
        for (x, y) in [(0, 0), (60, 60), (1, 1), (120, 120), (0, 0), (1, 0), (1, 1)] {
            viewport.x = x;
            viewport.y = y;
            let viewport_rect = viewport.viewport_rect(w, h, viewport.visible(w, h, area), map);
            render(
                indexed(&indices, &palette),
                &viewport,
                &crate::viewport::geometry(&viewport, (w, h), area),
                &mut current,
                &mut OverviewCaches::default(),
            );
            for pos in frame.positions() {
                let original = &baseline[pos];
                let cell = &current[pos];
                assert_eq!(
                    cell.symbol(),
                    original.symbol(),
                    "symbol at {pos:?}, pan {x},{y}"
                );
                let mut expected = original.clone();
                if map.contains(pos) && !viewport_rect.contains(pos) {
                    let dim = |color| match color {
                        Color::Rgb(17, 34, 51) => Color::Rgb(8, 17, 25),
                        Color::Rgb(170, 187, 204) => Color::Rgb(85, 93, 102),
                        Color::Rgb(238, 102, 68) => Color::Rgb(119, 51, 34),
                        Color::Rgb(255, 255, 255) => Color::Rgb(127, 127, 127),
                        _ => panic!("unexpected overview color: {color:?}"),
                    };
                    expected.set_fg(dim(original.fg)).set_bg(dim(original.bg));
                }
                assert_eq!(*cell, expected, "colors at {pos:?}, pan {x},{y}");
            }
        }
    }

    #[test]
    fn minimap_glyphs_are_single_width() {
        use ratatui::buffer::CellWidth;
        for ch in ('\u{1fb00}'..='\u{1fb3b}').chain(" ▘▝▀▖▌▞▛▗▚▐▜▄▙▟█┌┐└┘─│".chars())
        {
            assert_eq!(ch.to_string().cell_width(), 1, "{ch:?}");
        }
    }

    #[test]
    fn render_samples_centers_and_reduces_only_extra_colors() {
        let palette = make_palette(&["#000", "#333", "#fff"]).unwrap();
        let area = Rect::new(0, 0, 1, 1);
        for (glyphs, size, indices, approximate, symbol, fg, bg) in [
            (
                Glyphs::Sextant,
                (4, 3),
                vec![2, 0, 2, 1, 2, 0, 2, 1, 2, 0, 2, 1],
                false,
                "▐",
                51,
                0,
            ),
            (
                Glyphs::Sextant,
                (2, 3),
                vec![0, 2, 0, 2, 1, 2],
                true,
                "▌",
                0,
                255,
            ),
            (
                Glyphs::Quadrant,
                (4, 4),
                vec![2, 2, 2, 2, 2, 0, 2, 1, 2, 2, 2, 2, 2, 0, 2, 1],
                false,
                "▐",
                51,
                0,
            ),
            (
                Glyphs::Quadrant,
                (2, 2),
                vec![0, 2, 1, 2],
                true,
                "▌",
                0,
                255,
            ),
        ] {
            let viewport = Viewport {
                glyphs,
                ..Viewport::default()
            };
            let mut buffer = Buffer::empty(area);
            assert_eq!(
                render(
                    indexed(&indices, &palette),
                    &viewport,
                    &geometry(&viewport, size, area),
                    &mut buffer,
                    &mut OverviewCaches::default(),
                ),
                approximate,
                "{glyphs:?} {size:?}"
            );
            let cell = &buffer[(0, 0)];
            assert_eq!(cell.symbol(), symbol);
            assert_eq!(cell.fg, Color::Rgb(fg, fg, fg));
            assert_eq!(cell.bg, Color::Rgb(bg, bg, bg));
        }
    }

    #[test]
    fn rgb_overview_memo_tracks_grid_and_size() {
        let pixels = rgb(1, 2, &[[25, 26, 128], [231, 47, 93]]);
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        let mut cache = OverviewCaches::default();
        for (version, expected) in [(1, 1), (1, 1), (2, 2)] {
            render(
                Pixels::Rgb(&pixels, version),
                &Viewport::default(),
                &crate::viewport::geometry(&Viewport::default(), (1, 2), area),
                &mut buffer,
                &mut cache,
            );
            assert_eq!(cache.full.version(), expected);
        }
        let area = Rect::new(0, 0, 2, 2);
        render(
            Pixels::Rgb(&pixels, 2),
            &Viewport::default(),
            &crate::viewport::geometry(&Viewport::default(), (1, 2), area),
            &mut Buffer::empty(area),
            &mut cache,
        );
        assert_eq!(cache.full.version(), 3);
        let area = Rect::new(0, 0, 1, 1);
        let updated = rgb(1, 2, &[[0; 3], [255; 3]]);
        render(
            Pixels::Rgb(&updated, 3),
            &Viewport::default(),
            &crate::viewport::geometry(&Viewport::default(), (1, 2), area),
            &mut Buffer::empty(area),
            &mut cache,
        );
        assert_eq!(cache.full.version(), 4);
        assert_eq!(
            cache.full.get().unwrap(),
            &[Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255)]
        );
    }

    #[test]
    fn rgb_exact_preserves_each_pixel() {
        let pixels = RgbaImage::from_raw(1, 2, vec![25, 26, 128, 0, 231, 47, 93, 128]).unwrap();
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        assert!(!render(
            Pixels::Rgb(&pixels, 1),
            &Viewport {
                exact: true,
                ..Viewport::default()
            },
            &crate::viewport::geometry(
                &Viewport {
                    exact: true,
                    ..Viewport::default()
                },
                (1, 2),
                area
            ),
            &mut buffer,
            &mut OverviewCaches::default()
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▀");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(25, 26, 128));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(231, 47, 93));
    }

    #[test]
    fn rgb_overview_averages_half_cells_in_the_fitted_rectangle() {
        let pixels: Vec<_> = (0..16)
            .map(|i| if i % 4 < 2 { [0; 3] } else { [255; 3] })
            .collect();
        for (width, expected) in [(2, vec![0, 255]), (1, vec![127])] {
            let area = Rect::new(2, 3, width, 3);
            let mut buffer = Buffer::empty(area);
            assert!(!render(
                Pixels::Rgb(&rgb(4, 4, &pixels), 1),
                &Viewport::default(),
                &crate::viewport::geometry(&Viewport::default(), (4, 4), area),
                &mut buffer,
                &mut OverviewCaches::default()
            ));
            let (cw, ch) = fit_aspect(4, 4, area, CELL_ASPECT);
            let target = area.centered(Constraint::Length(cw), Constraint::Length(ch));
            assert_eq!((cw, ch), (width, 1));
            for (x, value) in expected.into_iter().enumerate() {
                let cell = &buffer[(target.x + x as u16, target.y)];
                assert_eq!(cell.symbol(), "▀");
                assert_eq!(cell.fg, Color::Rgb(value, value, value));
                assert_eq!(cell.bg, Color::Rgb(value, value, value));
            }
            assert!(
                area.positions()
                    .filter(|&pos| !target.contains(pos))
                    .all(|pos| buffer[pos].symbol() == " ")
            );
        }
    }

    #[test]
    fn rgb_overview_averages_source_rows_into_distinct_halves() {
        let pixels = [[0, 20, 100], [100, 40, 200], [200, 60, 0], [254, 80, 100]];
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        assert!(!render(
            Pixels::Rgb(&rgb(1, 4, &pixels), 1),
            &Viewport::default(),
            &crate::viewport::geometry(&Viewport::default(), (1, 4), area),
            &mut buffer,
            &mut OverviewCaches::default()
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▀");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(50, 30, 150));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(227, 70, 50));
    }

    #[test]
    fn rgb_overview_samples_at_least_one_pixel_when_enlarged() {
        let pixels = [[25, 26, 128], [231, 47, 93]];
        let area = Rect::new(0, 0, 3, 3);
        let mut buffer = Buffer::empty(area);
        assert!(!render(
            Pixels::Rgb(&rgb(1, 2, &pixels), 1),
            &Viewport::default(),
            &crate::viewport::geometry(&Viewport::default(), (1, 2), area),
            &mut buffer,
            &mut OverviewCaches::default()
        ));
        let (cw, ch) = fit_aspect(1, 2, area, CELL_ASPECT);
        let target = area.centered(Constraint::Length(cw), Constraint::Length(ch));
        assert_eq!((cw, ch), (3, 2));
        for y in 0..ch {
            for x in 0..cw {
                let cell = &buffer[(target.x + x, target.y + y)];
                let [r, g, b] = pixels[y as usize];
                assert_eq!(cell.symbol(), "▀");
                assert_eq!(cell.fg, Color::Rgb(r, g, b));
                assert_eq!(cell.bg, Color::Rgb(r, g, b));
            }
        }
    }

    #[test]
    fn exact_view_keeps_each_dot_and_odd_bottom_row() {
        let palette = make_palette(&["#000", "#333", "#fff"]).unwrap();
        let area = Rect::new(0, 0, 1, 2);
        let mut buffer = Buffer::empty(area);
        assert!(!render(
            indexed(&[0, 1, 2], &palette),
            &Viewport {
                exact: true,
                ..Viewport::default()
            },
            &crate::viewport::geometry(
                &Viewport {
                    exact: true,
                    ..Viewport::default()
                },
                (1, 3),
                area
            ),
            &mut buffer,
            &mut OverviewCaches::default()
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▀");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(0, 0, 0));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(51, 51, 51));
        assert_eq!(buffer[(0, 1)].fg, Color::Rgb(255, 255, 255));
        assert_eq!(buffer[(0, 1)].bg, Color::Reset);
    }
    #[test]
    fn minimap_fills_the_same_rectangle_used_for_projection() {
        let area = Rect::new(0, 0, 48, 18);
        let mut buffer = Buffer::empty(area);
        let palette = make_palette(&["#000", "#fff"]).unwrap();
        let viewport = Viewport {
            exact: true,
            ..Viewport::default()
        };
        render(
            indexed(&vec![0; 120 * 120], &palette),
            &viewport,
            &crate::viewport::geometry(&viewport, (120, 120), area),
            &mut buffer,
            &mut OverviewCaches::default(),
        );
        // The square grid fits 9x4 cells inside the 14x4 border of a 16x6 frame at x 32.
        assert_eq!(buffer[(32, 0)].symbol(), "┌");
        assert_eq!(buffer[(47, 5)].symbol(), "┘");
        assert_eq!(buffer[(33, 2)].symbol(), " ");
        assert_eq!(buffer[(33, 2)].bg, Color::Reset);
        let (_, map) = viewport.minimap(120, 120, area).unwrap();
        assert_eq!(buffer[(map.x, map.y)].fg, Color::Rgb(0, 0, 0));
    }
    #[test]
    fn rgb_overview_and_minimap_keep_separate_memos() {
        let pixels = RgbaImage::new(120, 120);
        let area = Rect::new(0, 0, 69, 36);
        let mut caches = OverviewCaches::default();
        for version in [1, 2] {
            for exact in [false, true, false, true] {
                let viewport = Viewport {
                    exact,
                    ..Viewport::default()
                };
                render(
                    Pixels::Rgb(&pixels, version),
                    &viewport,
                    &geometry(&viewport, pixels.dimensions(), area),
                    &mut Buffer::empty(area),
                    &mut caches,
                );
            }
            assert_eq!(caches.full.version(), version);
            assert_eq!(caches.inset.version(), version);
        }
    }
}
