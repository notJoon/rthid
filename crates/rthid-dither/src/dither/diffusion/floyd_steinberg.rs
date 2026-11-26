//! Floyd Steinberg weights with divisor 16.

/// Neighbor weights and the fraction of error they receive.
pub(super) const FLOYD_STEINBERG: super::Weights = super::Weights {
    taps: &[(1, 0, 7), (-1, 1, 3), (0, 1, 5), (1, 1, 1)],
    divisor: 16,
};
