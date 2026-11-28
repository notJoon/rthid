//! Pure preview geometry shared by input and rendering.
//!
//! Dots are grid pixels. Fit and Out preserve the terminal cell aspect.
//! In uses crisp half blocks, so dots look 12.5 percent taller.

use ratatui::{
    layout::{Constraint, Rect},
    widgets::Block,
};

/// Typical terminal cell height divided by its width, regardless of glyph set.
pub(crate) const CELL_ASPECT: f64 = 2.25;

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

/// Preview scale, ordered from the fitted image to enlarged dots.
///
/// A dot is one pixel of the composed card grid. A terminal cell is about
/// [`CELL_ASPECT`] times taller than wide, so it splits into a top and a bottom
/// half cell that are nearly square. The status line prints [`Zoom::scale`] as a
/// percentage, so 100% means one dot per cell column.
///
/// | Zoom     | Status          | Dots per cell, columns x rows ([`Zoom::dots_per_cell`]) |
/// |----------|-----------------|------------------------------------|
/// | `Fit`    | `Fit (sextant)` | whatever fits the whole grid       |
/// | `Out(4)` | `25%`           | 4 x 9                              |
/// | `Out(3)` | `33%`           | 3 x 6.75                           |
/// | `Out(2)` | `50%`           | 2 x 4.5                            |
/// | `In(1)`  | `100%`          | 1 x 2                              |
/// | `In(2)`  | `200%`          | 1/2 x 1                            |
/// | `In(3)`  | `300%`          | 1/3 x 2/3                          |
/// | `In(4)`  | `400%`          | 1/4 x 1/2                          |
///
/// # In(k): exact dots
///
/// Every cell is `▀` with the top half cell as foreground and the bottom half
/// cell as background, and a cell whose halves share a color is drawn as a
/// space on that background. No color is mixed or dropped. A half cell is about
/// 1.125 times taller than wide, so dots look slightly tall.
///
/// At In(1) each dot fills one half cell, so a cell shows two vertically
/// adjacent dots. Below an odd last row the bottom half keeps the terminal
/// default color.
///
/// ```text
/// dots       cells
/// a b c d    ▀▀▀▀    fg a b c d    bg e f g h
/// e f g h
/// i j k l    ▀▀▀▀    fg i j k l    bg m n o p
/// m n o p
/// ```
///
/// At In(k) each dot fills k cell columns and k half rows. With an even k both
/// halves of every cell come from the same dot, so In(2) and In(4) draw only
/// spaces.
///
/// ```text
/// In(2)
/// dots    half cells    cells, all spaces
/// a b     a a b b       bg a a b b
/// e f     a a b b
///         e e f f       bg e e f f
///         e e f f
/// ```
///
/// With an odd k a dot spans one and a half cells, so every other dot row
/// boundary falls inside a cell and that cell keeps a `▀`.
///
/// ```text
/// In(3), one column
/// dots    half rows    cell rows
/// a       0 a          0  a over a    space, bg a
///         1 a
/// e       2 a          1  a over e    ▀, fg a, bg e
///         3 e
///         4 e          2  e over e    space, bg e
///         5 e
/// ```
///
/// # Out(n) and Fit: sampled cells
///
/// Many dots feed one cell. At Out(n) a cell covers n dot columns and
/// n x [`CELL_ASPECT`] dot rows, so the image keeps its shape on screen.
/// Fractional row counts are rounded at each half cell or subcell boundary, so
/// at Out(2) and Out(3) neighbouring cells cover slightly different row counts.
/// Fit uses the same rules with the block that fits the whole grid into
/// [`PreviewGeometry::target`], and repeats dots when the grid is smaller than
/// the target.
///
/// Original averages every dot in each half cell box (`cells::average_rgb`) and
/// then draws `▀` or a space like In(k). Dithered keeps exact palette colors. It
/// takes one dot at the center of each glyph subcell, 2 x 2 for quadrant and
/// 2 x 3 for sextant (`cells::render_overview_indexed`). Then `cells::reduce_cell`
/// keeps the most frequent sample as background and the next distinct one as foreground,
/// snaps any third color to the nearer of the two, and draws the quadrant or
/// sextant glyph whose filled subcells hold the foreground. Fit reports that
/// snap as `approximate` in the status line.
///
/// ```text
/// Out(4), one cell covers 4 x 9 dots
/// row   Original             Quadrant    Sextant
/// 0     t t t t              . . . .     . . . .
/// 1     t t t t   top =      . . . .     . A . B
/// 2     t t t t   mean of t  . A . B     . . . .
/// 3     t t t t              . . . .     . . . .
/// 4     b b b b              . . . .     . C . D
/// 5     b b b b   bottom =   . . . .     . . . .
/// 6     b b b b   mean of b  . C . D     . . . .
/// 7     b b b b              . . . .     . E . F
/// 8     b b b b              . . . .     . . . .
/// ```
///
/// With Original at Fit, a graphics backend and the brush off, the preview is drawn
/// as a real image and none of these cells are painted.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    /// The whole grid fitted to the preview with the cell aspect preserved.
    /// Labeled `Fit` with the glyph set name.
    #[default]
    Fit,
    /// Dots per cell column, from two through four, with n x [`CELL_ASPECT`]
    /// dot rows per cell row. Labeled 50%, 33% and 25%.
    Out(u8),
    /// Cells wide and half cells tall per dot, from one through four.
    /// Labeled 100% through 400%.
    In(u8),
}

