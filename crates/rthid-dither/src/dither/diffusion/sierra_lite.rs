//! Sierra Lite weights with divisor 4.

/// Neighbor weights and the fraction of error they receive.
pub(super) const SIERRA_LITE: super::Weights = super::Weights {
    taps: &[(1, 0, 2), (-1, 1, 1), (0, 1, 1)],
    divisor: 4,
};
