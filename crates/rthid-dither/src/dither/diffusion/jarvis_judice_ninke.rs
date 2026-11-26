//! Jarvis Judice Ninke kernel, divisor 48.

/// Taps whose weights sum to the divisor, so all of the error is spread.
pub(super) const JARVIS_JUDICE_NINKE: super::Weights = super::Weights {
    taps: &[
        (1, 0, 7),
        (2, 0, 5),
        (-2, 1, 3),
        (-1, 1, 5),
        (0, 1, 7),
        (1, 1, 5),
        (2, 1, 3),
        (-2, 2, 1),
        (-1, 2, 3),
        (0, 2, 5),
        (1, 2, 3),
        (2, 2, 1),
    ],
    divisor: 48,
};
