//! Jarvis Judice Ninke weights with divisor 48.

/// Neighbor weights and the fraction of error they receive.
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
