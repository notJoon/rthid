use super::{MassStudy, components, touches_border};

/// One closed ring of cell corner vertices.
///
/// Outer rings run clockwise on screen (where y increases downward), while hole rings run
/// counterclockwise. The closing vertex is implicit: the last point connects to the first.
#[derive(Debug, Clone, PartialEq)]
pub struct Ring {
    /// Corner vertices, starting at the topmost-leftmost corner.
    pub points: Vec<[f64; 2]>,
    /// Whether this ring bounds a hole rather than a foreground component.
    pub hole: bool,
    /// Index of the enclosing ring in the same tier, if any.
    pub parent: Option<usize>,
}

impl MassStudy {
    /// Traces the base, shadow and accent masks into closed rings.
    ///
    /// Entry `k - 1` traces cells whose tone index is at least `k`. Foreground components
    /// use eight-connectivity and holes use four-connectivity, so rings may touch at a
    /// saddle corner but never cross.
    pub fn rings(&self) -> [Vec<Ring>; 3] {
        std::array::from_fn(|tier| {
            let mask: Vec<_> = self
                .indices
                .iter()
                .map(|&index| usize::from(index) > tier)
                .collect();
            trace_tier(&mask, self.w, self.h)
        })
    }
}

fn trace_tier(mask: &[bool], w: usize, h: usize) -> Vec<Ring> {
    // One label space for 8-connected foreground and 4-connected background components.
    let mut parts = components(mask, w, h, true, true);
    parts.extend(components(mask, w, h, false, false));
    let mut labels = vec![0; mask.len()];
    for (label, cells) in parts.iter().enumerate() {
        for &cell in cells {
            labels[cell] = label;
        }
    }

    // A ring starts at its component's first cell, so the cell above is always of the
    // other kind and its ring, if any, encloses this one. Exterior background has none.
    let mut ring_of = vec![None; parts.len()];
    let mut rings = Vec::new();
    for cell in 0..mask.len() {
        let (label, hole) = (labels[cell], !mask[cell]);
        let cells = &parts[label];
        if cells[0] != cell || hole && touches_border(cells, w, h) {
            continue;
        }
        let parent = cell.checked_sub(w).and_then(|above| ring_of[labels[above]]);
        ring_of[label] = Some(rings.len());
        rings.push(Ring {
            points: trace_ring(mask, w, h, cell, hole),
            hole,
            parent,
        });
    }
    rings
}

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    East,
    South,
    West,
    North,
}

impl Direction {
    fn left(self) -> Self {
        match self {
            Self::East => Self::North,
            Self::South => Self::East,
            Self::West => Self::South,
            Self::North => Self::West,
        }
    }

    fn right(self) -> Self {
        match self {
            Self::East => Self::South,
            Self::South => Self::West,
            Self::West => Self::North,
            Self::North => Self::East,
        }
    }

    fn step(self, x: &mut isize, y: &mut isize) {
        match self {
            Self::East => *x += 1,
            Self::South => *y += 1,
            Self::West => *x -= 1,
            Self::North => *y -= 1,
        }
    }
}

fn trace_ring(mask: &[bool], w: usize, h: usize, first: usize, hole: bool) -> Vec<[f64; 2]> {
    let start = ((first % w) as isize, (first / w) as isize);
    let start_direction = if hole {
        Direction::South
    } else {
        Direction::East
    };
    let (mut x, mut y) = start;
    let mut direction = start_direction;
    let mut points = vec![[x as f64, y as f64]];
    direction.step(&mut x, &mut y);

    loop {
        let (left, right) = adjacent_cells(x, y, direction);
        // Connectivity labels are unnecessary here: the 8/4 component pairing makes the
        // crack rule's target-label predicate equivalent to the plain foreground mask.
        let next = if cell(mask, w, h, left) {
            direction.left()
        } else if cell(mask, w, h, right) {
            direction
        } else {
            direction.right()
        };
        if (x, y) == start && next == start_direction {
            break;
        }
        if next != direction {
            points.push([x as f64, y as f64]);
        }
        next.step(&mut x, &mut y);
        direction = next;
    }
    points
}

fn adjacent_cells(x: isize, y: isize, direction: Direction) -> ((isize, isize), (isize, isize)) {
    match direction {
        Direction::East => ((x, y - 1), (x, y)),
        Direction::South => ((x, y), (x - 1, y)),
        Direction::West => ((x - 1, y), (x - 1, y - 1)),
        Direction::North => ((x - 1, y - 1), (x, y - 1)),
    }
}

fn cell(mask: &[bool], w: usize, h: usize, (x, y): (isize, isize)) -> bool {
    x >= 0 && y >= 0 && x < w as isize && y < h as isize && mask[y as usize * w + x as usize]
}

