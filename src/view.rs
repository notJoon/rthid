use image::{
    RgbaImage,
    imageops::{self, FilterType},
};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Rect},
    style::Color,
    widgets::{Block, Clear, Widget},
};
use rthid::dither::PaletteColor;

#[derive(Clone, Copy)]
pub enum Pixels<'a> {
    Indexed {
        indices: &'a [u8],
        palette: &'a [PaletteColor],
    },
    Rgb(&'a RgbaImage),
}

impl Pixels<'_> {
    fn rgb(self, i: usize) -> [u8; 3] {
        match self {
            Self::Indexed { indices, palette } => palette[usize::from(indices[i])].rgb,
            Self::Rgb(pixels) => pixels.as_raw()[i * 4..i * 4 + 3].try_into().unwrap(),
        }
    }
}

#[derive(Default)]
pub struct RgbOverviewCache {
    entries: Vec<((u16, u16), Vec<Color>)>,
}

impl RgbOverviewCache {
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn colors(&mut self, pixels: &RgbaImage, cols: u16, rows: u16) -> &[Color] {
        let key = (cols, rows);
        let index = if let Some(index) = self.entries.iter().position(|(size, _)| *size == key) {
            index
        } else {
            if self.entries.len() == 2 {
                self.entries.remove(0);
            }
            self.entries.push((key, average_rgb(pixels, cols, rows)));
            self.entries.len() - 1
        };
        &self.entries[index].1
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyphs {
    #[default]
    Sextant,
    Quadrant,
}

impl Glyphs {
    pub fn for_terminal(program: Option<&str>) -> Self {
        if program == Some("Apple_Terminal") {
            Self::Quadrant
        } else {
            Self::Sextant
        }
    }

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

// Fits a grid into whole cells, keeping its aspect on cells about 2.25 times taller than wide.
// The glyph set only changes how densely each cell is sampled, not the fitted shape.
pub fn fit(w: u32, h: u32, area: Rect) -> (u16, u16) {
    fit_aspect(w, h, area, 2.25)
}

fn fit_aspect(w: u32, h: u32, area: Rect, aspect: f64) -> (u16, u16) {
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

// Build an image that covers whole terminal cells without a second widget resize.
pub fn original_image(
    grid: &RgbaImage,
    view: &View,
    area: Rect,
    font: (u16, u16),
) -> (Rect, RgbaImage) {
    let (w, h) = grid.dimensions();
    let (fw, fh) = (u32::from(font.0), u32::from(font.1));
    if area.is_empty() || w == 0 || h == 0 || fw == 0 || fh == 0 {
        return (Rect::new(area.x, area.y, 0, 0), RgbaImage::new(0, 0));
    }
    if view.exact {
        let mut view = *view;
        view.clamp(w, h, area);
        let (vw, vh) = view.visible(w, h, area);
        let rect = Rect::new(area.x, area.y, vw as u16, vh.div_ceil(2) as u16);
        let crop = imageops::crop_imm(grid, view.x, view.y, vw, vh);
        let mut image = imageops::resize(
            &*crop,
            vw * fw,
            u32::from(rect.height) * fh,
            FilterType::Nearest,
        );
        for pixel in image.pixels_mut() {
            pixel[3] = 255;
        }
        return (rect, image);
    }
    let (cw, ch) = fit_aspect(w, h, area, f64::from(fh) / f64::from(fw));
    let rect = area.centered(Constraint::Length(cw), Constraint::Length(ch));
    let (pw, ph) = (u32::from(cw) * fw, u32::from(ch) * fh);
    let filter = if pw < w || ph < h {
        FilterType::Triangle
    } else {
        FilterType::Nearest
    };
    let mut resized = image::DynamicImage::ImageRgba8(grid.clone())
        .resize(pw, ph, filter)
        .into_rgba8();
    for pixel in resized.pixels_mut() {
        pixel[3] = 255;
    }
    if resized.dimensions() == (pw, ph) {
        return (rect, resized);
    }
    let mut image = RgbaImage::new(pw, ph);
    imageops::overlay(
        &mut image,
        &resized,
        i64::from((pw - resized.width()) / 2),
        i64::from((ph - resized.height()) / 2),
    );
    (rect, image)
}

#[derive(Default, Clone, Copy, Debug)]
pub struct View {
    pub glyphs: Glyphs,
    pub exact: bool,
    pub hide_minimap: bool,
    pub x: u32,
    pub y: u32,
}

impl View {
    pub fn minimap(&self, w: u32, h: u32, area: Rect) -> Option<(Rect, Rect)> {
        // A bordered frame in the top-right corner; the overview fits inside the border.
        let fw = (area.width / 3).min(18);
        let frame = Rect::new(area.right() - fw, area.y, fw, (area.height / 3).min(10));
        let inner = Block::bordered().inner(frame);
        if !self.exact || self.hide_minimap || inner.width < 3 || inner.height < 3 {
            return None;
        }
        let (mw, mh) = fit(w, h, inner);
        Some((
            frame,
            inner.centered(Constraint::Length(mw), Constraint::Length(mh)),
        ))
    }

    pub fn visible(&self, w: u32, h: u32, area: Rect) -> (u32, u32) {
        (
            w.min(u32::from(area.width)),
            h.min(u32::from(area.height) * 2),
        )
    }

    pub fn clamp(&mut self, w: u32, h: u32, area: Rect) {
        let (vw, vh) = self.visible(w, h, area);
        self.x = self.x.min(w.saturating_sub(vw));
        self.y = self.y.min(h.saturating_sub(vh));
    }

    pub fn pan(&mut self, dx: i64, dy: i64, w: u32, h: u32, area: Rect) {
        self.x = i64::from(self.x)
            .saturating_add(dx)
            .clamp(0, i64::from(u32::MAX)) as u32;
        self.y = i64::from(self.y)
            .saturating_add(dy)
            .clamp(0, i64::from(u32::MAX)) as u32;
        self.clamp(w, h, area);
    }

    pub fn render_minimap(
        &self,
        pixels: Pixels<'_>,
        w: u32,
        h: u32,
        area: Rect,
        buffer: &mut Buffer,
        cache: &mut RgbOverviewCache,
    ) -> Option<(Rect, Rect, Rect)> {
        let (frame, map) = self.minimap(w, h, area)?;
        // Clear letterboxing so the 1:1 dots cannot show through the inset.
        Clear.render(frame, buffer);
        render_overview(
            pixels,
            w,
            h,
            self.glyphs,
            Block::bordered().inner(frame),
            buffer,
            cache,
        );
        Some((
            frame,
            map,
            self.viewport_rect(w, h, self.visible(w, h, area), map),
        ))
    }

    pub fn center_on_minimap(&mut self, point: (u16, u16), w: u32, h: u32, area: Rect, map: Rect) {
        let (vw, vh) = self.visible(w, h, area);
        let center = |cell: u16, cells: u16, dots: u32| {
            ((u64::from(cell) * 2 + 1) * u64::from(dots) / (u64::from(cells) * 2)) as i64
        };
        self.pan(
            center(point.0 - map.x, map.width, w) - i64::from(vw / 2) - i64::from(self.x),
            center(point.1 - map.y, map.height, h) - i64::from(vh / 2) - i64::from(self.y),
            w,
            h,
            area,
        );
    }

    pub fn viewport_rect(&self, w: u32, h: u32, visible: (u32, u32), map: Rect) -> Rect {
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

pub fn render(
    pixels: Pixels<'_>,
    w: u32,
    h: u32,
    view: &View,
    area: Rect,
    buffer: &mut Buffer,
    cache: &mut RgbOverviewCache,
) -> bool {
    if area.is_empty() {
        return false;
    }
    let mut view = *view;
    view.clamp(w, h, area);
    if view.exact {
        render_exact(pixels, w, h, &view, area, buffer, cache);
        false
    } else {
        render_overview(pixels, w, h, view.glyphs, area, buffer, cache)
    }
}

fn render_exact(
    pixels: Pixels<'_>,
    w: u32,
    h: u32,
    view: &View,
    area: Rect,
    buffer: &mut Buffer,
    cache: &mut RgbOverviewCache,
) {
    let color = |i| {
        let [r, g, b] = pixels.rgb(i);
        Color::Rgb(r, g, b)
    };
    let (vw, vh) = view.visible(w, h, area);
    for y in (0..vh).step_by(2) {
        for x in 0..vw {
            let index = ((view.y + y) * w + view.x + x) as usize;
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
    draw_minimap(pixels, w, h, view, area, buffer, cache);
}

pub fn draw_minimap(
    pixels: Pixels<'_>,
    w: u32,
    h: u32,
    view: &View,
    area: Rect,
    buffer: &mut Buffer,
    cache: &mut RgbOverviewCache,
) {
    if let Some((frame, map, viewport)) = view.render_minimap(pixels, w, h, area, buffer, cache) {
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
}

fn render_overview(
    pixels: Pixels<'_>,
    w: u32,
    h: u32,
    glyphs: Glyphs,
    area: Rect,
    buffer: &mut Buffer,
    cache: &mut RgbOverviewCache,
) -> bool {
    let (indices, palette) = match pixels {
        Pixels::Rgb(pixels) => {
            let (cols, rows) = fit(w, h, area);
            let colors = cache.colors(pixels, cols, rows);
            let target = area.centered(Constraint::Length(cols), Constraint::Length(rows));
            render_overview_rgb(colors, target, buffer);
            return false;
        }
        Pixels::Indexed { indices, palette } => (indices, palette),
    };
    let (cw, ch) = fit(w, h, area);
    let target = area.centered(Constraint::Length(cw), Constraint::Length(ch));
    let color = |i: u8| {
        let [r, g, b] = palette[usize::from(i)].rgb;
        Color::Rgb(r, g, b)
    };
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
            let mut counts = [0u8; 256];
            for &i in samples.iter() {
                counts[usize::from(i)] += 1;
            }
            let mut ranked = [0u8; 6];
            let ranked = &mut ranked[..samples.len()];
            ranked.copy_from_slice(samples);
            ranked.sort_by_key(|&i| (std::cmp::Reverse(counts[usize::from(i)]), i));
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
            let glyph = glyphs.glyph(mask);
            buffer[(target.x + cx, target.y + cy)]
                .set_char(glyph)
                .set_fg(color(fg))
                .set_bg(color(bg));
        }
    }
    approximate
}

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
                for (total, channel) in sum
                    .iter_mut()
                    .zip(pixels.get_pixel(x as u32, y as u32).0[..3].iter().copied())
                {
                    *total += u64::from(channel);
                }
            }
        }
        let [r, g, b] = sum.map(|total| (total / ((x1 - x0) * (y1 - y0))) as u8);
        Color::Rgb(r, g, b)
    };
    (0..u32::from(ch) * 2)
        .flat_map(|row| (0..cw).map(move |col| average(col, row)))
        .collect()
}

fn render_overview_rgb(colors: &[Color], target: Rect, buffer: &mut Buffer) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;
    use ratatui::{buffer::Buffer, layout::Rect, style::Color};
    use rthid::dither::make_palette;

    fn indexed<'a>(indices: &'a [u8], palette: &'a [PaletteColor]) -> Pixels<'a> {
        Pixels::Indexed { indices, palette }
    }

    fn rgb(w: u32, h: u32, pixels: &[[u8; 3]]) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let [r, g, b] = pixels[(y * w + x) as usize];
            Rgba([r, g, b, 255])
        })
    }

    fn checkerboard(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            if (x + y) % 2 == 0 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        })
    }

    #[test]
    fn graphics_exact_uses_visible_crop_and_nearest_pixels() {
        let pixels = checkerboard(7, 9);
        let view = View {
            exact: true,
            x: 2,
            y: 3,
            ..View::default()
        };
        let (rect, image) = original_image(&pixels, &view, Rect::new(4, 5, 3, 2), (6, 15));
        assert_eq!(rect, Rect::new(4, 5, 3, 2));
        assert_eq!(image.dimensions(), (18, 30));
        for (x, y, pixel) in image.enumerate_pixels() {
            let [r, g, b, _] = pixels[(view.x + x / 6, view.y + (2 * y + 1) * 4 / 60)].0;
            assert_eq!(*pixel, Rgba([r, g, b, 255]));
        }
        let (rect, image) = original_image(
            &pixels,
            &View {
                exact: true,
                ..View::default()
            },
            Rect::new(1, 2, 20, 20),
            (6, 15),
        );
        assert_eq!(rect, Rect::new(1, 2, 7, 5));
        assert_eq!(image.dimensions(), (42, 75));
        assert!(
            image
                .pixels()
                .all(|p| p.0 == [255, 0, 0, 255] || p.0 == [0, 0, 255, 255])
        );
    }

    #[test]
    fn graphics_overview_uses_font_aspect_and_preserves_crisp_upscaling() {
        let pixels = checkerboard(4, 4);
        let (rect, image) =
            original_image(&pixels, &View::default(), Rect::new(2, 3, 20, 10), (8, 16));
        assert_eq!(rect, Rect::new(2, 3, 20, 10));
        assert_eq!(image.dimensions(), (160, 160));
        assert!(
            image
                .pixels()
                .all(|p| p.0 == [255, 0, 0, 255] || p.0 == [0, 0, 255, 255])
        );
        let (rect, image) =
            original_image(&pixels, &View::default(), Rect::new(2, 3, 20, 10), (8, 8));
        assert_eq!(rect, Rect::new(7, 3, 10, 10));
        assert_eq!(image.dimensions(), (80, 80));
    }

    #[test]
    fn graphics_ignores_source_alpha() {
        let grid = RgbaImage::from_raw(1, 2, vec![25, 26, 128, 0, 231, 47, 93, 128]).unwrap();
        for exact in [false, true] {
            let (_, image) = original_image(
                &grid,
                &View {
                    exact,
                    ..View::default()
                },
                Rect::new(0, 0, 1, 1),
                (1, 2),
            );
            assert_eq!(image.as_raw(), &[25, 26, 128, 255, 231, 47, 93, 255]);
        }
    }

    #[test]
    fn graphics_overview_pads_rounding_gaps_and_downscales() {
        let pixels = RgbaImage::from_pixel(300, 100, Rgba([10, 20, 30, 255]));
        let (rect, image) =
            original_image(&pixels, &View::default(), Rect::new(2, 3, 11, 10), (7, 15));
        assert_eq!(rect, Rect::new(2, 8, 11, 1));
        assert_eq!(image.dimensions(), (77, 15));
        let opaque: Vec<_> = image
            .enumerate_pixels()
            .filter(|(_, _, p)| p[3] == 255)
            .collect();
        let width = opaque.iter().map(|(x, _, _)| x).max().unwrap()
            - opaque.iter().map(|(x, _, _)| x).min().unwrap()
            + 1;
        assert_eq!(width, 45);
        assert!(opaque.iter().all(|(_, _, p)| p.0 == [10, 20, 30, 255]));
        assert_eq!(image.get_pixel(0, 0)[3], 0);
        for area in [Rect::new(0, 0, 0, 3), Rect::new(0, 0, 3, 0)] {
            let (rect, image) = original_image(&pixels, &View::default(), area, (7, 15));
            assert!(rect.is_empty());
            assert_eq!(image.dimensions(), (0, 0));
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
            w,
            h,
            &view,
            area,
            &mut buffer,
            &mut RgbOverviewCache::default(),
        );
        let is_block = |cell: &ratatui::buffer::Cell| {
            cell.symbol()
                .chars()
                .all(|c| c == ' ' || ('\u{2580}'..='\u{259f}').contains(&c))
        };
        assert!(buffer.content.iter().all(is_block));
        assert!(buffer.content.iter().any(|cell| cell.symbol() != " "));
        view.exact = true;
        let (_, map, _) = view
            .render_minimap(
                indexed(&indices, &palette),
                w,
                h,
                area,
                &mut buffer,
                &mut RgbOverviewCache::default(),
            )
            .unwrap();
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
        let (frame, map, _) = view
            .render_minimap(
                indexed(&indices, &palette),
                w,
                h,
                area,
                &mut baseline,
                &mut RgbOverviewCache::default(),
            )
            .unwrap();
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
                w,
                h,
                &view,
                area,
                &mut current,
                &mut RgbOverviewCache::default(),
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
        let (frame, map, _) = view
            .render_minimap(
                indexed(&indices, &palette),
                w,
                h,
                area,
                &mut baseline,
                &mut RgbOverviewCache::default(),
            )
            .unwrap();
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
                        w,
                        h,
                        &view,
                        area,
                        f.buffer_mut(),
                        &mut RgbOverviewCache::default(),
                    );
                })
                .unwrap();
            let current = terminal.backend().buffer().clone();
            let completed = ansi_terminal
                .draw(|f| {
                    render(
                        indexed(&indices, &palette),
                        w,
                        h,
                        &view,
                        area,
                        f.buffer_mut(),
                        &mut RgbOverviewCache::default(),
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
        assert_eq!(fit(120, 120, Rect::new(0, 0, 0, 5)), (0, 0));
        assert_eq!(fit(120, 120, Rect::new(0, 0, 5, 0)), (0, 0));
    }

    #[test]
    fn overview_fit_corrects_subcell_aspect() {
        assert_eq!(fit(120, 120, Rect::new(0, 0, 60, 40)), (60, 26));
        assert_eq!(fit(120, 120, Rect::new(0, 0, 20, 5)), (11, 5));
        assert_eq!(fit(1, 1000, Rect::new(0, 0, 1, 1)), (1, 1));
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
        // Sample columns 1 and 3 of each row; column 0 must not participate.
        assert!(!render(
            indexed(&[2, 0, 2, 1, 2, 0, 2, 1, 2, 0, 2, 1], &palette),
            4,
            3,
            &View::default(),
            area,
            &mut buffer,
            &mut RgbOverviewCache::default()
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▐");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(51, 51, 51));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(0, 0, 0));
        assert!(render(
            indexed(&[0, 2, 0, 2, 1, 2], &palette),
            2,
            3,
            &View::default(),
            area,
            &mut buffer,
            &mut RgbOverviewCache::default()
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
            4,
            4,
            &view,
            area,
            &mut buffer,
            &mut RgbOverviewCache::default()
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▐");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(51, 51, 51));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(0, 0, 0));
        assert!(render(
            indexed(&[0, 2, 1, 2], &palette),
            2,
            2,
            &view,
            area,
            &mut buffer,
            &mut RgbOverviewCache::default()
        ));
        assert_eq!(buffer[(0, 0)].symbol(), "▌");
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(0, 0, 0));
        assert_eq!(buffer[(0, 0)].bg, Color::Rgb(255, 255, 255));
    }

    #[test]
    fn rgb_overview_cache_reuses_two_sizes_and_clears() {
        let pixels = rgb(1, 2, &[[25, 26, 128], [231, 47, 93]]);
        let mut cache = RgbOverviewCache::default();
        let first = cache.colors(&pixels, 1, 1).as_ptr();
        cache.colors(&pixels, 2, 2);
        assert_eq!(cache.colors(&pixels, 1, 1).as_ptr(), first);
        assert_eq!(cache.entries.len(), 2);
        cache.colors(&pixels, 3, 3);
        assert_eq!(cache.entries.len(), 2);
        assert_eq!(cache.entries[0].0, (2, 2));
        cache.clear();
        let updated = rgb(1, 2, &[[0; 3], [255; 3]]);
        assert_eq!(
            cache.colors(&updated, 1, 1),
            &[Color::Rgb(0, 0, 0), Color::Rgb(255, 255, 255)]
        );
    }

    #[test]
    fn rgb_exact_preserves_each_pixel() {
        let pixels = RgbaImage::from_raw(1, 2, vec![25, 26, 128, 0, 231, 47, 93, 128]).unwrap();
        let area = Rect::new(0, 0, 1, 1);
        let mut buffer = Buffer::empty(area);
        assert!(!render(
            Pixels::Rgb(&pixels),
            1,
            2,
            &View {
                exact: true,
                ..View::default()
            },
            area,
            &mut buffer,
            &mut RgbOverviewCache::default()
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
                    Pixels::Rgb(&rgb(4, 4, &pixels)),
                    4,
                    4,
                    &View {
                        glyphs,
                        ..View::default()
                    },
                    area,
                    &mut buffer,
                    &mut RgbOverviewCache::default()
                ));
                let (cw, ch) = fit(4, 4, area);
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
            Pixels::Rgb(&rgb(1, 4, &pixels)),
            1,
            4,
            &View::default(),
            area,
            &mut buffer,
            &mut RgbOverviewCache::default()
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
            Pixels::Rgb(&rgb(1, 2, &pixels)),
            1,
            2,
            &View::default(),
            area,
            &mut buffer,
            &mut RgbOverviewCache::default()
        ));
        let (cw, ch) = fit(1, 2, area);
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
            1,
            3,
            &View {
                exact: true,
                ..View::default()
            },
            area,
            &mut buffer,
            &mut RgbOverviewCache::default()
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
            120,
            120,
            &view,
            area,
            &mut buffer,
            &mut RgbOverviewCache::default(),
        );
        // A 16x6 frame at x 32; the square grid fits 9x4 cells inside its 14x4 border.
        assert_eq!(buffer[(32, 0)].symbol(), "┌");
        assert_eq!(buffer[(47, 5)].symbol(), "┘");
        assert_eq!(buffer[(33, 2)].symbol(), " ");
        assert_eq!(buffer[(33, 2)].bg, Color::Reset);
        let (_, map) = view.minimap(120, 120, area).unwrap();
        assert_eq!(buffer[(map.x, map.y)].fg, Color::Rgb(0, 0, 0));
    }
}
