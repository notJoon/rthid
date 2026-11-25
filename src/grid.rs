//! Source image, scaled grid and dithered indices caches.

use crate::{
    memo::Memo,
    settings::{Rotation, Settings},
};
use image::{RgbaImage, imageops};
use rthid_dither::dither::{self, Options};

/// Scaled size before rotation, plus the rotation applied after scaling.
#[derive(PartialEq)]
pub(crate) struct GridKey {
    width: u32,
    height: u32,
    rotation: Rotation,
}

/// Grid build version and the settings the indices were dithered with.
#[derive(PartialEq)]
pub(crate) struct DitherKey {
    grid: u64,
    options: Options,
}

/// Source image, grid (scaled then rotated) and dithered indices. Each stage
/// rebuilds only when its key changes.
pub struct GridCache {
    /// Loaded image before scaling and rotation.
    pub(crate) source: RgbaImage,
    /// Scaled and rotated image cache.
    pub(crate) grid: Memo<GridKey, RgbaImage>,
    /// Palette indices from the last requested dither.
    pub(crate) dithered: Memo<DitherKey, Vec<u8>>,
}

impl GridCache {
    /// Grid dimensions before rotation for the current scale.
    pub(crate) fn scaled_size(&self, settings: &Settings) -> (u32, u32) {
        let scaled = |n| {
            (f64::from(n) * f64::from(settings.scale) / 100.0)
                .round()
                .max(1.0) as u32
        };
        (scaled(self.source.width()), scaled(self.source.height()))
    }

    /// Rebuilds the grid when the scaled size or rotation changed and, if
    /// `need_dither` is set, the indices when the grid or dither options changed.
    /// Returns true when new indices were built.
    pub fn refresh(&mut self, settings: &Settings, need_dither: bool) -> bool {
        let (width, height) = self.scaled_size(settings);
        self.grid.update(
            GridKey {
                width,
                height,
                rotation: settings.rotation,
            },
            || {
                let scaled =
                    imageops::resize(&self.source, width, height, imageops::FilterType::Triangle);
                settings.rotation.apply(scaled)
            },
        );
        if !need_dither {
            return false;
        }
        let grid = self.grid.get().expect("processed grid");
        let (w, h) = grid.dimensions();
        let version = self.grid_version();
        self.dithered.update_by(
            |key| key.grid == version && key.options == settings.options,
            || DitherKey {
                grid: version,
                options: settings.options.clone(),
            },
            || dither::dither(grid.as_raw(), w as usize, h as usize, &settings.options),
        )
    }

    /// The scaled and rotated image that is shown as Original and dithered.
    pub fn grid(&self) -> &RgbaImage {
        self.grid.get().expect("processed grid")
    }

    /// Number of grid builds, so preview caches can tell when the grid changed.
    pub fn grid_version(&self) -> u64 {
        self.grid.version()
    }

    /// Indices from the last dither, `None` before the first. They match the
    /// current grid only after a refresh that requested dithering.
    pub fn dithered(&self) -> Option<&Vec<u8>> {
        self.dithered.get()
    }

    /// Loaded dimensions before scaling and rotation.
    pub fn source_size(&self) -> (u32, u32) {
        self.source.dimensions()
    }
}
