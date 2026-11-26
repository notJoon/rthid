//! Sierra Lite kernel, divisor 4.

/// Taps whose weights sum to the divisor, so all of the error is spread.
pub(super) const SIERRA_LITE: super::Weights = super::Weights {
    taps: &[(1, 0, 2), (-1, 1, 1), (0, 1, 1)],
    divisor: 4,
};
