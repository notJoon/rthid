//! Preview coordinates and cell rendering.
//!
//! Paints the Overview and the 1:1 view into a ratatui buffer with half blocks or
//! Sextant and Quadrant glyphs, plus the minimap shown over the 1:1 view. A dot is
//! one grid pixel. The 1:1 view (`exact` in code) draws two dots per cell, one above
//! the other. The minimap is also called the inset.

use crate::memo::Memo;
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

// Typical terminal cell height divided by its width, regardless of glyph set.
const CELL_ASPECT: f64 = 2.25;

/// Largest (cols, rows) inside `area` that keeps the grid shape when a cell is
/// `aspect` times taller than wide. Each is at least 1 unless `area` is empty.
pub(crate) fn fit_aspect(w: u32, h: u32, area: Rect, aspect: f64) -> (u16, u16) {
    if area.is_empty() {
        return (0, 0);
    }
    let scale =
        (f64::from(area.width) / f64::from(w)).min(f64::from(area.height) * aspect / f64::from(h));
    (
        (f64::from(w) * scale).floor().max(1.0) as u16,
        (f64::from(h) * scale / aspect).floor().max(1.0) as u16,
    )
}

/// User controlled preview state: glyph set, Overview or 1:1, minimap, and 1:1 origin.
#[derive(Default, Clone, Copy, Debug)]
pub struct View {
    /// Glyph set for the Dithered Overview and minimap.
    pub glyphs: Glyphs,
    /// 1:1 view, one dot per half cell, instead of the Overview.
    pub exact: bool,
    /// Hides the minimap in the 1:1 view.
    pub hide_minimap: bool,
    /// Grid column at the left edge in 1:1. May exceed the grid until the next clamp.
    pub x: u32,
    /// Grid row at the top edge in 1:1. May exceed the grid until the next clamp.
    pub y: u32,
}

// TODO: Use Overview { target }, Exact { visible, origin, minimap } and Empty variants
// with the PreviewReport enum when the slider lands.
/// Preview coordinates shared by input handling and rendering.
///
/// Rebuilt from the View for each frame and input event. The origin is clamped here
/// without changing the View.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewGeometry {
    /// Grid size in dots, after scaling and rotation.
    pub grid: (u32, u32),
    /// Terminal cells available for the preview.
    pub area: Rect,
    /// Dots that fit in the preview at 1:1, two per cell row, capped by the grid size.
    /// Computed in every mode but only used by the 1:1 view.
    pub visible: (u32, u32),
    /// Clamped View origin, the grid dot at the top left of the 1:1 view.
    pub origin: (u32, u32),
    /// Present only in 1:1 when the minimap is shown and fits.
    pub minimap: Option<Minimap>,
}

/// Minimap placement in the upper right corner of the 1:1 view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Minimap {
    /// Bordered inset, including the border.
    pub frame: Rect,
    /// Cells of the fitted overview, centered inside the border.
    pub map: Rect,
    /// Map cells covering the region visible at 1:1. They stay undimmed.
    pub viewport: Rect,
}

/// Builds the PreviewGeometry for `view` over a grid of `grid` dots in `area`.
pub fn geometry(view: &View, grid: (u32, u32), area: Rect) -> PreviewGeometry {
    let (w, h) = grid;
    let mut view = *view;
    view.clamp(w, h, area);
    let visible = view.visible(w, h, area);
    let minimap = view.minimap(w, h, area).map(|(frame, map)| Minimap {
        frame,
        map,
        viewport: view.viewport_rect(w, h, visible, map),
    });
    PreviewGeometry {
        grid,
        area,
        visible,
        origin: (view.x, view.y),
        minimap,
    }
}

impl View {
    /// Minimap frame and map cells, in that order.
    ///
    /// None in the Overview, when the minimap is hidden, when the grid is empty, or
    /// when the area inside the border is smaller than 3x3 cells.
    pub fn minimap(&self, w: u32, h: u32, area: Rect) -> Option<(Rect, Rect)> {
        // A bordered inset in the upper right corner contains the fitted overview.
        let fw = (area.width / 3).min(18);
        let frame = Rect::new(area.right() - fw, area.y, fw, (area.height / 3).min(10));
        let inner = Block::bordered().inner(frame);
        if w == 0
            || h == 0
            || !self.exact
            || self.hide_minimap
            || inner.width < 3
            || inner.height < 3
        {
            return None;
        }
        let (mw, mh) = fit_aspect(w, h, inner, CELL_ASPECT);
        Some((
            frame,
            inner.centered(Constraint::Length(mw), Constraint::Length(mh)),
        ))
    }

