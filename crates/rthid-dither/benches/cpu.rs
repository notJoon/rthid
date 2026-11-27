//! CPU benchmarks for dithering and mass-study operations.

use std::sync::OnceLock;

use divan::Bencher;
use rthid_dither::{
    dither::{Algorithm, BayerSize, Diffusion, Kernel, Mode, Options, dither},
    mass::{MassOptions, mass_study, rasterize},
};

const LONG_SIDE: u32 = 256;

fn input() -> &'static (Vec<u8>, usize, usize) {
    static INPUT: OnceLock<(Vec<u8>, usize, usize)> = OnceLock::new();
    INPUT.get_or_init(|| {
        let image = image::load_from_memory(include_bytes!("../tests/fixtures/pratt.png"))
            .unwrap()
            .thumbnail(LONG_SIDE, LONG_SIDE)
            .to_rgba8();
        let (w, h) = image.dimensions();
        (image.into_raw(), w as usize, h as usize)
    })
}

fn bench_dither(bencher: Bencher, options: Options) {
    let (rgba, w, h) = input();
    bencher.bench_local(|| dither(rgba, *w, *h, &options));
}

#[divan::bench(args = [
    Kernel::Atkinson,
    Kernel::FloydSteinberg,
    Kernel::JarvisJudiceNinke,
    Kernel::Stucki,
    Kernel::SierraLite,
])]
fn dither_diffusion(bencher: Bencher, kernel: Kernel) {
    let algorithm = Algorithm::Diffusion(Diffusion {
        kernel,
        ..Diffusion::default()
    });
    bench_dither(
        bencher,
        Options {
            algorithm,
            ..Options::default()
        },
    );
}

#[divan::bench(args = [BayerSize::Two, BayerSize::Four, BayerSize::Eight])]
fn dither_bayer(bencher: Bencher, size: BayerSize) {
    let algorithm = Algorithm::Bayer(size);
    bench_dither(
        bencher,
        Options {
            algorithm,
            ..Options::default()
        },
    );
}

#[divan::bench]
fn dither_noise(bencher: Bencher) {
    let algorithm = Algorithm::Noise { seed: 42 };
    bench_dither(
        bencher,
        Options {
            algorithm,
            ..Options::default()
        },
    );
}

#[divan::bench]
fn dither_atkinson_color(bencher: Bencher) {
    let mode = Mode::Color;
    bench_dither(
        bencher,
        Options {
            mode,
            ..Options::default()
        },
    );
}

#[divan::bench]
fn mass_study_analysis(bencher: Bencher) {
    let (rgba, w, h) = input();
    let options = MassOptions::default();
    bencher.bench_local(|| mass_study(rgba, *w, *h, &options).unwrap());
}

#[divan::bench]
fn mass_study_rings(bencher: Bencher) {
    let (rgba, w, h) = input();
    let study = mass_study(rgba, *w, *h, &MassOptions::default()).unwrap();
    bencher.bench_local(|| study.rings());
}

#[divan::bench]
fn mass_study_rasterize(bencher: Bencher) {
    let (rgba, w, h) = input();
    let study = mass_study(rgba, *w, *h, &MassOptions::default()).unwrap();
    let rings = study.rings();
    bencher.bench_local(|| rasterize(&rings, study.w(), study.h(), 1));
}

fn main() {
    divan::main();
}
