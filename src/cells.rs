//! Cell rendering of the visible dot region at each zoom and the minimap.
//!
//! Paints into a ratatui buffer with half blocks or Sextant and Quadrant glyphs.
//! The minimap is also called the inset.

use crate::{
    memo::Memo,
    viewport::{CELL_ASPECT, Minimap, PreviewGeometry, Viewport, Zoom},
};
use image::RgbaImage;
use ratatui::{
    buffer::{Buffer, Cell},
    style::Color,
    widgets::{Block, Clear, Widget},
};
use rthid_dither::dither::PaletteColor;

/// Grid version, visible region, zoom and target size for averaged RGB cells.
#[derive(PartialEq)]
pub struct OverviewKey {
    grid: u64,
    cols: u16,
    rows: u16,
    origin: (u32, u32),
    visible: (u32, u32),
    zoom: Zoom,
}

/// One RGB overview retained for its grid version and cell dimensions.
pub type OverviewCache = Memo<OverviewKey, Vec<Color>>;

/// Averaged RGB colors for the sampled preview and minimap.
///
/// Separate caches preserve the fitted image while zooming.
#[derive(Default)]
pub struct OverviewCaches {
    /// Main sampled preview, including its visible region.
    pub full: OverviewCache,
    /// Fitted minimap image.
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

/// Glyph set for sampled Dithered previews and the minimap.
///
/// Original and enlarged dots use half blocks.
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

// Clears the minimap frame and paints the fitted overview inside its border.
// The border and dimming are drawn by draw_minimap.
fn render_minimap(
    pixels: Pixels<'_>,
    glyphs: Glyphs,
    geometry: &PreviewGeometry,
    buffer: &mut Buffer,
    cache: &mut OverviewCache,
) {
    let Some(Minimap { frame, .. }) = geometry.minimap else {
        return;
    };
    // Letterboxing hides the main preview beneath the inset.
    Clear.render(frame, buffer);
    let fitted = crate::viewport::geometry(
        &Viewport::default(),
        geometry.grid,
        Block::bordered().inner(frame),
    );
    render_overview(pixels, glyphs, &fitted, buffer, cache);
}

/// Paints the visible dot region and minimap with the chosen glyphs, reporting
/// sampled Dithered approximation.
pub fn render(
    pixels: Pixels<'_>,
    glyphs: Glyphs,
    geometry: &PreviewGeometry,
    buffer: &mut Buffer,
    caches: &mut OverviewCaches,
) -> bool {
    if geometry.target.is_empty() {
        return false;
    }
    let approximate = if let Zoom::In(k) = geometry.zoom {
        render_exact(pixels, geometry, k, buffer);
        false
    } else {
        render_overview(pixels, glyphs, geometry, buffer, &mut caches.full)
    };
    draw_minimap(pixels, glyphs, geometry, buffer, &mut caches.inset);
    approximate
}

// Solid backgrounds avoid redundant foreground color output.
fn write_cell(cell: &mut Cell, glyph: char, fg: Color, bg: Color) {
    let (glyph, fg) = if glyph == ' ' || fg == bg {
        (' ', Color::Reset)
    } else {
        (glyph, fg)
    };
    cell.set_char(glyph).set_fg(fg).set_bg(bg);
}

/// Paints In(k) dot by dot, as drawn in the In(k) diagrams on [`Zoom`].
fn render_exact(pixels: Pixels<'_>, geometry: &PreviewGeometry, k: u8, buffer: &mut Buffer) {
    let (w, _) = geometry.grid;
    let area = geometry.target;
    let (ox, oy) = geometry.origin;
    // Paint the partially visible edge dot that `visible` leaves out for navigation.
    let (vw, vh) = geometry.drawn;
    let k = u32::from(k);
    let color = |x, half_y| {
        let (x, y) = (x / k, half_y / k);
        if x < vw && y < vh {
            let rgb = pixels.rgb(((oy + y) * w + ox + x) as usize);
            Color::from(rgb)
        } else {
            Color::Reset
        }
    };
    for y in 0..area.height {
        for x in 0..area.width {
            write_cell(
                &mut buffer[(area.x + x, area.y + y)],
                '▀',
                color(u32::from(x), u32::from(y) * 2),
                color(u32::from(x), u32::from(y) * 2 + 1),
            );
        }
    }
}

// Draws the minimap overview and its gray border, then dims map cells outside the
// viewport to half brightness.
fn draw_minimap(
    pixels: Pixels<'_>,
    glyphs: Glyphs,
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
    render_minimap(pixels, glyphs, geometry, buffer, cache);
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

/// Samples the visible region into its target cells at Fit and Out(n), as drawn in
/// the sampled cell diagram on [`Zoom`].
fn render_overview(
    pixels: Pixels<'_>,
    glyphs: Glyphs,
    geometry: &PreviewGeometry,
    buffer: &mut Buffer,
    cache: &mut OverviewCache,
) -> bool {
    match pixels {
        Pixels::Rgb(pixels, version) => {
            render_overview_rgb(pixels, version, geometry, buffer, cache);
            false
        }
        Pixels::Indexed { indices, palette } => {
            render_overview_indexed(indices, palette, glyphs, geometry, buffer)
        }
    }
}

/// Draws averaged half blocks for Original, see the Original column on [`Zoom`].
/// Colors rebuild only when the grid version, visible region, zoom or target size changes.
fn render_overview_rgb(
    pixels: &RgbaImage,
    version: u64,
    geometry: &PreviewGeometry,
    buffer: &mut Buffer,
    cache: &mut OverviewCache,
) {
    let target = geometry.target;
    let colors = cache.get_or_build(
        OverviewKey {
            grid: version,
            cols: target.width,
            rows: target.height,
            origin: geometry.origin,
            visible: geometry.visible,
            zoom: geometry.zoom,
        },
        || average_rgb(pixels, geometry),
    );
    let cw = usize::from(target.width);
    for cy in 0..target.height {
        for cx in 0..target.width {
            let index = usize::from(cy) * 2 * cw + usize::from(cx);
            write_cell(
                &mut buffer[(target.x + cx, target.y + cy)],
                '▀',
                colors[index],
                colors[index + cw],
            );
        }
    }
}

/// Samples one dot at each subcell center without averaging and draws one glyph per
/// cell, see the Quadrant and Sextant columns on [`Zoom`]. Returns whether any cell
/// was approximated.
fn render_overview_indexed(
    indices: &[u8],
    palette: &[PaletteColor],
    glyphs: Glyphs,
    geometry: &PreviewGeometry,
    buffer: &mut Buffer,
) -> bool {
    let (w, h) = geometry.visible;
    let (ox, oy) = geometry.origin;
    let target = geometry.target;
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
                let (x, y) = if let Zoom::Out(n) = geometry.zoom {
                    (
                        ((f64::from(sx) + 0.5) * f64::from(n) / f64::from(cols)) as u32,
                        ((f64::from(sy) + 0.5) * f64::from(n) * CELL_ASPECT / f64::from(rows))
                            as u32,
                    )
                } else {
                    (
                        ((u64::from(sx) * 2 + 1) * u64::from(w)
                            / (u64::from(cw) * u64::from(cols) * 2)) as u32,
                        ((u64::from(sy) * 2 + 1) * u64::from(h)
                            / (u64::from(ch) * u64::from(rows) * 2)) as u32,
                    )
                };
                let (x, y) = (x.min(w - 1), y.min(h - 1));
                *sample = indices[((oy + y) * geometry.grid.0 + ox + x) as usize];
            }
            let (mask, fg, bg, reduced) = reduce_cell(samples, palette);
            approximate |= reduced;
            let glyph = glyphs.glyph(mask);
            let cell = &mut buffer[(target.x + cx, target.y + cy)];
            write_cell(cell, glyph, color(fg), color(bg));
        }
    }
    approximate
}

