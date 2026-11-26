//! Stucki kernel, divisor 42.

/// Taps whose weights sum to the divisor, so all of the error is spread.
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
