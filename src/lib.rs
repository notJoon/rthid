//! Terminal independent image processing (palette dithering and PNG export)
//! used by the rthid binary and its golden tests.

/// Palette parsing and Atkinson error diffusion into palette indices.
pub mod dither;
/// Atomic PNG export and background writing.
pub mod png;

/// Complete file replacement through a temporary file beside the destination.
pub mod atomic;
