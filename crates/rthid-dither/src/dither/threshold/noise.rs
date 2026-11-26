//! Reproducible noise thresholds from SplitMix64.
//!
//! This is random threshold dithering, not blue noise and not cryptographic.
//! A seed is used instead of a random source because caching and tests need
//! repeatable output. The contract is that the same input, size, options and
//! seed give the same output. A different seed does not guarantee a different
//! image, for example a flat input can quantize the same way.
//!
//! Changing the mixing constants or the index mapping changes the output for
//! every seed, so it is a compatibility change.

/// Maps the indexed generator output to a threshold in [0, 1).
fn splitmix64(seed: u64, i: u64) -> f64 {
    let mut z = seed.wrapping_add(i.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// Prepares thresholds indexed by pixel position.
///
/// The generator is keyed by the row major index `i = y * width + x`, so a
/// value does not depend on how many numbers earlier pixels used. Width is
/// part of the index, so the pattern is not independent of image width.
pub(in crate::dither) fn threshold(seed: u64, w: usize) -> impl Fn(usize, usize) -> f64 {
    move |x, y| splitmix64(seed, (y * w + x) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_noise_is_reproducible() {
        assert_eq!(
            splitmix64(0, 0),
            (0xE220_A839_7B1D_CDAFu64 >> 11) as f64 / (1u64 << 53) as f64
        );
        for i in 0..256 {
            assert_eq!(splitmix64(42, i), splitmix64(42, i));
            assert_ne!(splitmix64(42, i), splitmix64(43, i));
            assert!((0.0..1.0).contains(&splitmix64(42, i)));
        }
    }
}
