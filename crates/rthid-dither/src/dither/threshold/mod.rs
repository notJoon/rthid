//! Quantization with ordered or seeded thresholds, without changing channels.

pub(super) mod bayer;
pub(super) mod noise;

use super::{Mode, nearest};

/// Quantizes prepared channels in rows from left to right.
#[allow(clippy::too_many_arguments)]
pub(super) fn threshold(
    work: &[Vec<f32>],
    w: usize,
    h: usize,
    mode: Mode,
    levels: f64,
    colors: &[[f64; 3]],
    threshold: impl Fn(usize, usize) -> f64,
    out: &mut [u8],
) {
    let spread = 1.6 / f64::max(1.0, levels);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let t = threshold(x, y);
            let q = match mode {
                Mode::Tone => {
                    let v = f64::from(work[0][i]);
                    (v * levels + t).floor().clamp(0.0, levels) as usize
                }
                Mode::Color => {
                    let d = (t - 0.5) * spread;
                    nearest(std::array::from_fn(|c| f64::from(work[c][i]) + d), colors)
                }
            };
            out[i] = q as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Algorithm, BayerSize, Options, dither, make_palette};
    use super::*;

    const ALGORITHMS: [Algorithm; 4] = [
        Algorithm::Bayer(BayerSize::Two),
        Algorithm::Bayer(BayerSize::Four),
        Algorithm::Bayer(BayerSize::Eight),
        Algorithm::Noise { seed: 0 },
    ];

    #[test]
    fn thresholds_return_valid_indices_for_small_images() {
        let colors = ["#000", "#345", "#789", "#abc", "#fff"];
        for mut algorithm in ALGORITHMS {
            assert!(algorithm.diffusion().is_none());
            assert!(algorithm.diffusion_mut().is_none());
            for mode in [Mode::Tone, Mode::Color] {
                for (w, h) in [(1, 1), (7, 1), (1, 7), (0, 0), (0, 3)] {
                    let rgba: Vec<_> = (0..w * h)
                        .flat_map(|i| [((i * 53 + 71) % 256) as u8, 137, 211, 255])
                        .collect();
                    for count in 1..=5 {
                        let options = Options {
                            palette: make_palette(&colors[..count]).unwrap(),
                            mode,
                            algorithm,
                            ..Options::default()
                        };
                        let indices = dither(&rgba, w, h, &options);
                        assert_eq!(indices.len(), w * h);
                        assert!(
                            indices.iter().all(|&index| usize::from(index) < count),
                            "{algorithm:?} {mode:?} {w}x{h} palette {count}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn thresholds_repeat_and_noise_changes_with_seed() {
        let rgba: Vec<_> = (0..256)
            .flat_map(|i| [i as u8, i as u8, i as u8, 255])
            .collect();
        for mode in [Mode::Tone, Mode::Color] {
            for algorithm in ALGORITHMS {
                let mut options = Options {
                    mode,
                    algorithm,
                    ..Options::default()
                };
                let indices = dither(&rgba, 16, 16, &options);
                assert_eq!(indices, dither(&rgba, 16, 16, &options));
                if matches!(algorithm, Algorithm::Noise { .. }) {
                    options.algorithm = Algorithm::Noise { seed: 1 };
                    assert_ne!(indices, dither(&rgba, 16, 16, &options));
                }
            }
        }
    }
}