impl Zoom {
    /// Cell columns per dot, with Fit determined by the available area.
    pub fn scale(self, grid: (u32, u32), area: Rect) -> f64 {
        match self {
            Self::Fit => {
                if grid.0 == 0 || grid.1 == 0 || area.is_empty() {
                    return 0.0;
                }
                (f64::from(area.width) / f64::from(grid.0))
                    .min(f64::from(area.height) * CELL_ASPECT / f64::from(grid.1))
            }
            Self::Out(n) => 1.0 / f64::from(n),
            Self::In(k) => f64::from(k),
        }
    }

    /// Next scale in the chosen direction, skipping scales no larger than Fit.
    pub fn step(self, inward: bool, grid: (u32, u32), area: Rect) -> Self {
        let steps = [
            Self::Out(4),
            Self::Out(3),
            Self::Out(2),
            Self::In(1),
            Self::In(2),
            Self::In(3),
            Self::In(4),
        ];
        let fit = Self::Fit.scale(grid, area);
        let current = self.scale(grid, area);
        if inward {
            steps
                .into_iter()
                .find(|s| s.scale(grid, area) > current.max(fit))
                .unwrap_or(self)
        } else {
            steps
                .into_iter()
                .rev()
                .find(|s| {
                    let scale = s.scale(grid, area);
                    scale < current && scale > fit
                })
                .unwrap_or(Self::Fit)
        }
    }

    /// Dots covered by one cell on each axis.
    pub fn dots_per_cell(self, grid: (u32, u32), area: Rect) -> (f64, f64) {
        match self {
            Self::In(k) => (1.0 / f64::from(k), 2.0 / f64::from(k)),
            _ => {
                let dots = 1.0 / self.scale(grid, area);
                (dots, dots * CELL_ASPECT)
            }
        }
    }
}

/// User controlled zoom, minimap visibility and dot origin.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    /// Current preview scale.
    pub zoom: Zoom,
    /// Whether the user has hidden the minimap.
    pub hide_minimap: bool,
    /// Leftmost grid column before clamping.
    pub x: u32,
    /// Topmost grid row before clamping.
    pub y: u32,
}

/// Preview coordinates shared by input and rendering without mutating the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewGeometry {
    /// Grid dimensions in dots after scaling and rotation.
    pub grid: (u32, u32),
    /// Terminal cells available for the preview.
    pub area: Rect,
    /// Current preview scale.
    pub zoom: Zoom,
    /// Fully visible dot dimensions used for navigation, status and overflow.
    pub visible: (u32, u32),
    /// Dot dimensions to paint, which at In(k) also include the partially visible edge dot.
    pub drawn: (u32, u32),
    /// Clamped dot origin of the visible region.
    pub origin: (u32, u32),
    /// Cells occupied by the visible region.
    pub target: Rect,
    /// Optional minimap when the image overflows the preview.
    pub minimap: Option<Minimap>,
}