/// Reduces a cell to two colors. The most frequent sample becomes the background and
/// the next ranked distinct one the foreground, with count ties going to the lower
/// palette index. Other samples map to the nearer of the two, ties to the background.
/// Returns the foreground mask, foreground, background, and whether any sample
/// changed its RGB color.
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

/// Averages the dots under each half cell, see the Original column on [`Zoom`].
/// Returns two color rows per cell row, in row major order.
fn average_rgb(pixels: &RgbaImage, geometry: &PreviewGeometry) -> Vec<Color> {
    let (cw, ch) = (geometry.target.width, geometry.target.height);
    let (w, h) = geometry.visible;
    let (ox, oy) = geometry.origin;
    let (dx, dy) = if let Zoom::Out(n) = geometry.zoom {
        (f64::from(n), f64::from(n) * CELL_ASPECT / 2.0)
    } else {
        (
            f64::from(w) / f64::from(cw),
            f64::from(h) / (f64::from(ch) * 2.0),
        )
    };
    let average = |cx: u16, half_row: u32| {
        let x0 = ((f64::from(cx) * dx) as u64).min(u64::from(w - 1));
        let x1 = (((f64::from(cx) + 1.0) * dx) as u64)
            .max(x0 + 1)
            .min(u64::from(w));
        let y0 = ((f64::from(half_row) * dy) as u64).min(u64::from(h - 1));
        let y1 = (((f64::from(half_row) + 1.0) * dy) as u64)
            .max(y0 + 1)
            .min(u64::from(h));
        let mut sum = [0u64; 3];
        for y in y0..y1 {
            for x in x0..x1 {
                let [r, g, b, _] = pixels.get_pixel(ox + x as u32, oy + y as u32).0;
                sum[0] += u64::from(r);
                sum[1] += u64::from(g);
                sum[2] += u64::from(b);
            }
        }
        let rgb = sum.map(|total| (total / ((x1 - x0) * (y1 - y0))) as u8);
        Color::from(rgb)
    };
    (0..u32::from(ch) * 2)
        .flat_map(|row| (0..cw).map(move |col| average(col, row)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewport::{fit_aspect, geometry};
    use image::Rgba;
    use ratatui::layout::Constraint;
    use ratatui::{buffer::Buffer, layout::Rect, style::Color};
    use rthid_dither::dither::make_palette;

    fn render_with_new_caches(
        pixels: Pixels<'_>,
        glyphs: Glyphs,
        geometry: &PreviewGeometry,
        buffer: &mut Buffer,
    ) -> bool {
        render(
            pixels,
            glyphs,
            geometry,
            buffer,
            &mut OverviewCaches::default(),
        )
    }

    #[test]
    fn solid_cells_reset_foreground_for_every_glyph() {
        for glyph in [' ', '▀', '▌', '█', '\u{1fb00}'] {
            for fg in [Color::Red, Color::Blue] {
                let mut cell = Cell::default();
                write_cell(&mut cell, glyph, fg, Color::Blue);
                if glyph == ' ' || fg == Color::Blue {
                    assert_eq!(cell.symbol(), " ");
                    assert_eq!(cell.fg, Color::Reset);
                } else {
                    assert_eq!(cell.symbol(), glyph.to_string());
                    assert_eq!(cell.fg, fg);
                }
                assert_eq!(cell.bg, Color::Blue);
            }
        }
    }

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
        let area = Rect::new(2, 3, 60, 30);
        let mut buffer = Buffer::empty(area);
        viewport.zoom = Zoom::Fit;
        render_with_new_caches(
            indexed(&indices, &palette),
            Glyphs::Quadrant,
            &crate::viewport::geometry(&viewport, (w, h), area),
            &mut buffer,
        );
        let is_block = |cell: &ratatui::buffer::Cell| {
            cell.symbol()
                .chars()
                .all(|c| c == ' ' || ('\u{2580}'..='\u{259f}').contains(&c))
        };
        assert!(buffer.content.iter().all(is_block));
        assert!(buffer.content.iter().any(|cell| cell.symbol() != " "));
        viewport.zoom = Zoom::In(1);
        let geometry = geometry(&viewport, (w, h), area);
        let Minimap { map, .. } = geometry.minimap.unwrap();
        render_minimap(
            indexed(&indices, &palette),
            Glyphs::Quadrant,
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
            zoom: Zoom::In(1),
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
        render_minimap(
            indexed(&indices, &palette),
            Glyphs::default(),
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
            render_with_new_caches(
                indexed(&indices, &palette),
                Glyphs::default(),
                &crate::viewport::geometry(&viewport, (w, h), area),
                &mut current,
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
                        Color::Reset => Color::Reset,
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
            let mut buffer = Buffer::empty(area);
            assert_eq!(
                render_with_new_caches(
                    indexed(&indices, &palette),
                    glyphs,
                    &geometry(&Viewport::default(), size, area),
                    &mut buffer,
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
                Glyphs::default(),
                &crate::viewport::geometry(&Viewport::default(), (1, 2), area),
                &mut buffer,
                &mut cache,
            );
            assert_eq!(cache.full.version(), expected);
        }
        let area = Rect::new(0, 0, 2, 2);
        render(
            Pixels::Rgb(&pixels, 2),
            Glyphs::default(),
            &crate::viewport::geometry(&Viewport::default(), (1, 2), area),
            &mut Buffer::empty(area),
            &mut cache,
        );
        assert_eq!(cache.full.version(), 3);
        let area = Rect::new(0, 0, 1, 1);
        let updated = rgb(1, 2, &[[0; 3], [255; 3]]);
        render(
            Pixels::Rgb(&updated, 3),
            Glyphs::default(),
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
    fn exact_painting_preserves_partial_edge_dots() {
        let palette = make_palette(&["#000", "#f00"]).unwrap();
        let indices: Vec<_> = (0..256)
            .map(|i| u8::from(i % 16 == 15 || i / 16 == 15))
            .collect();
        let pixels = rgb(
            16,
            16,
            &indices
                .iter()
                .map(|&i| palette[usize::from(i)].rgb)
                .collect::<Vec<_>>(),
        );
        for (zoom, area, partial, full) in [
            // Each area clips the sixteenth dot, so `partial` lies in the clipped dot and `full` in the last dot after panning.
            (Zoom::In(3), Rect::new(0, 0, 47, 30), (46, 0), (44, 0)),
            (Zoom::In(3), Rect::new(0, 0, 48, 23), (0, 22), (0, 21)),
            (Zoom::In(4), Rect::new(0, 0, 64, 31), (0, 30), (0, 28)),
        ] {
            let mut viewport = Viewport {
                zoom,
                hide_minimap: true,
                ..Viewport::default()
            };
            let mut buffer = Buffer::empty(area);
            let before = geometry(&viewport, (16, 16), area);
            viewport.pan(i64::MAX, i64::MAX, &before);
            let after = geometry(&viewport, (16, 16), area);
            for source in [indexed(&indices, &palette), Pixels::Rgb(&pixels, 1)] {
                render_with_new_caches(source, Glyphs::default(), &before, &mut buffer);
                assert_eq!(buffer[partial].bg, Color::Rgb(255, 0, 0));
                render_with_new_caches(source, Glyphs::default(), &after, &mut buffer);
                assert_eq!(buffer[full].symbol(), " ");
                assert_eq!(buffer[full].fg, Color::Reset);
                assert_eq!(buffer[full].bg, Color::Rgb(255, 0, 0));
            }
        }
    }

    #[test]
    fn rgb_exact_preserves_each_pixel() {
        let pixels = RgbaImage::from_raw(1, 2, vec![25, 26, 128, 0, 231, 47, 93, 128]).unwrap();
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        assert!(!render_with_new_caches(
            Pixels::Rgb(&pixels, 1),
            Glyphs::default(),
            &crate::viewport::geometry(
                &Viewport {
                    zoom: Zoom::In(1),
                    ..Viewport::default()
                },
                (1, 2),
                area
            ),
            &mut buffer,
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
            assert!(!render_with_new_caches(
                Pixels::Rgb(&rgb(4, 4, &pixels), 1),
                Glyphs::default(),
                &crate::viewport::geometry(&Viewport::default(), (4, 4), area),
                &mut buffer,
            ));
            let (cw, ch) = fit_aspect(4, 4, area, CELL_ASPECT);
            let target = area.centered(Constraint::Length(cw), Constraint::Length(ch));
            assert_eq!((cw, ch), (width, 1));
            for (x, value) in expected.into_iter().enumerate() {
                let cell = &buffer[(target.x + x as u16, target.y)];
                assert_eq!(cell.symbol(), " ");
                assert_eq!(cell.fg, Color::Reset);
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
        assert!(!render_with_new_caches(
            Pixels::Rgb(&rgb(1, 4, &pixels), 1),
            Glyphs::default(),
            &crate::viewport::geometry(&Viewport::default(), (1, 4), area),
            &mut buffer,
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
        assert!(!render_with_new_caches(
            Pixels::Rgb(&rgb(1, 2, &pixels), 1),
            Glyphs::default(),
            &crate::viewport::geometry(&Viewport::default(), (1, 2), area),
            &mut buffer,
        ));
        let (cw, ch) = fit_aspect(1, 2, area, CELL_ASPECT);
        let target = area.centered(Constraint::Length(cw), Constraint::Length(ch));
        assert_eq!((cw, ch), (3, 2));
        for y in 0..ch {
            for x in 0..cw {
                let cell = &buffer[(target.x + x, target.y + y)];
                let [r, g, b] = pixels[y as usize];
                assert_eq!(cell.symbol(), " ");
                assert_eq!(cell.fg, Color::Reset);
                assert_eq!(cell.bg, Color::Rgb(r, g, b));
            }
        }
    }

    #[test]
    fn exact_view_keeps_each_dot_and_odd_bottom_row() {
        let palette = make_palette(&["#000", "#333", "#fff"]).unwrap();
        let area = Rect::new(0, 0, 1, 2);
        let mut buffer = Buffer::empty(area);
        assert!(!render_with_new_caches(
            indexed(&[0, 1, 2], &palette),
            Glyphs::default(),
            &crate::viewport::geometry(
                &Viewport {
                    zoom: Zoom::In(1),
                    ..Viewport::default()
                },
                (1, 3),
                area
            ),
            &mut buffer,
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
            zoom: Zoom::In(1),
            ..Viewport::default()
        };
        render_with_new_caches(
            indexed(&vec![0; 120 * 120], &palette),
            Glyphs::default(),
            &crate::viewport::geometry(&viewport, (120, 120), area),
            &mut buffer,
        );
        // The square grid fits 9x4 cells inside the 14x4 border of a 16x6 frame at x 32.
        assert_eq!(buffer[(32, 0)].symbol(), "┌");
        assert_eq!(buffer[(47, 5)].symbol(), "┘");
        assert_eq!(buffer[(33, 2)].symbol(), " ");
        assert_eq!(buffer[(33, 2)].bg, Color::Reset);
        let (_, map) = viewport.minimap(120, 120, area).unwrap();
        assert_eq!(buffer[(map.x, map.y)].fg, Color::Reset);
    }
    #[test]
    fn rgb_overview_and_minimap_keep_separate_memos() {
        let pixels = RgbaImage::new(120, 120);
        let area = Rect::new(0, 0, 69, 36);
        let mut caches = OverviewCaches::default();
        for version in [1, 2] {
            for exact in [false, true, false, true] {
                let viewport = Viewport {
                    zoom: if exact { Zoom::In(1) } else { Zoom::Fit },
                    ..Viewport::default()
                };
                render(
                    Pixels::Rgb(&pixels, version),
                    Glyphs::default(),
                    &geometry(&viewport, pixels.dimensions(), area),
                    &mut Buffer::empty(area),
                    &mut caches,
                );
            }
            assert_eq!(caches.full.version(), version);
            assert_eq!(caches.inset.version(), version);
        }
    }
    #[test]
    fn enlarged_solid_cells_use_spaces_and_distinct_halves_keep_half_blocks() {
        let area = Rect::new(2, 3, 2, 2);
        let palette = make_palette(&["#f00", "#00f"]).unwrap();
        let pixels = rgb(1, 2, &[palette[0].rgb, palette[1].rgb]);
        for zoom in [Zoom::In(2), Zoom::In(1)] {
            let viewport = Viewport {
                zoom,
                hide_minimap: true,
                ..Viewport::default()
            };
            let g = geometry(&viewport, (1, 2), area);
            for source in [indexed(&[0, 1], &palette), Pixels::Rgb(&pixels, 1)] {
                let mut buffer = Buffer::empty(area);
                render_with_new_caches(source, Glyphs::Quadrant, &g, &mut buffer);
                for pos in g.target.positions() {
                    let cell = &buffer[pos];
                    if zoom == Zoom::In(2) {
                        assert_eq!(cell.symbol(), " ");
                        assert_eq!(cell.fg, Color::Reset);
                    } else {
                        assert_eq!(cell.symbol(), "▀");
                        assert_eq!(cell.fg, Color::from(palette[0].rgb));
                        assert_eq!(cell.bg, Color::from(palette[1].rgb));
                    }
                }
            }
        }
    }

    #[test]
    fn in_two_draws_each_dot_as_two_columns_and_two_halves() {
        let area = Rect::new(2, 3, 4, 2);
        let pixels = rgb(2, 2, &[[255, 0, 0], [0, 255, 0], [0, 0, 255], [255; 3]]);
        let palette = make_palette(&["#f00", "#0f0", "#00f", "#fff"]).unwrap();
        let viewport = Viewport {
            zoom: Zoom::In(2),
            ..Viewport::default()
        };
        let indices: Vec<_> = pixels
            .pixels()
            .map(|p| palette.iter().position(|c| c.rgb == p.0[..3]).unwrap() as u8)
            .collect();
        for pixels in [Pixels::Rgb(&pixels, 1), indexed(&indices, &palette)] {
            let mut buffer = Buffer::empty(area);
            assert!(!render_with_new_caches(
                pixels,
                Glyphs::default(),
                &geometry(&viewport, (2, 2), area),
                &mut buffer,
            ));
            for (x, y, rgb) in [
                (2, 3, [255, 0, 0]),
                (3, 3, [255, 0, 0]),
                (4, 3, [0, 255, 0]),
                (5, 3, [0, 255, 0]),
                (2, 4, [0, 0, 255]),
                (3, 4, [0, 0, 255]),
                (4, 4, [255; 3]),
                (5, 4, [255; 3]),
            ] {
                let cell = &buffer[(x, y)];
                assert_eq!(cell.symbol(), " ");
                assert_eq!(cell.fg, Color::Reset);
                assert_eq!(cell.bg, Color::from(rgb));
            }
        }
    }

    #[test]
    fn out_two_samples_the_visible_region_and_rgb_cache_tracks_pan() {
        let area = Rect::new(2, 3, 2, 2);
        let pixels = RgbaImage::from_fn(12, 20, |x, _| Rgba([if x < 4 { 0 } else { 255 }; 4]));
        let palette = make_palette(&["#000", "#fff"]).unwrap();
        let indices: Vec<_> = (0..12 * 20).map(|i| u8::from(i % 12 >= 4)).collect();
        let mut viewport = Viewport {
            zoom: Zoom::Out(2),
            ..Viewport::default()
        };
        let mut caches = OverviewCaches::default();
        for (origin, expected) in [(0, 0), (4, 255), (0, 0)] {
            viewport.x = origin;
            viewport.y = 5;
            let g = geometry(&viewport, (12, 20), area);
            assert_eq!(g.visible, (4, 9));
            for pixels in [Pixels::Rgb(&pixels, 1), indexed(&indices, &palette)] {
                let mut buffer = Buffer::empty(area);
                assert!(!render(
                    pixels,
                    Glyphs::default(),
                    &g,
                    &mut buffer,
                    &mut caches,
                ));
                for pos in area.positions() {
                    assert_eq!(buffer[pos].fg, Color::Reset);
                    assert_eq!(buffer[pos].bg, Color::Rgb(expected, expected, expected));
                }
            }
        }
        assert_eq!(caches.full.version(), 3);
    }
    #[test]
    fn out_two_subcell_centers_keep_the_cell_aspect_with_fractional_height() {
        let area = Rect::new(0, 0, 2, 3);
        let palette = make_palette(&["#000", "#fff"]).unwrap();
        let indices: Vec<_> = (0..20 * 20).map(|i| u8::from(i / 20 >= 12)).collect();
        let viewport = Viewport {
            zoom: Zoom::Out(2),
            ..Viewport::default()
        };
        let g = geometry(&viewport, (20, 20), area);
        assert_eq!(g.visible, (4, 13));
        let mut buffer = Buffer::empty(area);
        assert!(!render_with_new_caches(
            indexed(&indices, &palette),
            Glyphs::Quadrant,
            &g,
            &mut buffer,
        ));
        for x in 0..2 {
            let cell = &buffer[(x, 2)];
            assert_eq!(cell.symbol(), "▄");
            assert_eq!(cell.fg, Color::Rgb(255, 255, 255));
            assert_eq!(cell.bg, Color::Rgb(0, 0, 0));
        }
    }
}
