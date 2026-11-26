//! Checks that dithering hsl.jpg matches the golden output pixel for pixel.
//! Goldens (.idx) hold one palette index per pixel.
use std::sync::LazyLock;

use image::RgbaImage;
use rthid_dither::dither::{Algorithm, Kernel, Mode, Options, Tone, dither, make_palette};

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

fn tone_options(kernel: Kernel) -> Options {
    Options {
        palette: make_palette(&PALETTE).unwrap(),
        algorithm: Algorithm::Diffusion {
            kernel,
            serpentine: true,
            strength: 1.0,
        },
        ..Options::default()
    }
}

#[test]
fn hsl_tone_matches_golden() {
    check(
        tone_options(Kernel::Atkinson),
        include_bytes!("fixtures/hsl_tone.idx"),
    );
}

fn color_options(kernel: Kernel) -> Options {
    Options {
        mode: Mode::Color,
        tone: Tone {
            contrast: 0.15,
            white: 0.95,
            black: 0.04,
            sharpen: 0.6,
            ..Tone::default()
        },
        ..tone_options(kernel)
    }
}

#[test]
fn hsl_color_matches_golden() {
    check(
        color_options(Kernel::Atkinson),
        include_bytes!("fixtures/hsl_color.idx"),
    );
}

#[test]
fn hsl_tone_floyd_steinberg_matches_golden() {
    check(
        tone_options(Kernel::FloydSteinberg),
        include_bytes!("fixtures/hsl_tone_floyd_steinberg.idx"),
    );
}

#[test]
fn hsl_color_floyd_steinberg_matches_golden() {
    check(
        color_options(Kernel::FloydSteinberg),
        include_bytes!("fixtures/hsl_color_floyd_steinberg.idx"),
    );
}

#[test]
fn hsl_tone_jarvis_matches_golden() {
    check(
        tone_options(Kernel::JarvisJudiceNinke),
        include_bytes!("fixtures/hsl_tone_jarvis.idx"),
    );
}

#[test]
fn hsl_color_jarvis_matches_golden() {
    check(
        color_options(Kernel::JarvisJudiceNinke),
        include_bytes!("fixtures/hsl_color_jarvis.idx"),
    );
}

#[test]
fn hsl_tone_stucki_matches_golden() {
    check(
        tone_options(Kernel::Stucki),
        include_bytes!("fixtures/hsl_tone_stucki.idx"),
    );
}

#[test]
fn hsl_color_stucki_matches_golden() {
    check(
        color_options(Kernel::Stucki),
        include_bytes!("fixtures/hsl_color_stucki.idx"),
    );
}

#[test]
fn hsl_tone_sierra_lite_matches_golden() {
    check(
        tone_options(Kernel::SierraLite),
        include_bytes!("fixtures/hsl_tone_sierra_lite.idx"),
    );
}

#[test]
fn hsl_color_sierra_lite_matches_golden() {
    check(
        color_options(Kernel::SierraLite),
        include_bytes!("fixtures/hsl_color_sierra_lite.idx"),
    );
}