impl PreviewGeometry {
    /// Grid dot under the cell center, excluding cells outside the preview target.
    /// Integer coordinates are dot centers. A trailing partial cell maps to the
    /// last dot it draws.
    pub(crate) fn dot_at(&self, column: u16, row: u16) -> Option<(f64, f64)> {
        if !self.target.contains((column, row).into()) {
            return None;
        }
        let (dx, dy) = if self.zoom == Zoom::Fit {
            (
                f64::from(self.grid.0) / f64::from(self.target.width),
                f64::from(self.grid.1) / f64::from(self.target.height),
            )
        } else {
            self.zoom.dots_per_cell(self.grid, self.area)
        };
        let last = |len: u32| f64::from(len.saturating_sub(1));
        Some((
            (f64::from(self.origin.0) + (f64::from(column - self.target.x) + 0.5) * dx - 0.5)
                .clamp(0.0, last(self.grid.0)),
            (f64::from(self.origin.1) + (f64::from(row - self.target.y) + 0.5) * dy - 0.5)
                .clamp(0.0, last(self.grid.1)),
        ))
    }

    /// Whether any image dots lie outside the visible region.
    pub fn overflows(&self) -> bool {
        self.grid.0 > self.visible.0 || self.grid.1 > self.visible.1
    }

    /// Whole dot distance for a keyboard cell step on each axis.
    pub fn pan_step(&self) -> (u32, u32) {
        if self.zoom == Zoom::Fit || self.target.is_empty() {
            return (1, 1);
        }
        let (x, y) = self.zoom.dots_per_cell(self.grid, self.area);
        (x.round().max(1.0) as u32, y.round().max(1.0) as u32)
    }
}

/// Minimap placement in the upper right of the preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Minimap {
    /// Inset including its border.
    pub frame: Rect,
    /// Cells of the fitted image inside the border.
    pub map: Rect,
    /// Map cells covering the visible dots, which stay undimmed.
    pub viewport: Rect,
}

/// Clamped visible region, target cells and optional minimap for this preview.
pub fn geometry(viewport: &Viewport, grid: (u32, u32), area: Rect) -> PreviewGeometry {
    let (w, h) = grid;
    let mut viewport = *viewport;
    viewport.clamp(w, h, area);
    let visible = viewport.visible(w, h, area);
    // In(k) paints the partial edge dot as well, capped by the grid remaining from the origin
    // so painting never reads past the image. Other zooms paint exactly the visible region.
    let drawn = if let Zoom::In(k) = viewport.zoom {
        let k = u32::from(k);
        (
            (w - viewport.x).min(u32::from(area.width).div_ceil(k)),
            (h - viewport.y).min((u32::from(area.height) * 2).div_ceil(k)),
        )
    } else {
        visible
    };
    let (cw, ch) = if area.is_empty() || w == 0 || h == 0 {
        (0, 0)
    } else if viewport.zoom == Zoom::Fit {
        fit_aspect(w, h, area, CELL_ASPECT)
    } else {
        let (dx, dy) = viewport.zoom.dots_per_cell(grid, area);
        (
            ((f64::from(drawn.0) / dx).ceil() as u16).min(area.width),
            ((f64::from(drawn.1) / dy).ceil() as u16).min(area.height),
        )
    };
    let target = if viewport.zoom == Zoom::Fit {
        area.centered(Constraint::Length(cw), Constraint::Length(ch))
    } else {
        Rect::new(area.x, area.y, cw, ch)
    };
    let minimap = viewport.minimap(w, h, area).map(|(frame, map)| Minimap {
        frame,
        map,
        viewport: viewport.viewport_rect(w, h, visible, map),
    });
    PreviewGeometry {
        grid,
        area,
        zoom: viewport.zoom,
        visible,
        drawn,
        origin: (viewport.x, viewport.y),
        target,
        minimap,
    }
}

