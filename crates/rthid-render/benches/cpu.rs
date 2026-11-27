//! CPU benchmark for PNG rendering.

use divan::Bencher;
use rthid_dither::dither::make_palette;
use rthid_render::{Grid, write_png};

#[divan::bench]
fn write_png_to_vec(bencher: Bencher) {
    let (w, h) = (256, 256);
    let indices = (0..w * h).map(|i| (i % 4) as u8).collect();
    let palette = make_palette(&["#111", "#666", "#aaa", "#fff"]).unwrap();
    let grid = Grid::new(indices, w, h, palette).unwrap();
    bencher
        .with_inputs(|| Vec::with_capacity((w * h * 4) as usize))
        .bench_local_refs(|output| write_png(output, &grid, (w, h), None).unwrap());
}

fn main() {
    divan::main();
}
