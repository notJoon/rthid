//! Floyd Steinberg kernel, divisor 16.

/// Taps whose weights sum to the divisor, so all of the error is spread.
pub(super) const FLOYD_STEINBERG: super::Weights = super::Weights {
    taps: &[(1, 0, 7), (-1, 1, 3), (0, 1, 5), (1, 1, 1)],
    divisor: 16,
};