impl Viewport {
    /// Minimap border and fitted map, absent when hidden, unnecessary or too small.
    pub fn minimap(&self, w: u32, h: u32, area: Rect) -> Option<(Rect, Rect)> {
        let fw = (area.width / 3).min(18);
        let frame = Rect::new(area.right() - fw, area.y, fw, (area.height / 3).min(10));
        let inner = Block::bordered().inner(frame);
        let (vw, vh) = self.visible(w, h, area);
        if w == 0
            || h == 0
            || self.zoom == Zoom::Fit
            || (w <= vw && h <= vh)
            || self.hide_minimap
            || inner.width < 3
            || inner.height < 3
        {
            return None;
        }
        let (mw, mh) = fit_aspect(w, h, inner, CELL_ASPECT);
        let frame = Rect::new(area.right() - mw - 2, area.y, mw + 2, mh + 2);
        let map = Block::bordered().inner(frame);
        Some((frame, map))
    }

    /// Whole dots that fit. A clipped edge dot is excluded so it stays reachable by panning
    /// and the status line does not report it as shown.
    pub(crate) fn visible(&self, w: u32, h: u32, area: Rect) -> (u32, u32) {
        if self.zoom == Zoom::Fit {
            return (w, h);
        }
        if area.is_empty() {
            return (0, 0);
        }
        let (dx, dy) = self.zoom.dots_per_cell((w, h), area);
        let count = |cells: u16, dots: f64| (f64::from(cells) * dots).floor() as u32;
        (w.min(count(area.width, dx)), h.min(count(area.height, dy)))
    }

    /// Keeps the visible region inside the grid.
    pub fn clamp(&mut self, w: u32, h: u32, area: Rect) {
        let (vw, vh) = self.visible(w, h, area);
        self.x = self.x.min(w.saturating_sub(vw));
        self.y = self.y.min(h.saturating_sub(vh));
    }

    /// Changes scale around the visible center, then clamps to the grid.
    pub fn set_zoom(&mut self, zoom: Zoom, geometry: &PreviewGeometry) {
        self.zoom = zoom;
        let (w, h) = geometry.grid;
        let next = self.visible(w, h, geometry.area);
        self.x = (geometry.origin.0 + geometry.visible.0 / 2).saturating_sub(next.0 / 2);
        self.y = (geometry.origin.1 + geometry.visible.1 / 2).saturating_sub(next.1 / 2);
        self.clamp(w, h, geometry.area);
    }

    /// Shifts the visible region by dots and clamps to the grid.
    pub fn pan(&mut self, dx: i64, dy: i64, geometry: &PreviewGeometry) {
        self.x = i64::from(geometry.origin.0)
            .saturating_add(dx)
            .clamp(0, i64::from(u32::MAX)) as u32;
        self.y = i64::from(geometry.origin.1)
            .saturating_add(dy)
            .clamp(0, i64::from(u32::MAX)) as u32;
        self.clamp(geometry.grid.0, geometry.grid.1, geometry.area);
    }

