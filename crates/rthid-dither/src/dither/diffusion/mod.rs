//! Error diffusion with selectable neighbor weights and scan direction.
//!
//! Each kernel is data: a list of `(dx, dy, weight)` taps and an explicit
//! divisor, kept in its own file. All kernels run through the one shared
//! loop in `diffuse`, so traversal, quantization and numeric behavior stay
//! identical across them.
//!
//! Adding a kernel means adding a table file, a `Kernel` variant with its
//! entry in `Kernel::weights`, and contract and golden tests. The loop is
//! never copied. There is no public kernel trait or registration on purpose,
//! the supported set is a closed enum.

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
    /// Rows 1, 3, 5 (counting from 0) run right to left and every tap `dx`
    /// is mirrored. Without the mirror, error would go to pixels already done.
    pub serpentine: bool,
    /// Scales the error before it is spread. 1 uses the kernel's own weights,
    /// so Atkinson still spreads 3/4 of the error. 0 is plain quantization
    /// with no diffusion. Dithering panics outside 0..=1 or for nonfinite values.
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
///
/// The fractions below hold for interior pixels. Taps that fall outside
/// the image are dropped, not redistributed to the remaining taps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Kernel {
    /// Six taps, weight sum 6, divisor 8. Spreads 3/4 of the error.
    #[default]
    Atkinson,
    /// Four taps, weight sum 16, divisor 16. Spreads all of the error.
    #[doc(alias = "Floyd-Steinberg")]
    FloydSteinberg,
    /// Twelve taps, weight sum 48, divisor 48. Spreads all of the error.
    #[doc(alias = "JJN")]
    #[doc(alias = "Jarvis")]
    JarvisJudiceNinke,
    /// Twelve taps, weight sum 42, divisor 42. Spreads all of the error.
    Stucki,
    /// Three taps, weight sum 4, divisor 4. Spreads all of the error.
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

/// Taps relative to the current pixel and the divisor applied to each weight.
/// The divisor is explicit instead of derived from the weight sum because
/// Atkinson spreads 6/8 on purpose. Out of image taps are dropped, so the
/// spread fraction applies to interior pixels only.
struct Weights {
    taps: &'static [(isize, usize, u8)], // (dx, dy, weight)
    divisor: u8,
}

/// Quantizes prepared channels and spreads error to later pixels.
///
/// Error only reaches pixels not yet processed, so the result depends on the
/// scan order. Tiling or parallel traversal would change the output.
/// Channels are stored as f32, the math runs in f64, and each write is
/// stored back to f32 immediately. That rounding point is part of the output.
pub(super) fn diffuse(
    work: &mut [Vec<f32>],
    w: usize,
    h: usize,
    mode: Mode,
    levels: f64,
    colors: &[[f64; 3]],
    settings: &Diffusion,
) -> Vec<u8> {
    let mut out = vec![0; w * h];
    let weights = settings.kernel.weights();
    for y in 0..h {
        let reverse = settings.serpentine && y % 2 == 1;
        for k in 0..w {
            let x = if reverse { w - 1 - k } else { k };
            let i = y * w + x;
            let (q, target) = match mode {
                Mode::Tone => {
                    let v = f64::from(work[0][i]);
                    let q = tone_level(v, levels, 0.5);
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
                    // Kept apart on purpose. The two forms differ in the last bit when
                    // the divisor is not a power of two, and that bit can flip a later
                    // quantization. Goldens guard both forms, so do not merge them.
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
                        // f64 math, rounded to f32 on every store.
                        channel[j] = (f64::from(channel[j]) + share) as f32;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::{Algorithm, Options, dither};
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
                        ..Diffusion::default()
                    }),
                    ..Options::default()
                };
                let forward = dither(&rgba, w, 2, &options);
                options.algorithm = Algorithm::Diffusion(Diffusion {
                    kernel,
                    serpentine: false,
                    ..Diffusion::default()
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
}
