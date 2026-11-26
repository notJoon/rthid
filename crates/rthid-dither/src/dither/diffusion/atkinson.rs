//! Atkinson weights with divisor 8.

/// Neighbor weights and the fraction of error they receive. The divisor is
/// explicit because Atkinson spreads only 6/8 of the error.
pub(super) const ATKINSON: super::Weights = super::Weights {
    taps: &[
        (1, 0, 1),
        (2, 0, 1),
        (-1, 1, 1),
        (0, 1, 1),
        (1, 1, 1),
        (0, 2, 1),
    ],
    divisor: 8,
};
