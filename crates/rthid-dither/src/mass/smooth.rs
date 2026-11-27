use super::{ANALYSIS_WIDTH, MassStudy, Ring};

/// A simplified ring whose corners are rounded by quadratic curves.
///
/// [`ring`](Self::ring) stores the curve control vertices, while [`radii`](Self::radii)
/// stores the distance trimmed from each adjacent edge. Independent simplification and
/// rounding can put vectors slightly outside the tier below; [`super::rasterize`] keeps the
/// raster tiers nested.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundedRing {
    /// The kept vertices and the original ring's hole and parent metadata.
    pub ring: Ring,
    /// Corner radii corresponding one-to-one with `ring.points`; zero means sharp.
    pub radii: Vec<f64>,
}

impl RoundedRing {
    /// Flattens the rounded path to a closed polyline suitable for rasterization.
    /// Its first point is the first corner's entry point, not necessarily the topmost-leftmost
    /// point promised by traced [`Ring`] values.
    ///
    /// # Panics
    ///
    /// Panics if `radii` does not contain one entry for every point in `ring`.
    pub fn flatten(&self) -> Ring {
        assert_eq!(self.radii.len(), self.ring.points.len());
        let mut points = Vec::new();
        for i in 0..self.ring.points.len() {
            let vertex = self.ring.points[i];
            let radius = self.radii[i];
            if radius == 0.0 {
                push_distinct(&mut points, vertex);
                continue;
            }
            let (entry, exit) = corner_points(&self.ring.points, i, radius);
            push_distinct(&mut points, entry);
            let segments = (radius.ceil() as usize).clamp(2, 8);
            for step in 1..=segments {
                let t = step as f64 / segments as f64;
                let u = 1.0 - t;
                push_distinct(
                    &mut points,
                    [
                        u * u * entry[0] + 2.0 * u * t * vertex[0] + t * t * exit[0],
                        u * u * entry[1] + 2.0 * u * t * vertex[1] + t * t * exit[1],
                    ],
                );
            }
        }
        Ring {
            points,
            hole: self.ring.hole,
            parent: self.ring.parent,
        }
    }
}

impl MassStudy {
    /// Simplifies and rounds the base, shadow and accent contour tiers.
    pub fn rounded_rings(&self) -> [Vec<RoundedRing>; 3] {
        let rings = self.rings();
        let scale = self.w as f64 / ANALYSIS_WIDTH as f64;
        std::array::from_fn(|tier| {
            round_tier(
                &rings[tier],
                [4.0, 4.0, 3.0][tier] * scale,
                [10.0, 10.0, 8.0][tier] * scale,
                self.w as f64,
                self.h as f64,
            )
        })
    }
}

fn round_tier(rings: &[Ring], epsilon: f64, cap: f64, w: f64, h: f64) -> Vec<RoundedRing> {
    // ponytail: traced rings never repeat a point consecutively, so a raw ring is its own
    // sharp flattening and is checked directly.
    let representatives: Vec<_> = rings
        .iter()
        .map(|ring| [ring.points[0][0] + 0.5, ring.points[0][1] + 0.5])
        .collect();
    let mut accepted: Vec<RoundedRing> = Vec::with_capacity(rings.len());
    let mut flattened: Vec<Ring> = Vec::with_capacity(rings.len());

    for (index, ring) in rings.iter().enumerate() {
        let simple = simplified(ring, epsilon);
        let radii = radii(&simple.points, cap, w, h);
        let half = radii.iter().map(|r| r / 2.0).collect();
        let candidates = [
            RoundedRing {
                ring: simple.clone(),
                radii,
            },
            RoundedRing {
                ring: simple.clone(),
                radii: half,
            },
            sharp(simple),
            sharp(simplified(ring, epsilon / 2.0)),
            sharp(ring.clone()),
        ];
        // Later raw rings count too, so an early simplification cannot block their raw fallback.
        let others = flattened.iter().chain(&rings[index + 1..]);
        let points = representatives[..index]
            .iter()
            .chain(&representatives[index + 1..]);
        let (candidate, flat) = candidates
            .into_iter()
            .find_map(|candidate| {
                let flat = candidate.flatten();
                let valid = !self_crosses(&flat.points)
                    && others.clone().zip(points.clone()).all(|(other, &point)| {
                        pair_valid(&flat.points, &ring.points, &other.points, point)
                    });
                valid.then_some((candidate, flat))
            })
            .expect("raw contour rings do not properly cross");
        accepted.push(candidate);
        flattened.push(flat);
    }
    accepted
}

fn sharp(ring: Ring) -> RoundedRing {
    RoundedRing {
        radii: vec![0.0; ring.points.len()],
        ring,
    }
}

