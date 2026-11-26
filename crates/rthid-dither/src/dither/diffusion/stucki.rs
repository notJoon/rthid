//! Stucki weights with divisor 42.

/// Neighbor weights and the fraction of error they receive.
pub(super) const STUCKI: super::Weights = super::Weights {
    taps: &[
        (1, 0, 8),
        (2, 0, 4),
        (-2, 1, 2),
        (-1, 1, 4),
        (0, 1, 8),
        (1, 1, 4),
        (2, 1, 2),
        (-2, 2, 1),
        (-1, 2, 2),
        (0, 2, 4),
        (1, 2, 2),
        (2, 2, 1),
    ],
    divisor: 42,
};
