//! Error diffusion with selectable neighbor weights and scan direction.

mod atkinson;
mod floyd_steinberg;
mod jarvis_judice_ninke;
mod sierra_lite;
mod stucki;

use atkinson::ATKINSON;
use floyd_steinberg::FLOYD_STEINBERG;
use jarvis_judice_ninke::JARVIS_JUDICE_NINKE;
use sierra_lite::SIERRA_LITE;
use stucki::STUCKI;

use super::{Mode, nearest, tone_level};

/// Error diffusion settings. Defaults to serpentine Atkinson at strength 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Diffusion {
    /// Weights and neighbors that receive error.
    pub kernel: Kernel,
    /// Rows 1, 3, 5 (counting from 0) run right to left with taps mirrored.
    pub serpentine: bool,
    /// 1 diffuses the full kernel weights, spreading 3/4 of the error for Atkinson
    /// and all of it for other kernels. 0 is plain nearest color quantization.
    /// Dithering panics outside 0..=1 or for nonfinite values.
    pub strength: f64,
}

impl Default for Diffusion {
    fn default() -> Self {
        Self {
            kernel: Kernel::default(),
            serpentine: true,
            strength: 1.0,
        }
    }
}

/// Error diffusion weights. Atkinson is the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Kernel {
    /// Weight sum 6, divisor 8. Spreads 3/4 of the error.
    /// Choose for high contrast with clean highlights and shadows.
    #[default]
    Atkinson,
    /// Weight sum 16, divisor 16. Spreads all of the error.
    /// Choose for classic fine grain and speed.
    #[doc(alias = "Floyd-Steinberg")]
    FloydSteinberg,
    /// Weight sum 48, divisor 48. Spreads all of the error.
    /// Choose for smoother gradients with a wider spread at lower speed.
    #[doc(alias = "JJN")]
    #[doc(alias = "Jarvis")]
    JarvisJudiceNinke,
    /// Weight sum 42, divisor 42. Spreads all of the error.
    /// Choose for smoother gradients with a wider spread at lower speed.
    Stucki,
    /// Weight sum 4, divisor 4. Spreads all of the error.
    /// Choose for results close to Floyd Steinberg with fewer taps.
    #[doc(alias = "Sierra-2-4A")]
    SierraLite,
}

impl Kernel {
    fn weights(self) -> &'static Weights {
        match self {
            Self::Atkinson => &ATKINSON,
            Self::FloydSteinberg => &FLOYD_STEINBERG,
            Self::JarvisJudiceNinke => &JARVIS_JUDICE_NINKE,
            Self::Stucki => &STUCKI,
            Self::SierraLite => &SIERRA_LITE,
        }
    }
}

struct Weights {
    taps: &'static [(isize, usize, u8)], // (dx, dy, weight)
    divisor: u8,
}

