//! Atkinson kernel, divisor 8.

/// Six taps of weight 1 over divisor 8, so 6/8 of the error is spread on purpose.
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