fn simplified(ring: &Ring, epsilon: f64) -> Ring {
    let points = simplify_closed(&ring.points, epsilon);
    let points = if points.len() < 3 || area(&points) < 0.5 {
        ring.points.clone()
    } else {
        points
    };
    Ring {
        points,
        hole: ring.hole,
        parent: ring.parent,
    }
}

fn simplify_closed(points: &[[f64; 2]], epsilon: f64) -> Vec<[f64; 2]> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let a = points[0];
    let mut b = 1;
    for i in 2..points.len() {
        if distance2(a, points[i]) > distance2(a, points[b]) {
            b = i;
        }
    }
    let mut first = simplify_chain(&points[..=b], epsilon);
    let mut second = simplify_chain(&[&points[b..], &points[..1]].concat(), epsilon);
    first.pop();
    second.pop();
    first.extend(second);
    first
}

fn simplify_chain(points: &[[f64; 2]], epsilon: f64) -> Vec<[f64; 2]> {
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0, points.len() - 1)];
    while let Some((start, end)) = stack.pop() {
        let mut farthest = None;
        for i in start + 1..end {
            let distance = point_segment_distance(points[i], points[start], points[end]);
            if farthest.is_none_or(|(_, best)| distance > best) {
                farthest = Some((i, distance));
            }
        }
        if let Some((i, _)) = farthest.filter(|(_, distance)| *distance > epsilon) {
            keep[i] = true;
            stack.push((i, end));
            stack.push((start, i));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(&point, keep)| keep.then_some(point))
        .collect()
}

fn radii(points: &[[f64; 2]], cap: f64, w: f64, h: f64) -> Vec<f64> {
    (0..points.len())
        .map(|i| {
            let (prev, next) = neighbors(points, i);
            let point = points[i];
            let turn = cross(sub(point, prev), sub(next, point));
            if point[0] == 0.0 || point[0] == w || point[1] == 0.0 || point[1] == h || turn == 0.0 {
                0.0
            } else {
                cap.min(0.4 * distance(prev, point))
                    .min(0.4 * distance(point, next))
            }
        })
        .collect()
}

fn neighbors(points: &[[f64; 2]], i: usize) -> ([f64; 2], [f64; 2]) {
    let n = points.len();
    (points[(i + n - 1) % n], points[(i + 1) % n])
}

fn corner_points(points: &[[f64; 2]], i: usize, radius: f64) -> ([f64; 2], [f64; 2]) {
    let (prev, next) = neighbors(points, i);
    (
        toward(points[i], prev, radius),
        toward(points[i], next, radius),
    )
}

fn toward(from: [f64; 2], to: [f64; 2], length: f64) -> [f64; 2] {
    let scale = length / distance(from, to);
    [
        from[0] + (to[0] - from[0]) * scale,
        from[1] + (to[1] - from[1]) * scale,
    ]
}

fn push_distinct(points: &mut Vec<[f64; 2]>, point: [f64; 2]) {
    if points.last() != Some(&point) {
        points.push(point);
    }
}

fn self_crosses(points: &[[f64; 2]]) -> bool {
    let n = points.len();
    crosses(points, points, |i, j| j <= i + 1 || i == 0 && j == n - 1)
}

fn rings_cross(a: &[[f64; 2]], b: &[[f64; 2]]) -> bool {
    crosses(a, b, |_, _| false)
}

fn crosses(a: &[[f64; 2]], b: &[[f64; 2]], skip: impl Fn(usize, usize) -> bool) -> bool {
    // ponytail: O(n^2) over a tier's segments is fine at <=450 columns; use a grid or
    // sweep if analysis grids grow substantially.
    (0..a.len()).any(|i| {
        (0..b.len()).any(|j| {
            !skip(i, j)
                && (segments_cross(a[i], a[(i + 1) % a.len()], b[j], b[(j + 1) % b.len()])
                    || a[i] == b[j] && edges_interleave(a, i, b, j))
        })
    })
}

fn segments_cross(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    if a[0].max(b[0]) < c[0].min(d[0])
        || c[0].max(d[0]) < a[0].min(b[0])
        || a[1].max(b[1]) < c[1].min(d[1])
        || c[1].max(d[1]) < a[1].min(b[1])
    {
        return false;
    }
    let [abc, abd, cda, cdb] = [
        cross(sub(b, a), sub(c, a)),
        cross(sub(b, a), sub(d, a)),
        cross(sub(d, c), sub(a, c)),
        cross(sub(d, c), sub(b, c)),
    ];
    if opposite(abc, abd) && opposite(cda, cdb) {
        return true;
    }
    if abc == 0.0 && abd == 0.0 {
        let (a0, a1, c0, c1) = if (b[0] - a[0]).abs() >= (b[1] - a[1]).abs() {
            (a[0], b[0], c[0], d[0])
        } else {
            (a[1], b[1], c[1], d[1])
        };
        return a0.min(a1).max(c0.min(c1)) < a0.max(a1).min(c0.max(c1));
    }
    touches(abc, c, a, b) || touches(abd, d, a, b) || touches(cda, a, c, d) || touches(cdb, b, c, d)
}