    fn visible(&self, w: u32, h: u32, area: Rect) -> (u32, u32) {
        (
            w.min(u32::from(area.width)),
            h.min(u32::from(area.height) * 2),
        )
    }

    /// Moves the origin so the 1:1 window stays inside the grid.
    pub fn clamp(&mut self, w: u32, h: u32, area: Rect) {
        let (vw, vh) = self.visible(w, h, area);
        self.x = self.x.min(w.saturating_sub(vw));
        self.y = self.y.min(h.saturating_sub(vh));
    }

    /// Shifts the origin by whole dots and keeps the 1:1 window inside the grid.
    pub fn pan(&mut self, dx: i64, dy: i64, geometry: &PreviewGeometry) {
        self.x = i64::from(self.x)
            .saturating_add(dx)
            .clamp(0, i64::from(u32::MAX)) as u32;
        self.y = i64::from(self.y)
            .saturating_add(dy)
            .clamp(0, i64::from(u32::MAX)) as u32;
        self.clamp(geometry.grid.0, geometry.grid.1, geometry.area);
    }

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

    /// Centers the 1:1 window on the dots under a terminal cell inside the map.
    /// Does nothing without a minimap.
    pub fn center_on_minimap(&mut self, point: (u16, u16), geometry: &PreviewGeometry) {
        let Some(Minimap { map, .. }) = geometry.minimap else {
            return;
        };
        let (w, h) = geometry.grid;
        let (vw, vh) = geometry.visible;
        let center = |cell: u16, cells: u16, dots: u32| {
            ((u64::from(cell) * 2 + 1) * u64::from(dots) / (u64::from(cells) * 2)) as i64
        };
        self.pan(
            center(point.0 - map.x, map.width, w) - i64::from(vw / 2) - i64::from(self.x),
            center(point.1 - map.y, map.height, h) - i64::from(vh / 2) - i64::from(self.y),
            geometry,
        );
    }

    // Map cells covering the visible dots, at least one cell on each axis.
    fn viewport_rect(&self, w: u32, h: u32, visible: (u32, u32), map: Rect) -> Rect {
        let project = |start: u32, length: u32, total: u32, cells: u16| {
            let first = (u64::from(start) * u64::from(cells) / u64::from(total)) as u16;
            let end =
                (u64::from(start + length) * u64::from(cells)).div_ceil(u64::from(total)) as u16;
            (first.min(cells - 1), end.min(cells).max(first + 1) - first)
        };
        let (x, width) = project(self.x, visible.0, w, map.width);
        let (y, height) = project(self.y, visible.1, h, map.height);
        Rect::new(map.x + x, map.y + y, width, height)
    }
}

/// Paints the preview and reports whether the Dithered Overview approximated colors.
///
/// The 1:1 view draws one half block per two dots plus the minimap and never reports
/// approximation.
pub fn render(
    pixels: Pixels<'_>,
    view: &View,
    geometry: &PreviewGeometry,
    buffer: &mut Buffer,
    caches: &mut OverviewCaches,
) -> bool {
    let area = geometry.area;
    if area.is_empty() {
        return false;
    }
    if view.exact {
        render_exact(pixels, view, geometry, buffer, &mut caches.inset);
        false
    } else {
        render_overview(
            pixels,
            geometry.grid,
            view.glyphs,
            area,
            buffer,
            &mut caches.full,
        )
    }
}

fn render_exact(
    pixels: Pixels<'_>,
    view: &View,
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
    draw_minimap(pixels, view, geometry, buffer, cache);
}