/// Quantizes prepared channels and spreads error to later pixels.
#[allow(clippy::too_many_arguments)]
pub(super) fn diffuse(
    work: &mut [Vec<f32>],
    w: usize,
    h: usize,
    mode: Mode,
    levels: f64,
    colors: &[[f64; 3]],
    settings: &Diffusion,
    out: &mut [u8],
) {
    let weights = settings.kernel.weights();
    for y in 0..h {
        let reverse = settings.serpentine && y % 2 == 1;
        for k in 0..w {
            let x = if reverse { w - 1 - k } else { k };
            let i = y * w + x;
            let (q, target) = match mode {
                Mode::Tone => {
                    let v = f64::from(work[0][i]);
                    let q = tone_level(v, levels);
                    (q, [q as f64 / levels; 3])
                }
                Mode::Color => {
                    let px: [f64; 3] = std::array::from_fn(|c| f64::from(work[c][i]));
                    let q = nearest(px, colors);
                    (q, colors[q])
                }
            };
            out[i] = q as u8;
            for (channel, t) in work.iter_mut().zip(target) {
                let e = (f64::from(channel[i]) - t) * settings.strength;
                for &(dx, dy, weight) in weights.taps {
                    let (weight, divisor) = (f64::from(weight), f64::from(weights.divisor));
                    // The two forms differ in the last bit when the divisor is not a power of two.
                    let share = match mode {
                        Mode::Tone => e * weight / divisor,
                        Mode::Color => e * (weight / divisor),
                    };
                    let xx = x.checked_add_signed(if reverse { -dx } else { dx });
                    let yy = y + dy;
                    if let Some(xx) = xx.filter(|&xx| xx < w)
                        && yy < h
                    {
                        let j = yy * w + xx;
                        // Compute in f64, convert to f32 only on store.
                        channel[j] = (f64::from(channel[j]) + share) as f32;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Algorithm, Options, dither, make_palette};
    use super::*;

    const KERNELS: [Kernel; 5] = [
        Kernel::Atkinson,
        Kernel::FloydSteinberg,
        Kernel::JarvisJudiceNinke,
        Kernel::Stucki,
        Kernel::SierraLite,
    ];

    #[test]
    fn kernel_tables_preserve_error_fraction_and_bounds() {
        for kernel in KERNELS {
            let weights = kernel.weights();
            let sum = if kernel == Kernel::Atkinson {
                6
            } else {
                weights.divisor
            };
            assert_eq!(weights.taps.iter().map(|tap| tap.2).sum::<u8>(), sum);
            for &(dx, dy, _) in weights.taps {
                assert!((0..=2).contains(&dy));
                assert!(dx.abs() <= 2);
                assert!(dy != 0 || dx > 0);
            }
        }
    }

    #[test]
    fn serpentine_mirrors_the_second_row() {
        let w = 17;
        let row: Vec<_> = (0..w)
            .map(|x| {
                [
                    (x * 13 + 17) as u8,
                    (x * 7 + 3) as u8,
                    (x * 11 + 37) as u8,
                    255,
                ]
            })
            .collect();
        let rgba: Vec<_> = [vec![[0, 0, 0, 255]; w], row.clone()]
            .concat()
            .into_iter()
            .flatten()
            .collect();
        let mirrored: Vec<_> = [vec![[0, 0, 0, 255]; w], row.into_iter().rev().collect()]
            .concat()
            .into_iter()
            .flatten()
            .collect();
        for kernel in KERNELS {
            for mode in [Mode::Tone, Mode::Color] {
                let mut options = Options {
                    mode,
                    algorithm: Algorithm::Diffusion(Diffusion {
                        kernel,
                        serpentine: true,
                        strength: 1.0,
                    }),
                    ..Options::default()
                };
                let forward = dither(&rgba, w, 2, &options);
                options.algorithm = Algorithm::Diffusion(Diffusion {
                    kernel,
                    serpentine: false,
                    strength: 1.0,
                });
                let reverse = dither(&mirrored, w, 2, &options);
                assert_eq!(
                    &forward[w..],
                    reverse[w..].iter().rev().copied().collect::<Vec<_>>(),
                    "{kernel:?} {mode:?}"
                );
            }
        }
    }

    #[test]
    fn kernels_return_valid_indices_for_small_images() {
        let colors = ["#000", "#345", "#789", "#abc", "#fff"];
        for kernel in KERNELS {
            for mode in [Mode::Tone, Mode::Color] {
                for (w, h) in [(1, 1), (7, 1), (1, 7), (0, 0), (0, 3)] {
                    let rgba: Vec<_> = (0..w * h)
                        .flat_map(|i| [((i * 53 + 71) % 256) as u8, 137, 211, 255])
                        .collect();
                    for count in 1..=5 {
                        let options = Options {
                            palette: make_palette(&colors[..count]).unwrap(),
                            mode,
                            algorithm: Algorithm::Diffusion(Diffusion {
                                kernel,
                                serpentine: true,
                                strength: 1.0,
                            }),
                            ..Options::default()
                        };
                        let indices = dither(&rgba, w, h, &options);
                        assert_eq!(indices.len(), w * h);
                        assert!(
                            indices.iter().all(|&index| usize::from(index) < count),
                            "{kernel:?} {mode:?} {w}x{h} palette {count}"
                        );
                        if count == 1 {
                            assert_eq!(indices, vec![0; w * h]);
                        }
                    }
                }
            }
        }
    }
}
