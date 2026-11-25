//! Terminal independent palette dithering, PNG export and atomic file replacement,
//! used by the rthid binary.

/// Palette parsing and Atkinson error diffusion into palette indices.
pub mod dither;
/// Atomic PNG export and background writing.
pub mod png;

/// Complete file replacement through a temporary file beside the destination.
pub mod atomic;