// Draws the minimap overview and its gray border, then dims map cells outside the
// viewport to half brightness.
fn draw_minimap(
    pixels: Pixels<'_>,
    view: &View,
    geometry: &PreviewGeometry,
    buffer: &mut Buffer,
    cache: &mut OverviewCache,
) {
    let Some(Minimap {
        frame,
        map,
        viewport,
    }) = geometry.minimap
    else {
        return;
    };
    view.render_minimap(pixels, geometry, buffer, cache);
    Block::bordered()
        .border_style(Color::Gray)
        .render(frame, buffer);
    for pos in map.positions().filter(|&pos| !viewport.contains(pos)) {
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

    #[test]
    fn geometry_preserves_visible_size_and_minimap_coordinates() {
        let area = Rect::new(2, 3, 48, 18);
        for (exact, hidden, grid, area, origin, visible, expected_minimap) in [
            (false, false, (120, 120), area, (72, 84), (48, 36), None),
            (true, true, (120, 120), area, (72, 84), (48, 36), None),
            (
                true,
                false,
                (120, 120),
                area,
                (72, 84),
                (48, 36),
                Some(Minimap {
                    frame: Rect::new(34, 3, 16, 6),
                    map: Rect::new(38, 4, 9, 4),
                    viewport: Rect::new(43, 6, 4, 2),
                }),
            ),
            (
                true,
                false,
                (120, 120),
                Rect::new(2, 3, 10, 5),
                (110, 110),
                (10, 10),
                None,
            ),
            (
                true,
                false,
                (120, 120),
                Rect::default(),
                (120, 120),
                (0, 0),
                None,
            ),
            (true, false, (0, 0), area, (0, 0), (0, 0), None),
            (
                true,
                false,
                (2, 3),
                area,
                (0, 0),
                (2, 3),
                Some(Minimap {
                    frame: Rect::new(34, 3, 16, 6),
                    map: Rect::new(39, 4, 6, 4),
                    viewport: Rect::new(39, 4, 6, 4),
                }),
            ),
        ] {
            let view = View {
                exact,
                hide_minimap: hidden,
                x: 999,
                y: 999,
                ..View::default()
            };
            assert_eq!(
                geometry(&view, grid, area),
                PreviewGeometry {
                    grid,
                    area,
                    visible,
                    origin,
                    minimap: expected_minimap,
                },
            );
            assert_eq!((view.x, view.y), (999, 999));
        }
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
    fn indexed_modes_draw_their_glyphs() {
        let palette = make_palette(&["#000", "#fff"]).unwrap();
        let mut indices = vec![0; 4 * 6];
        indices[0] = 1;
        let area = Rect::new(1, 1, 69, 20);
        for (glyphs, exact) in [
            (Glyphs::Sextant, false),
            (Glyphs::Quadrant, false),
            (Glyphs::Quadrant, true),
        ] {
            let view = View {
                glyphs,
                exact,
                ..View::default()
            };
            let mut buffer = Buffer::empty(area);
            render(
                indexed(&indices, &palette),
                &view,
                &geometry(&view, (4, 6), area),
                &mut buffer,
                &mut OverviewCaches::default(),
            );
            let has_sextant = buffer.content.iter().any(|cell| {
                cell.symbol()
                    .chars()
                    .any(|c| ('\u{1fb00}'..='\u{1fb3b}').contains(&c))
            });
            assert_eq!(has_sextant, glyphs == Glyphs::Sextant && !exact);
            if exact {
                assert!(buffer.content.iter().any(|cell| cell.symbol() == "▀"));
                assert_eq!(buffer[(1, 1)].fg, Color::Rgb(255, 255, 255));
                assert_eq!(buffer[(1, 1)].bg, Color::Rgb(0, 0, 0));
            }
        }
    }

    #[test]
    fn quadrant_masks_use_row_major_bits() {
        let expected = [
            ' ', '▘', '▝', '▀', '▖', '▌', '▞', '▛', '▗', '▚', '▐', '▜', '▄', '▙', '▟', '█',
        ];
        for (mask, glyph) in expected.into_iter().enumerate() {
            assert_eq!(Glyphs::Quadrant.glyph(mask as u32), glyph, "mask {mask}");
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
        let (w, h, palette, indices, mut view) = minimap_fixture();
        view.glyphs = Glyphs::Quadrant;
        let area = Rect::new(2, 3, 60, 30);
        let mut buffer = Buffer::empty(area);
        view.exact = false;
        render(
            indexed(&indices, &palette),
            &view,
            &crate::view::geometry(&view, (w, h), area),
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
        view.exact = true;
        let geometry = geometry(&view, (w, h), area);
        let Minimap { map, .. } = geometry.minimap.unwrap();
        view.render_minimap(
            indexed(&indices, &palette),
            &geometry,
            &mut buffer,
            &mut Memo::default(),
        );
        assert!(map.positions().all(|pos| is_block(&buffer[pos])));
        assert!(map.positions().any(|pos| buffer[pos].symbol() != " "));
    }

    fn minimap_fixture() -> (u32, u32, Vec<PaletteColor>, Vec<u8>, View) {
        let (w, h) = (180, 180);
        let palette = make_palette(&["#123", "#abc", "#e64", "#fff"]).unwrap();
        let indices = (0..w * h)
            .map(|i| ((i * 17 + i / w * 13 + i / 7) % 4) as u8)
            .collect();
        let view = View {
            exact: true,
            ..View::default()
        };
        (w, h, palette, indices, view)
    }

    #[test]
    fn minimap_pan_preserves_symbols_and_dims_only_outside_viewport() {
        let area = Rect::new(2, 3, 60, 30);
        let (w, h, palette, indices, mut view) = minimap_fixture();
        let mut baseline = Buffer::empty(area);
        let geometry = geometry(&view, (w, h), area);
        let Minimap { frame, map, .. } = geometry.minimap.unwrap();
        view.render_minimap(
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
            view.x = x;
            view.y = y;
            let viewport = view.viewport_rect(w, h, view.visible(w, h, area), map);
            render(
                indexed(&indices, &palette),
                &view,
                &crate::view::geometry(&view, (w, h), area),
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
                if map.contains(pos) && !viewport.contains(pos) {
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
    fn minimap_pan_restores_cells_and_emits_colors() {
        use ratatui::{
            Terminal, TerminalOptions, Viewport,
            backend::{Backend, CrosstermBackend, TestBackend},
        };
        ratatui::crossterm::style::force_color_output(true);
        let area = Rect::new(0, 0, 60, 30);
        let (w, h, palette, indices, mut view) = minimap_fixture();
        let mut baseline = Buffer::empty(area);
        let geometry = geometry(&view, (w, h), area);
        let Minimap { frame, map, .. } = geometry.minimap.unwrap();
        view.render_minimap(
            indexed(&indices, &palette),
            &geometry,
            &mut baseline,
            &mut Memo::default(),
        );
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
        let mut previous: Option<(Buffer, Rect)> = None;
        let mut bytes = Vec::<u8>::new();
        let mut ansi_terminal = Terminal::with_options(
            CrosstermBackend::new(&mut bytes),
            TerminalOptions {
                viewport: Viewport::Fixed(area),
            },
        )
        .unwrap();
        for (x, y) in [(0, 0), (60, 60), (1, 1), (120, 120), (0, 0), (1, 0), (1, 1)] {
            view.x = x;
            view.y = y;
            let viewport = view.viewport_rect(w, h, view.visible(w, h, area), map);
            terminal
                .draw(|f| {
                    render(
                        indexed(&indices, &palette),
                        &view,
                        &crate::view::geometry(&view, (w, h), area),
                        f.buffer_mut(),
                        &mut OverviewCaches::default(),
                    );
                })
                .unwrap();
            let current = terminal.backend().buffer().clone();
            let completed = ansi_terminal
                .draw(|f| {
                    render(
                        indexed(&indices, &palette),
                        &view,
                        &crate::view::geometry(&view, (w, h), area),
                        f.buffer_mut(),
                        &mut OverviewCaches::default(),
                    );
                })
                .unwrap();
            assert_eq!(completed.buffer, &current);
            if let Some((prev, old)) = &previous {
                let mut restored = 0;
                for pos in frame.positions() {
                    if map.contains(pos) && viewport.contains(pos) && !old.contains(pos) {
                        restored += 1;
                    }
                }
                let emitted = prev.diff(&current);
                let expected: Vec<_> = frame
                    .positions()
                    .filter(|&p| current[p] != prev[p])
                    .collect();
                let actual: Vec<_> = emitted
                    .iter()
                    .filter(|(x, y, _)| frame.contains((*x, *y).into()))
                    .map(|(x, y, _)| ratatui::layout::Position::new(*x, *y))
                    .collect();
                assert_eq!(
                    actual, expected,
                    "minimap diff must emit every changed cell exactly once"
                );
                let mut map_bytes = Vec::<u8>::new();
                let mut map_backend = CrosstermBackend::new(&mut map_bytes);
                map_backend
                    .draw(
                        prev.diff(&current)
                            .into_iter()
                            .filter(|(x, y, _)| frame.contains((*x, *y).into())),
                    )
                    .unwrap();
                assert!(map_bytes.ends_with(b"\x1b[39m\x1b[49m\x1b[59m\x1b[0m"));
                if restored > 0 {
                    assert!(map_bytes.windows(6).any(|s| s == b"\x1b[38;2"));
                    assert!(
                        map_bytes.windows(6).any(|s| s == b";48;2;"),
                        "RGB backgrounds must be explicit"
                    );
                }
            }
            previous = Some((current, viewport));
        }
        drop(ansi_terminal);
        let output = String::from_utf8(bytes).unwrap();
        let second_frame = output.split("\x1b[0m").nth(1).unwrap();
        assert!(second_frame.contains("\x1b[38;2;255;255;255;48;2;238;102;68m🬣"));
        assert!(second_frame.contains("\x1b[38;2;85;93;102;48;2;119;51;34m🬱"));
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
    fn glyph_set_does_not_change_fitted_shape() {
        let map = |glyphs| {
            let view = View {
                glyphs,
                exact: true,
                ..View::default()
            };
            view.minimap(120, 120, Rect::new(0, 0, 48, 18)).unwrap().1
        };
        assert_eq!(map(Glyphs::Quadrant), map(Glyphs::Sextant));
        assert_eq!(
            fit_aspect(120, 120, Rect::new(0, 0, 0, 5), CELL_ASPECT),
            (0, 0)
        );
        assert_eq!(
            fit_aspect(120, 120, Rect::new(0, 0, 5, 0), CELL_ASPECT),
            (0, 0)
        );
    }

    #[test]
    fn overview_fit_corrects_subcell_aspect() {
        assert_eq!(
            fit_aspect(120, 120, Rect::new(0, 0, 60, 40), CELL_ASPECT),
            (60, 26)
        );
        assert_eq!(
            fit_aspect(120, 120, Rect::new(0, 0, 20, 5), CELL_ASPECT),
            (11, 5)
        );
        assert_eq!(
            fit_aspect(1, 1000, Rect::new(0, 0, 1, 1), CELL_ASPECT),
            (1, 1)
        );
    }
    #[test]
    fn clamps_pan_and_projects_minimap_viewport() {
        let mut view = View {
            exact: true,
            hide_minimap: false,
            x: 99,
            y: 99,
            ..View::default()
        };
        view.clamp(100, 80, Rect::new(0, 0, 20, 10));
        assert_eq!((view.x, view.y), (80, 60));
        assert_eq!(view.visible(100, 80, Rect::new(0, 0, 20, 10)), (20, 20));
        assert_eq!(
            view.viewport_rect(100, 80, (20, 20), Rect::new(3, 4, 10, 8)),
            Rect::new(11, 10, 2, 2)
        );
        assert_eq!(
            view.viewport_rect(10_000, 10_000, (1, 1), Rect::new(3, 4, 10, 8)),
            Rect::new(3, 4, 1, 1)
        );
        view.clamp(2, 3, Rect::new(0, 0, 20, 10));
        assert_eq!((view.x, view.y), (0, 0));
    }

    #[test]
    fn render_samples_centers_and_reduces_only_extra_colors() {
        let palette = make_palette(&["#000", "#333", "#fff"]).unwrap();
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        // Sample columns 1 and 3 of each row. Column 0 must not participate.
        assert!(!render(
            indexed(&[2, 0, 2, 1, 2, 0, 2, 1, 2, 0, 2, 1], &palette),
            &View::default(),
            &crate::view::geometry(&View::default(), (4, 3), area),
            &mut buffer,
            &mut OverviewCaches::default()
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▐");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(51, 51, 51));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(0, 0, 0));
        assert!(render(
            indexed(&[0, 2, 0, 2, 1, 2], &palette),
            &View::default(),
            &crate::view::geometry(&View::default(), (2, 3), area),
            &mut buffer,
            &mut OverviewCaches::default()
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▌");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(0, 0, 0));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(255, 255, 255));
    }

    #[test]
    fn quadrant_samples_centers_and_reduces_only_extra_colors() {
        let palette = make_palette(&["#000", "#333", "#fff"]).unwrap();
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        let view = View {
            glyphs: Glyphs::Quadrant,
            ..View::default()
        };
        assert!(!render(
            indexed(&[2, 2, 2, 2, 2, 0, 2, 1, 2, 2, 2, 2, 2, 0, 2, 1], &palette),
            &view,
            &crate::view::geometry(&view, (4, 4), area),
            &mut buffer,
            &mut OverviewCaches::default()
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▐");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(51, 51, 51));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(0, 0, 0));
        assert!(render(
            indexed(&[0, 2, 1, 2], &palette),
            &view,
            &crate::view::geometry(&view, (2, 2), area),
            &mut buffer,
            &mut OverviewCaches::default()
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▌");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(0, 0, 0));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(255, 255, 255));
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
                &View::default(),
                &crate::view::geometry(&View::default(), (1, 2), area),
                &mut buffer,
                &mut cache,
            );
            assert_eq!(cache.full.version(), expected);
        }
        let area = Rect::new(0, 0, 2, 2);
        render(
            Pixels::Rgb(&pixels, 2),
            &View::default(),
            &crate::view::geometry(&View::default(), (1, 2), area),
            &mut Buffer::empty(area),
            &mut cache,
        );
        assert_eq!(cache.full.version(), 3);
        let area = Rect::new(0, 0, 1, 1);
        let updated = rgb(1, 2, &[[0; 3], [255; 3]]);
        render(
            Pixels::Rgb(&updated, 3),
            &View::default(),
            &crate::view::geometry(&View::default(), (1, 2), area),
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
            &View {
                exact: true,
                ..View::default()
            },
            &crate::view::geometry(
                &View {
                    exact: true,
                    ..View::default()
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
        for glyphs in [Glyphs::Sextant, Glyphs::Quadrant] {
            for (width, expected) in [(2, vec![0, 255]), (1, vec![127])] {
                let area = Rect::new(2, 3, width, 3);
                let mut buffer = Buffer::empty(area);
                assert!(!render(
                    Pixels::Rgb(&rgb(4, 4, &pixels), 1),
                    &View {
                        glyphs,
                        ..View::default()
                    },
                    &crate::view::geometry(
                        &View {
                            glyphs,
                            ..View::default()
                        },
                        (4, 4),
                        area
                    ),
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
    }

    #[test]
    fn rgb_overview_averages_source_rows_into_distinct_halves() {
        let pixels = [[0, 20, 100], [100, 40, 200], [200, 60, 0], [254, 80, 100]];
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        assert!(!render(
            Pixels::Rgb(&rgb(1, 4, &pixels), 1),
            &View::default(),
            &crate::view::geometry(&View::default(), (1, 4), area),
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
            &View::default(),
            &crate::view::geometry(&View::default(), (1, 2), area),
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
            &View {
                exact: true,
                ..View::default()
            },
            &crate::view::geometry(
                &View {
                    exact: true,
                    ..View::default()
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
        let view = View {
            exact: true,
            ..View::default()
        };
        render(
            indexed(&vec![0; 120 * 120], &palette),
            &view,
            &crate::view::geometry(&view, (120, 120), area),
            &mut buffer,
            &mut OverviewCaches::default(),
        );
        // The square grid fits 9x4 cells inside the 14x4 border of a 16x6 frame at x 32.
        assert_eq!(buffer[(32, 0)].symbol(), "┌");
        assert_eq!(buffer[(47, 5)].symbol(), "┘");
        assert_eq!(buffer[(33, 2)].symbol(), " ");
        assert_eq!(buffer[(33, 2)].bg, Color::Reset);
        let (_, map) = view.minimap(120, 120, area).unwrap();
        assert_eq!(buffer[(map.x, map.y)].fg, Color::Rgb(0, 0, 0));
    }
}