    /// Centers the visible region on the dots under a terminal cell inside the map.
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
            center(point.0 - map.x, map.width, w)
                - i64::from(vw / 2)
                - i64::from(geometry.origin.0),
            center(point.1 - map.y, map.height, h)
                - i64::from(vh / 2)
                - i64::from(geometry.origin.1),
            geometry,
        );
    }

    /// Map cells covering the visible dots, at least one cell on each axis.
    pub(crate) fn viewport_rect(&self, w: u32, h: u32, visible: (u32, u32), map: Rect) -> Rect {
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

#[cfg(test)]
mod tests {
    use super::*;

    const STEPS: [Zoom; 8] = [
        Zoom::Fit,
        Zoom::Out(4),
        Zoom::Out(3),
        Zoom::Out(2),
        Zoom::In(1),
        Zoom::In(2),
        Zoom::In(3),
        Zoom::In(4),
    ];

    #[test]
    fn cell_centers_map_to_dots_at_fit_out_and_in() {
        let area = Rect::new(3, 5, 20, 10);
        for (zoom, target, first, second) in [
            (
                Zoom::Fit,
                Rect::new(3, 6, 20, 8),
                (2.0, 5.125),
                (7.0, 16.375),
            ),
            (Zoom::Out(2), area, (12.5, 9.75), (14.5, 14.25)),
            (Zoom::In(2), area, (11.75, 8.0), (12.25, 9.0)),
        ] {
            let viewport = Viewport {
                zoom,
                x: 12,
                y: 8,
                ..Viewport::default()
            };
            let g = geometry(&viewport, (100, 90), area);
            assert_eq!(g.target, target);
            assert_eq!(g.dot_at(target.x, target.y), Some(first));
            assert_eq!(g.dot_at(target.x + 1, target.y + 1), Some(second));
            assert_eq!(g.dot_at(target.x - 1, target.y), None);
            assert_eq!(g.dot_at(target.right(), target.y), None);
            assert_eq!(g.dot_at(target.x, target.bottom()), None);
        }
    }

    #[test]
    fn trailing_partial_cells_map_to_the_last_dot() {
        let viewport = Viewport {
            zoom: Zoom::In(1),
            ..Viewport::default()
        };
        let area = Rect::new(0, 0, 20, 10);
        for (grid, row, y) in [((10, 1), 0, 0.0), ((10, 3), 1, 2.0)] {
            let g = geometry(&viewport, grid, area);
            assert_eq!(g.dot_at(g.target.x, g.target.y + row), Some((0.0, y)));
        }
    }

    #[test]
    fn partial_edge_dots_remain_reachable() {
        for (zoom, area, visible, origin) in [
            // 47 columns at 3x clip the sixteenth column, while 23 rows at 3x and 31 rows at 4x clip the sixteenth row.
            (Zoom::In(3), Rect::new(0, 0, 47, 24), (15, 16), (1, 0)),
            (Zoom::In(3), Rect::new(0, 0, 48, 23), (16, 15), (0, 1)),
            (Zoom::In(4), Rect::new(0, 0, 64, 31), (16, 15), (0, 1)),
        ] {
            let mut viewport = Viewport {
                zoom,
                ..Viewport::default()
            };
            let before = geometry(&viewport, (16, 16), area);
            assert_eq!(before.visible, visible);
            assert_eq!(before.drawn, (16, 16));
            assert_eq!(before.target, area);
            assert!(before.overflows());
            let map = before.minimap.unwrap();
            assert_eq!(
                map.viewport,
                viewport.viewport_rect(16, 16, visible, map.map)
            );
            viewport.pan(i64::MAX, i64::MAX, &before);
            let after = geometry(&viewport, (16, 16), area);
            assert_eq!(after.origin, origin);
            assert_eq!(after.drawn, visible);
            assert_eq!(
                (after.origin.0 + visible.0, after.origin.1 + visible.1),
                (16, 16)
            );
        }
    }

    #[test]
    fn zoom_step_table_and_limits() {
        let grid = (1000, 1000);
        let area = Rect::new(2, 3, 48, 18);
        for (i, zoom) in STEPS.into_iter().enumerate() {
            assert_eq!(zoom.step(true, grid, area), STEPS[(i + 1).min(7)]);
            assert_eq!(zoom.step(false, grid, area), STEPS[i.saturating_sub(1)]);
        }
        assert_eq!(Zoom::default(), Zoom::Fit);
    }

    #[test]
    fn steps_no_larger_than_fit_are_skipped_in_both_directions() {
        let grid = (120, 1);
        for (width, first) in [
            (20, Zoom::Out(4)),
            (30, Zoom::Out(3)),
            (40, Zoom::Out(2)),
            (60, Zoom::In(1)),
            (120, Zoom::In(2)),
            (240, Zoom::In(3)),
            (360, Zoom::In(4)),
            (480, Zoom::Fit),
            (600, Zoom::Fit),
        ] {
            let area = Rect::new(0, 0, width, 100);
            assert_eq!(Zoom::Fit.step(true, grid, area), first, "width {width}");
            assert_eq!(first.step(false, grid, area), Zoom::Fit, "width {width}");
        }
    }

    #[test]
    fn visible_regions_targets_and_clamps_at_every_zoom() {
        let area = Rect::new(2, 3, 48, 18);
        let grid = (1000, 1000);
        for (zoom, visible) in [
            (Zoom::Fit, grid),
            (Zoom::Out(4), (192, 162)),
            (Zoom::Out(3), (144, 121)),
            (Zoom::Out(2), (96, 81)),
            (Zoom::In(1), (48, 36)),
            (Zoom::In(2), (24, 18)),
            (Zoom::In(3), (16, 12)),
            (Zoom::In(4), (12, 9)),
        ] {
            let mut viewport = Viewport {
                zoom,
                x: 9999,
                y: 9999,
                ..Viewport::default()
            };
            let g = geometry(&viewport, grid, area);
            assert_eq!(g.zoom, zoom);
            assert_eq!(g.visible, visible);
            assert_eq!(g.origin, (1000 - visible.0, 1000 - visible.1));
            assert_eq!((viewport.x, viewport.y), (9999, 9999));
            assert_eq!(
                g.target,
                if zoom == Zoom::Fit {
                    Rect::new(6, 3, 40, 18)
                } else {
                    area
                }
            );
            viewport.pan(i64::MAX, i64::MAX, &g);
            assert_eq!((viewport.x, viewport.y), g.origin);
            viewport.pan(i64::MIN, i64::MIN, &geometry(&viewport, grid, area));
            assert_eq!((viewport.x, viewport.y), (0, 0));
            viewport.pan(1, 1, &geometry(&viewport, grid, area));
            assert_eq!(
                (viewport.x, viewport.y),
                if zoom == Zoom::Fit { (0, 0) } else { (1, 1) }
            );
        }
    }

    #[test]
    fn zoom_preserves_the_center_dot_and_clamps_at_edges() {
        let area = Rect::new(0, 0, 48, 18);
        let grid = (1000, 1000);
        let mut viewport = Viewport::default();
        for zoom in STEPS.into_iter().chain(STEPS.into_iter().rev()) {
            let before = geometry(&viewport, grid, area);
            let center = (
                before.origin.0 + before.visible.0 / 2,
                before.origin.1 + before.visible.1 / 2,
            );
            viewport.set_zoom(zoom, &before);
            let after = geometry(&viewport, grid, area);
            assert_eq!(
                (
                    after.origin.0 + after.visible.0 / 2,
                    after.origin.1 + after.visible.1 / 2
                ),
                center
            );
        }
        viewport.zoom = Zoom::In(4);
        viewport.set_zoom(Zoom::Out(4), &geometry(&viewport, grid, area));
        assert_eq!((viewport.x, viewport.y), (0, 0));
    }

    #[test]
    fn minimap_requires_overflow_space_and_user_visibility() {
        let area = Rect::new(2, 3, 48, 18);
        for zoom in STEPS {
            for hidden in [false, true] {
                for grid in [(120, 120), (1, 1), (0, 0)] {
                    let viewport = Viewport {
                        zoom,
                        hide_minimap: hidden,
                        ..Viewport::default()
                    };
                    let g = geometry(&viewport, grid, area);
                    assert_eq!(g.minimap.is_some(), !hidden && g.overflows());
                    assert!(
                        geometry(&viewport, grid, Rect::new(0, 0, 10, 5))
                            .minimap
                            .is_none()
                    );
                }
            }
        }
        let viewport = Viewport {
            zoom: Zoom::In(1),
            x: 72,
            y: 84,
            ..Viewport::default()
        };
        assert_eq!(
            geometry(&viewport, (120, 120), area).minimap,
            Some(Minimap {
                frame: Rect::new(39, 3, 11, 6),
                map: Rect::new(40, 4, 9, 4),
                viewport: Rect::new(45, 6, 4, 2),
            })
        );
    }

    #[test]
    fn minimap_frame_hugs_wide_map_at_top_right() {
        let area = Rect::new(2, 3, 48, 18);
        let viewport = Viewport {
            zoom: Zoom::In(1),
            ..Viewport::default()
        };
        assert_eq!(
            viewport.minimap(480, 120, area),
            Some((Rect::new(34, 3, 16, 3), Rect::new(35, 4, 14, 1)))
        );
    }

    #[test]
    fn empty_previews_and_aspect_fit() {
        for zoom in STEPS {
            let viewport = Viewport {
                zoom,
                ..Viewport::default()
            };
            for area in [
                Rect::default(),
                Rect::new(2, 3, 0, 5),
                Rect::new(2, 3, 5, 0),
            ] {
                let g = geometry(&viewport, (100, 100), area);
                assert!(g.target.is_empty());
                assert!(g.minimap.is_none());
            }
        }
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
}