/// Whether `point`, at signed side `side` of segment `a`-`b`, lies strictly inside it.
fn touches(side: f64, point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> bool {
    side == 0.0 && on_segment(point, a, b) && point != a && point != b
}

fn pair_valid(
    candidate: &[[f64; 2]],
    raw: &[[f64; 2]],
    other: &[[f64; 2]],
    point: [f64; 2],
) -> bool {
    let Some(raw_inside) = point_inside(raw, point) else {
        return false;
    };
    !rings_cross(candidate, other) && point_inside(candidate, point) == Some(raw_inside)
}

fn point_inside(points: &[[f64; 2]], point: [f64; 2]) -> Option<bool> {
    let mut winding = 0;
    for (&a, &b) in points.iter().zip(points.iter().cycle().skip(1)) {
        let side = cross(sub(b, a), sub(point, a));
        if side == 0.0 && on_segment(point, a, b) {
            return None;
        }
        if a[1] <= point[1] && b[1] > point[1] && side > 0.0 {
            winding += 1;
        } else if a[1] > point[1] && b[1] <= point[1] && side < 0.0 {
            winding -= 1;
        }
    }
    Some(winding != 0)
}

fn on_segment(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> bool {
    point[0] >= a[0].min(b[0])
        && point[0] <= a[0].max(b[0])
        && point[1] >= a[1].min(b[1])
        && point[1] <= a[1].max(b[1])
}

fn edges_interleave(a: &[[f64; 2]], i: usize, b: &[[f64; 2]], j: usize) -> bool {
    let point = a[i];
    let angle = |to: [f64; 2]| (to[1] - point[1]).atan2(to[0] - point[0]);
    let ((a_prev, a_next), (b_prev, b_next)) = (neighbors(a, i), neighbors(b, j));
    let mut rays = [
        (angle(a_prev), 0),
        (angle(a_next), 0),
        (angle(b_prev), 1),
        (angle(b_next), 1),
    ];
    rays.sort_by(|a, b| a.0.total_cmp(&b.0));
    rays[0].1 == rays[2].1 && rays[1].1 == rays[3].1
}

fn opposite(a: f64, b: f64) -> bool {
    a < 0.0 && b > 0.0 || a > 0.0 && b < 0.0
}

fn area(points: &[[f64; 2]]) -> f64 {
    points
        .iter()
        .zip(points.iter().cycle().skip(1))
        .map(|(a, b)| a[0] * b[1] - b[0] * a[1])
        .sum::<f64>()
        .abs()
        / 2.0
}

fn point_segment_distance(point: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = sub(b, a);
    let length2 = distance2(a, b);
    if length2 == 0.0 {
        return distance(point, a);
    }
    let ap = sub(point, a);
    let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / length2).clamp(0.0, 1.0);
    distance(point, [a[0] + t * ab[0], a[1] + t * ab[1]])
}

fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    distance2(a, b).sqrt()
}

