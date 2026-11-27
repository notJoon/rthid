// The README doubles as the crate docs, so its examples run as doc tests.
#![doc = include_str!("../README.md")]

pub mod dither;
pub mod mass;

/// Invalid palette input from [`dither::make_palette`].
pub use dither::PaletteError;

/// Invalid analysis input from [`mass::mass_study`].
pub use mass::MassError;
