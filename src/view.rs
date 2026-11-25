//! Preview coordinates for the Overview, 1:1 view and minimap.
//!
//! A dot is one grid pixel. The 1:1 view (`exact` in code) shows two dots per cell,
//! one above the other.

use crate::render::Glyphs;
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

    /// Dots visible at 1:1 within the preview area.
    pub(crate) fn visible(&self, w: u32, h: u32, area: Rect) -> (u32, u32) {
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
    fn overview_fit_corrects_cell_aspect() {
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
}
