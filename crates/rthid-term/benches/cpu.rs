//! CPU benchmark for full-frame terminal rendering.

use divan::Bencher;
use ratatui::{
    backend::Backend, buffer::Buffer, crossterm::style::Colored, layout::Rect, style::Color,
};
use rthid_term::ElidingBackend;

#[divan::bench]
fn draw_full_frame(bencher: Bencher) {
    let (width, height) = (200, 55);
    let mut frame = Buffer::empty(Rect::new(0, 0, width, height));
    for (i, cell) in frame.content.iter_mut().enumerate() {
        let rgb = Color::Rgb((i % 251) as u8, (i * 3 % 251) as u8, (i * 7 % 251) as u8);
        let indexed = Color::Indexed((i % 256) as u8);
        (cell.fg, cell.bg) = if i % 2 == 0 {
            (rgb, indexed)
        } else {
            (indexed, rgb)
        };
        if i % 7 == 0 {
            cell.set_symbol(" ");
            cell.fg = cell.bg;
        } else {
            cell.set_symbol("▀");
        }
    }
    // Every cell differs from an empty buffer, so this is the full frame.
    let updates = Buffer::empty(frame.area).diff(&frame);
    assert_eq!(updates.len(), frame.content.len());
    bencher
        .with_inputs(|| ElidingBackend::new(Vec::with_capacity(1024 * 1024)))
        .bench_local_refs(|backend| backend.draw(updates.iter().copied()).unwrap());
}

fn main() {
    // crossterm caches NO_COLOR process-wide; keep color escapes in the measured output.
    Colored::set_ansi_color_disabled(false);
    divan::main();
}
