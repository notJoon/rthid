// Checks that dithering hsl.jpg matches the golden output pixel for pixel.
// Goldens (.idx) hold one palette index per pixel.
use std::sync::LazyLock;

use image::RgbaImage;
use rthid::dither::{Mode, Options, Tone, dither, make_palette};

const PALETTE: [&str; 4] = ["#ffffff", "#ee6a45", "#a3300f", "#1c0d07"];

static HSL: LazyLock<RgbaImage> =
    LazyLock::new(|| image::open("tests/fixtures/hsl.jpg").unwrap().to_rgba8());

fn check(options: Options, golden: &[u8]) {
    let (w, h) = (HSL.width() as usize, HSL.height() as usize);
    let out = dither(HSL.as_raw(), w, h, &options);
    assert_eq!(out.len(), golden.len());
    let diff = out.iter().zip(golden).filter(|(a, b)| a != b).count();
    assert_eq!(diff, 0, "{diff} pixels differ from the golden");
}

#[test]
fn hsl_tone_matches_golden() {
    let options = Options {
        palette: make_palette(&PALETTE).unwrap(),
        ..Options::default()
    };
    check(options, include_bytes!("fixtures/hsl_tone.idx"));
}

#[test]
fn hsl_color_matches_golden() {
    let options = Options {
        palette: make_palette(&PALETTE).unwrap(),
        mode: Mode::Color,
        tone: Tone {
            contrast: 0.15,
            white: 0.95,
            black: 0.04,
            sharpen: 0.6,
            ..Tone::default()
        },
        ..Options::default()
    };
    check(options, include_bytes!("fixtures/hsl_color.idx"));
}
