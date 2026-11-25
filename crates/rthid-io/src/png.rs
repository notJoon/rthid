//! PNG encoding for the dithered grid. Each dot becomes a solid square
//! and the file is replaced only after a complete encode.

use image::ImageEncoder;
use rthid_dither::dither::{self, PaletteColor};
use std::{io, path::Path};

/// Integer dot size from 1 to 4 that keeps the long side within 2048 px,
/// unless the grid is already larger.
pub fn preview_scale(w: u32, h: u32) -> u32 {
    2048u32.checked_div(w.max(h)).unwrap_or(1).clamp(1, 4)
}

/// Owned grid submitted for background PNG encoding.
pub struct Grid {
    /// One palette index per dot, row major.
    pub indices: Vec<u8>,
    /// Grid width in pixels.
    pub w: u32,
    /// Grid height in pixels.
    pub h: u32,
    /// Colors referenced by the indices.
    pub palette: Vec<PaletteColor>,
}

/// Writes the grid with each dot as a `scale` px solid square. Encodes to a temporary
/// file beside `path` and renames it over the destination only after encoding succeeds.
pub fn write_atomic(
    path: &Path,
    indices: &[u8],
    w: u32,
    h: u32,
    palette: &[PaletteColor],
    scale: u32,
) -> io::Result<()> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "invalid PNG grid or scale");
    let scaled = |n: u32| n.checked_mul(scale).filter(|&v| v > 0).ok_or_else(invalid);
    let ow = scaled(w)?;
    let oh = scaled(h)?;
    (ow as usize)
        .checked_mul(oh as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|&n| n <= isize::MAX as usize)
        .ok_or_else(invalid)?;
    if (w as usize).checked_mul(h as usize) != Some(indices.len())
        || indices.iter().any(|&i| usize::from(i) >= palette.len())
    {
        return Err(invalid());
    }
    let pixels = dither::rasterize(
        indices,
        w as usize,
        h as usize,
        ow as usize,
        oh as usize,
        |i| palette[usize::from(i)].rgb,
    );
    crate::atomic::replace(path, |file| {
        image::codecs::png::PngEncoder::new(file)
            .write_image(&pixels, ow, oh, image::ExtendedColorType::Rgba8)
            .map_err(io::Error::other)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rthid_dither::dither::make_palette;

    #[test]
    fn preview_scale_bounds_the_long_side_with_exact_integer_dots() {
        for (w, h, expected) in [
            (1, 1, 4),
            (100, 20, 4),
            (20, 1024, 2),
            (1200, 400, 1),
            (2048, 2048, 1),
            (4096, 1, 1),
            (0, 0, 1),
        ] {
            assert_eq!(preview_scale(w, h), expected, "{w}x{h}");
        }
    }

    #[test]
    fn atomic_png_replaces_and_preserves_destination_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        let path = dir.join("preview.png");
        let palette = make_palette(&["#000", "#fff"]).unwrap();
        std::fs::write(&path, b"old").unwrap();
        write_atomic(&path, &[0, 1], 2, 1, &palette, 4).unwrap();
        let image = image::open(&path).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (8, 4));
        assert_eq!(image.get_pixel(3, 3).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(4, 3).0, [255; 4]);
        let before = std::fs::read(&path).unwrap();
        for (indices, w, h, scale) in [
            (&[0, 2][..], 2, 1, 4),
            (&[0][..], 0, 1, 4),
            (&[0][..], 1, 1, 0),
            (&[0][..], 1, 1, u32::MAX),
        ] {
            assert!(write_atomic(&path, indices, w, h, &palette, scale).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        assert!(write_atomic(&dir, &[0], 1, 1, &palette, 1).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        write_atomic(&path, &[1], 1, 1, &palette, 1).unwrap();
        assert_eq!(image::open(&path).unwrap().width(), 1);
    }
}
