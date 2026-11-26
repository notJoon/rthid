//! Quantization with ordered or seeded thresholds, without changing channels.

pub(super) mod bayer;
pub(super) mod noise;

use super::{Mode, nearest, tone_level};

/// Quantizes prepared channels in rows from left to right.
pub(super) fn threshold(
    work: &[Vec<f32>],
    w: usize,
    h: usize,
    mode: Mode,
    levels: f64,
    colors: &[[f64; 3]],
    threshold: impl Fn(usize, usize) -> f64,
) -> Vec<u8> {
    let mut out = vec![0; w * h];
    let spread = 1.6 / f64::max(1.0, levels);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let t = threshold(x, y);
            let q = match mode {
                Mode::Tone => {
                    let v = f64::from(work[0][i]);
                    tone_level(v, levels, t)
                }
                Mode::Color => {
                    let d = (t - 0.5) * spread;
                    nearest(std::array::from_fn(|c| f64::from(work[c][i]) + d), colors)
                }
            };
            out[i] = q as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::{Algorithm, BayerSize, Options, dither, tests::gradient};
    use super::*;

    const ALGORITHMS: [Algorithm; 4] = [
        Algorithm::Bayer(BayerSize::Two),
        Algorithm::Bayer(BayerSize::Four),
        Algorithm::Bayer(BayerSize::Eight),
        Algorithm::Noise { seed: 0 },
    ];

    #[test]
    fn thresholds_repeat_and_noise_changes_with_seed() {
        let rgba = gradient();
        for mode in [Mode::Tone, Mode::Color] {
            for algorithm in ALGORITHMS {
                let mut options = Options {
                    mode,
                    algorithm,
                    ..Options::default()
                };
                let indices = dither(&rgba, 16, 8, &options);
                assert_eq!(indices, dither(&rgba, 16, 8, &options));
                if matches!(algorithm, Algorithm::Noise { .. }) {
                    options.algorithm = Algorithm::Noise { seed: 1 };
                    assert_ne!(indices, dither(&rgba, 16, 8, &options));
                }
            }
        }
    }
}
