//! Checks that dithering hsl.jpg matches the golden output pixel for pixel.
//! Goldens (.idx) hold one palette index per pixel.
use std::sync::LazyLock;

use image::RgbaImage;
use rthid_dither::dither::{
    Algorithm, BayerSize, Diffusion, Kernel, Mode, Options, Tone, dither, make_palette,
};

const PALETTE: [&str; 4] = ["#ffffff", "#ee6a45", "#a3300f", "#1c0d07"];

static HSL: LazyLock<RgbaImage> =
    LazyLock::new(|| image::open("tests/fixtures/hsl.jpg").unwrap().to_rgba8());

fn check(name: &str, options: Options, golden: &[u8]) {
    let (w, h) = (HSL.width() as usize, HSL.height() as usize);
    let out = dither(HSL.as_raw(), w, h, &options);
    assert_eq!(out.len(), golden.len(), "{name}");
    let diff = out.iter().zip(golden).filter(|(a, b)| a != b).count();
    assert_eq!(diff, 0, "{name}: {diff} pixels differ from the golden");
}

fn tone_options(algorithm: Algorithm) -> Options {
    Options {
        palette: make_palette(&PALETTE).unwrap(),
        algorithm,
        ..Options::default()
    }
}

fn color_options(algorithm: Algorithm) -> Options {
    Options {
        mode: Mode::Color,
        tone: Tone {
            contrast: 0.15,
            white: 0.95,
            black: 0.04,
            sharpen: 0.6,
            ..Tone::default()
        },
        ..tone_options(algorithm)
    }
}

#[test]
fn hsl_tone_matches_golden() {
    let rows: [(&str, Algorithm, &[u8]); 9] = [
        (
            "atkinson",
            Algorithm::Diffusion(Diffusion {
                kernel: Kernel::Atkinson,
                ..Diffusion::default()
            }),
            include_bytes!("fixtures/hsl_tone.idx"),
        ),
        (
            "floyd_steinberg",
            Algorithm::Diffusion(Diffusion {
                kernel: Kernel::FloydSteinberg,
                ..Diffusion::default()
            }),
            include_bytes!("fixtures/hsl_tone_floyd_steinberg.idx"),
        ),
        (
            "jarvis_judice_ninke",
            Algorithm::Diffusion(Diffusion {
                kernel: Kernel::JarvisJudiceNinke,
                ..Diffusion::default()
            }),
            include_bytes!("fixtures/hsl_tone_jarvis_judice_ninke.idx"),
        ),
        (
            "stucki",
            Algorithm::Diffusion(Diffusion {
                kernel: Kernel::Stucki,
                ..Diffusion::default()
            }),
            include_bytes!("fixtures/hsl_tone_stucki.idx"),
        ),
        (
            "sierra_lite",
            Algorithm::Diffusion(Diffusion {
                kernel: Kernel::SierraLite,
                ..Diffusion::default()
            }),
            include_bytes!("fixtures/hsl_tone_sierra_lite.idx"),
        ),
        (
            "bayer2",
            Algorithm::Bayer(BayerSize::Two),
            include_bytes!("fixtures/hsl_tone_bayer2.idx"),
        ),
        (
            "bayer4",
            Algorithm::Bayer(BayerSize::Four),
            include_bytes!("fixtures/hsl_tone_bayer4.idx"),
        ),
        (
            "bayer8",
            Algorithm::Bayer(BayerSize::Eight),
            include_bytes!("fixtures/hsl_tone_bayer8.idx"),
        ),
        (
            "noise",
            Algorithm::Noise { seed: 0 },
            include_bytes!("fixtures/hsl_tone_noise.idx"),
        ),
    ];
    for (name, algorithm, golden) in rows {
        check(name, tone_options(algorithm), golden);
    }
}

#[test]
fn hsl_color_matches_golden() {
    let rows: [(&str, Algorithm, &[u8]); 9] = [
        (
            "atkinson",
            Algorithm::Diffusion(Diffusion {
                kernel: Kernel::Atkinson,
                ..Diffusion::default()
            }),
            include_bytes!("fixtures/hsl_color.idx"),
        ),
        (
            "floyd_steinberg",
            Algorithm::Diffusion(Diffusion {
                kernel: Kernel::FloydSteinberg,
                ..Diffusion::default()
            }),
            include_bytes!("fixtures/hsl_color_floyd_steinberg.idx"),
        ),
        (
            "jarvis_judice_ninke",
            Algorithm::Diffusion(Diffusion {
                kernel: Kernel::JarvisJudiceNinke,
                ..Diffusion::default()
            }),
            include_bytes!("fixtures/hsl_color_jarvis_judice_ninke.idx"),
        ),
        (
            "stucki",
            Algorithm::Diffusion(Diffusion {
                kernel: Kernel::Stucki,
                ..Diffusion::default()
            }),
            include_bytes!("fixtures/hsl_color_stucki.idx"),
        ),
        (
            "sierra_lite",
            Algorithm::Diffusion(Diffusion {
                kernel: Kernel::SierraLite,
                ..Diffusion::default()
            }),
            include_bytes!("fixtures/hsl_color_sierra_lite.idx"),
        ),
        (
            "bayer2",
            Algorithm::Bayer(BayerSize::Two),
            include_bytes!("fixtures/hsl_color_bayer2.idx"),
        ),
        (
            "bayer4",
            Algorithm::Bayer(BayerSize::Four),
            include_bytes!("fixtures/hsl_color_bayer4.idx"),
        ),
        (
            "bayer8",
            Algorithm::Bayer(BayerSize::Eight),
            include_bytes!("fixtures/hsl_color_bayer8.idx"),
        ),
        (
            "noise",
            Algorithm::Noise { seed: 0 },
            include_bytes!("fixtures/hsl_color_noise.idx"),
        ),
    ];
    for (name, algorithm, golden) in rows {
        check(name, color_options(algorithm), golden);
    }
}
