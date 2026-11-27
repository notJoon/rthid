//! Fixture agreement for cumulative graphic mass masks.

use rthid_dither::mass::{MassOptions, MassStudy, mass_study};

fn tone_counts(study: &MassStudy) -> [usize; 4] {
    let mut counts = [0; 4];
    for &index in study.indices() {
        counts[index as usize] += 1;
    }
    counts
}

fn connected_components(mask: &[bool], w: usize, h: usize) -> Vec<Vec<usize>> {
    let mut seen = vec![false; mask.len()];
    let mut stack = Vec::new();
    let mut regions = Vec::new();
    for first in 0..mask.len() {
        if !mask[first] || seen[first] {
            continue;
        }
        let mut cells = Vec::new();
        seen[first] = true;
        stack.push(first);
        while let Some(p) = stack.pop() {
            cells.push(p);
            let (x, y) = (p % w, p / w);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let q = ny * w + nx;
                    if mask[q] && !seen[q] {
                        seen[q] = true;
                        stack.push(q);
                    }
                }
            }
        }
        cells.sort_unstable();
        regions.push(cells);
    }
    regions
}

#[test]
fn pratt_mass_masks() {
    let portrait = image::open("tests/fixtures/pratt.png").unwrap().to_rgba8();
    assert_eq!(portrait.dimensions(), (1181, 2126));
    let source = image::imageops::rotate270(&portrait);
    assert_eq!(source.dimensions(), (2126, 1181));
    let study = mass_study(
        source.as_raw(),
        source.width() as usize,
        source.height() as usize,
        &MassOptions {
            ellipses: false,
            ..MassOptions::default()
        },
    )
    .unwrap();
    assert_eq!((study.w(), study.h()), (450, 250));
    assert_eq!(
        tone_counts(&study).iter().sum::<usize>(),
        study.w() * study.h()
    );
    let reference = image::open("tests/fixtures/pratt_mass_masks.png")
        .unwrap()
        .to_rgb8();
    assert_eq!(reference.dimensions(), (450, 250));
    let mut ious = [0.0; 3];
    let mut counts = [0; 3];
    let mut reference_counts = [0; 3];
    let mut components = [0; 3];
    let mut reference_components = [0; 3];
    let masks: Vec<Vec<bool>> = (1..=3)
        .map(|i| study.indices().iter().map(|&v| v >= i).collect())
        .collect();
    for pair in masks.windows(2) {
        assert!(
            pair[1]
                .iter()
                .zip(&pair[0])
                .all(|(&child, &parent)| !child || parent)
        );
    }
    for i in 0..3 {
        let expected: Vec<bool> = reference.pixels().map(|p| p[i] >= 128).collect();
        let mut intersection = 0;
        let mut union = 0;
        for (&actual, &expected) in masks[i].iter().zip(&expected) {
            intersection += usize::from(actual && expected);
            union += usize::from(actual || expected);
            counts[i] += usize::from(actual);
            reference_counts[i] += usize::from(expected);
        }
        ious[i] = intersection as f64 / union as f64;
        components[i] = connected_components(&masks[i], study.w(), study.h()).len();
        reference_components[i] = connected_components(&expected, study.w(), study.h()).len();
    }
    println!("IoU: {ious:?}, cells: {counts:?}, components: {components:?}");
    println!("Reference cells: {reference_counts:?}, components: {reference_components:?}");
    assert_eq!(reference_counts, [45371, 20529, 9926]);
    assert_eq!(reference_components, [1, 3, 5]);
    assert_eq!(components[0], 1);
    for (iou, gate) in ious.into_iter().zip([0.95, 0.95, 0.90]) {
        assert!(iou >= gate, "IoU {iou} below {gate}");
    }
    let fitted = mass_study(
        source.as_raw(),
        source.width() as usize,
        source.height() as usize,
        &MassOptions::default(),
    )
    .unwrap();
    let mut accent = Vec::new();
    let (mut intersection, mut union) = (0, 0);
    for (p, (&before, &after)) in study.indices().iter().zip(fitted.indices()).enumerate() {
        assert_eq!(before >= 1, after >= 1);
        assert_eq!(before >= 2, after >= 2);
        let cell = after == 3;
        accent.push(cell);
        let expected = reference.as_raw()[p * 3 + 2] >= 128;
        intersection += usize::from(cell && expected);
        union += usize::from(cell || expected);
    }
    let accent_iou = intersection as f64 / union as f64;
    assert!(accent_iou >= 0.85, "Accent IoU {accent_iou} below 0.85");
    let before = connected_components(&masks[2], study.w(), study.h());
    let after = connected_components(&accent, study.w(), study.h());
    assert!(after.len() <= before.len());
    let mut changed = 0;
    for original in &before {
        let mut fitted_cells = Vec::new();
        for region in &after {
            let mut overlaps = false;
            for p in region {
                if original.binary_search(p).is_ok() {
                    overlaps = true;
                    break;
                }
            }
            if overlaps {
                fitted_cells.extend_from_slice(region);
            }
        }
        fitted_cells.sort_unstable();
        changed += usize::from(fitted_cells != *original);
    }
    println!(
        "Accent components before: {}, after: {}, changed: {changed}, IoU: {accent_iou}",
        before.len(),
        after.len()
    );
    assert!((2..=4).contains(&changed), "Changed components: {changed}");
}