fn distance2(a: [f64; 2], b: [f64; 2]) -> f64 {
    let delta = sub(a, b);
    delta[0] * delta[0] + delta[1] * delta[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mass::rasterize;

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

    fn ring(points: &[[f64; 2]]) -> Ring {
        Ring {
            points: points.to_vec(),
            hole: false,
            parent: None,
        }
    }

    fn flattened(tiers: &[Vec<RoundedRing>; 3]) -> [Vec<Ring>; 3] {
        std::array::from_fn(|tier| tiers[tier].iter().map(RoundedRing::flatten).collect())
    }

    fn assert_no_crossings(tiers: &[Vec<Ring>; 3]) {
        for flat in tiers {
            for (i, ring) in flat.iter().enumerate() {
                assert!(!self_crosses(&ring.points));
                for other in &flat[..i] {
                    assert!(!rings_cross(&ring.points, &other.points));
                }
            }
        }
    }

    fn winding(points: &[[f64; 2]], point: [f64; 2]) -> i32 {
        points
            .iter()
            .zip(points.iter().cycle().skip(1))
            .fold(0, |winding, (&a, &b)| {
                let side = (b[0] - a[0]) * (point[1] - a[1]) - (point[0] - a[0]) * (b[1] - a[1]);
                if a[1] <= point[1] && b[1] > point[1] && side > 0.0 {
                    winding + 1
                } else if a[1] > point[1] && b[1] <= point[1] && side < 0.0 {
                    winding - 1
                } else {
                    winding
                }
            })
    }

    fn assert_valid_winding(tiers: &[Vec<Ring>; 3], w: usize, h: usize) {
        let offsets = [0.137, 0.428, 0.719];
        for flat in tiers {
            for y in 0..h {
                for x in 0..w {
                    for &dy in &offsets {
                        for &dx in &offsets {
                            let point = [x as f64 + dx, y as f64 + dy];
                            let total: i32 =
                                flat.iter().map(|ring| winding(&ring.points, point)).sum();
                            assert!(total == 0 || total == 1, "winding {total} at {point:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn square_simplifies_and_rounds_unless_it_touches_the_border() {
        let square = ring(&[[5.0, 5.0], [15.0, 5.0], [15.0, 15.0], [5.0, 15.0]]);
        let rounded = round_tier(&[square], 1.0, 10.0, 20.0, 20.0);
        assert_eq!(rounded[0].ring.points.len(), 4);
        assert_eq!(rounded[0].radii, [4.0; 4]);

        let border = ring(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
        let rounded = round_tier(&[border], 1.0, 10.0, 10.0, 10.0);
        assert_eq!(rounded[0].radii, [0.0; 4]);
    }

    #[test]
    fn staircase_disc_simplifies_without_crossing() {
        let size = 21;
        let center = 10_i32;
        let indices = (0..size)
            .flat_map(|y| {
                (0..size).map(move |x| {
                    let dx = x as i32 - center;
                    let dy = y as i32 - center;
                    u8::from(dx * dx + dy * dy <= 8 * 8)
                })
            })
            .collect();
        let study = MassStudy {
            indices,
            w: size,
            h: size,
        };
        let raw = study.rings();
        let rounded = round_tier(&raw[0], 1.0, 4.0, size as f64, size as f64);
        assert!(rounded[0].ring.points.len() * 2 < raw[0][0].points.len());
        assert!(!self_crosses(&rounded[0].flatten().points));
    }

    #[test]
    fn tiny_components_keep_their_raw_corners() {
        for rows in [&["1"][..], &["11"][..]] {
            let study = study(rows);
            let raw = study.rings();
            let rounded = round_tier(&raw[0], 10.0, 4.0, study.w as f64, study.h as f64);
            assert_eq!(rounded[0].ring.points, raw[0][0].points);
        }
    }

    #[test]
    fn vertex_on_another_segment_is_a_conflict() {
        assert!(segments_cross(
            [0.0, 0.0],
            [4.0, 4.0],
            [0.0, 4.0],
            [2.0, 2.0]
        ));
    }

    #[test]
    fn random_tiers_are_deterministic_nested_and_non_crossing() {
        let (w, h) = (30, 20);
        let mut state = 0x9e37_79b9_u32;
        for _ in 0..300 {
            let indices = (0..w * h)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    (state >> 30) as u8
                })
                .collect();
            let study = MassStudy { indices, w, h };
            let raw = study.rings();
            let make =
                || std::array::from_fn(|tier| round_tier(&raw[tier], 2.0, 4.0, w as f64, h as f64));
            let first = make();
            assert_eq!(first, make());
            let flat = flattened(&first);
            assert_no_crossings(&flat);
            assert_valid_winding(&flat, w, h);
            let raster = rasterize(&flat, w, h, 1);
            assert!(raster.iter().all(|&index| index <= 3));
            let masks: [Vec<_>; 3] = std::array::from_fn(|tier| {
                raster
                    .iter()
                    .map(|&cell| usize::from(cell) > tier)
                    .collect()
            });
            for tier in 1..3 {
                assert!(
                    masks[tier]
                        .iter()
                        .zip(&masks[tier - 1])
                        .all(|(&upper, &lower)| !upper || lower)
                );
            }
        }
    }

    #[test]
    fn rounded_rings_with_tiny_parameters_round_trip() {
        let study = study(&["01230", "12321", "23012", "10101"]);
        let raw = study.rings();
        let rounded: [Vec<_>; 3] = std::array::from_fn(|tier| {
            round_tier(
                &raw[tier],
                f64::EPSILON,
                f64::EPSILON,
                study.w as f64,
                study.h as f64,
            )
        });
        assert_eq!(
            rasterize(&flattened(&rounded), study.w, study.h, 1),
            study.indices
        );

        let public = study.rounded_rings();
        assert_eq!(public, study.rounded_rings());
        assert_eq!(
            rasterize(&flattened(&public), study.w, study.h, 1),
            study.indices
        );
    }
}