/// Rasterizes three contour tiers into row-major tone indices at an integer scale.
///
/// Each tier uses an even-odd fill and is intersected with the tier below it. A zero
/// dimension or scale returns an empty buffer.
///
/// # Panics
///
/// Panics if the scaled output dimensions or their product overflow `usize`.
#[track_caller]
pub fn rasterize(tiers: &[Vec<Ring>; 3], w: usize, h: usize, scale: usize) -> Vec<u8> {
    if w == 0 || h == 0 || scale == 0 {
        return Vec::new();
    }
    let Some(out_w) = w.checked_mul(scale) else {
        panic!("output width {w} * scale {scale} exceeds usize");
    };
    let Some(out_h) = h.checked_mul(scale) else {
        panic!("output height {h} * scale {scale} exceeds usize");
    };
    let Some(len) = out_w.checked_mul(out_h) else {
        panic!("output {out_w}x{out_h} exceeds {} bytes", usize::MAX);
    };
    let mut out = vec![0; len];
    let scale = scale as f64;

    let mut crossings = Vec::new();
    for (tier, rings) in tiers.iter().enumerate() {
        // Horizontal edges never cross a scanline under the half-open rule.
        let edges: Vec<[f64; 4]> = rings
            .iter()
            .flat_map(|ring| {
                let next = ring.points.iter().skip(1).chain(ring.points.first());
                ring.points.iter().zip(next)
            })
            .filter(|([_, y0], [_, y1])| y0 != y1)
            .map(|(&[x0, y0], &[x1, y1])| [x0 * scale, y0 * scale, x1 * scale, y1 * scale])
            .collect();
        for y in 0..out_h {
            let yc = y as f64 + 0.5;
            crossings.clear();
            for &[x0, y0, x1, y1] in &edges {
                if (y0 <= yc) != (y1 <= yc) {
                    crossings.push(x0 + (yc - y0) * (x1 - x0) / (y1 - y0));
                }
            }
            crossings.sort_by(f64::total_cmp);
            let (pairs, _) = crossings.as_chunks::<2>();
            for pair in pairs {
                let start = (pair[0] - 0.5).ceil().clamp(0.0, out_w as f64) as usize;
                let end = (pair[1] - 0.5).ceil().clamp(0.0, out_w as f64) as usize;
                // Sorted finite crossings keep start <= end; a NaN crossing can invert them.
                if start < end {
                    for pixel in &mut out[y * out_w + start..y * out_w + end] {
                        if *pixel == tier as u8 {
                            *pixel += 1;
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn study(rows: &[&str]) -> MassStudy {
        let w = rows.first().map_or(0, |row| row.len());
        assert!(rows.iter().all(|row| row.len() == w));
        MassStudy {
            indices: rows
                .iter()
                .flat_map(|row| row.bytes().map(|byte| byte - b'0'))
                .collect(),
            w,
            h: rows.len(),
        }
    }

    #[test]
    fn square_has_four_corners() {
        let rings = study(&["11", "11"]).rings();
        assert_eq!(
            rings[0],
            [Ring {
                points: vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]],
                hole: false,
                parent: None,
            }]
        );
    }

    #[test]
    fn hole_and_island_have_enclosing_parents() {
        let rings = study(&["11111", "10001", "10101", "10001", "11111"]).rings();
        assert_eq!(rings[0].len(), 3);
        assert_eq!((rings[0][1].hole, rings[0][1].parent), (true, Some(0)));
        assert_eq!((rings[0][2].hole, rings[0][2].parent), (false, Some(1)));
    }

    #[test]
    fn saddle_connectivity_matches_components() {
        let rings = study(&["10", "01"]).rings();
        assert_eq!(rings[0].len(), 1);
        assert_eq!(
            rings[0][0].points,
            [
                [0.0, 0.0],
                [1.0, 0.0],
                [1.0, 1.0],
                [2.0, 1.0],
                [2.0, 2.0],
                [1.0, 2.0],
                [1.0, 1.0],
                [0.0, 1.0],
            ]
        );

        let rings = study(&["1111", "1011", "1101", "1111"]).rings();
        assert_eq!(rings[0].iter().filter(|ring| ring.hole).count(), 2);
    }

    #[test]
    fn border_edges_are_traced_without_exterior_holes() {
        let study = study(&["110", "110", "001"]);
        let rings = study.rings();
        assert_eq!(rings[0].len(), 1);
        let points = &rings[0][0].points;
        assert!(points.iter().any(|point| point[0] == 0.0));
        assert!(points.iter().any(|point| point[1] == 0.0));
        assert!(points.iter().any(|point| point[0] == study.w as f64));
        assert!(points.iter().any(|point| point[1] == study.h as f64));
        assert!(!rings[0][0].hole);
    }

    fn assert_round_trip(rows: &[&str]) {
        let study = study(rows);
        let rings = study.rings();
        assert_eq!(rasterize(&rings, study.w, study.h, 1), study.indices);
        let scaled = rasterize(&rings, study.w, study.h, 3);
        for y in 0..study.h * 3 {
            for x in 0..study.w * 3 {
                assert_eq!(
                    scaled[y * study.w * 3 + x],
                    study.indices[y / 3 * study.w + x / 3]
                );
            }
        }
    }

    #[test]
    fn round_trips_tiers_holes_and_saddles() {
        assert_round_trip(&["0123"]);
        assert_round_trip(&["11111", "10001", "10101", "10001", "11111"]);
        assert_round_trip(&["1200", "2130", "0310", "0031"]);
    }

    #[test]
    fn every_binary_three_by_three_mask_round_trips() {
        for bits in 0..1 << 9 {
            let indices = (0..9).map(|bit| ((bits >> bit) & 1) as u8).collect();
            let study = MassStudy {
                indices,
                w: 3,
                h: 3,
            };
            assert_eq!(
                rasterize(&study.rings(), 3, 3, 1),
                study.indices,
                "mask {bits:#011b}"
            );
        }
    }

    #[test]
    fn non_finite_crossing_does_not_panic() {
        let tiers = [
            vec![Ring {
                points: vec![[f64::NAN, 0.0], [2.0, 0.0], [0.0, 2.0]],
                hole: false,
                parent: None,
            }],
            vec![],
            vec![],
        ];
        assert_eq!(rasterize(&tiers, 2, 2, 2).len(), 16);
    }
}
