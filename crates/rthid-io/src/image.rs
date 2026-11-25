//! Source decoding and protection from overwriting the source.

use image::{RgbaImage, imageops};
use std::{io, path::Path};

// Sources are shrunk on load to bound dither work per keypress.
// Offering a full resolution export would require keeping the unshrunk original.
const MAX_SOURCE_SIDE: u32 = 1024;

/// Rejects a companion PNG destination that resolves to the source image.
pub fn check_not_source(source: &Path, preview: Option<&Path>) -> io::Result<()> {
    if let Some(preview) = preview
        && let (Ok(a), Ok(b)) = (source.canonicalize(), preview.canonicalize())
        && a == b
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--preview-png must not point at the source image",
        ));
    }
    Ok(())
}

/// Loads an image as RGBA and shrinks it to bound processing work.
pub fn load_image(path: &Path) -> Result<RgbaImage, image::ImageError> {
    let source = image::open(path)?;
    Ok(if source.width().max(source.height()) > MAX_SOURCE_SIDE {
        source.resize(
            MAX_SOURCE_SIDE,
            MAX_SOURCE_SIDE,
            imageops::FilterType::Triangle,
        )
    } else {
        source
    }
    .to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_png_may_not_overwrite_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("photo.png");
        std::fs::write(&source, b"image").unwrap();
        let aliased = dir.path().join(".").join("photo.png");
        assert!(check_not_source(&source, Some(&aliased)).is_err());
        assert!(check_not_source(&source, Some(&dir.path().join("dots.png"))).is_ok());
        assert!(check_not_source(&source, None).is_ok());
    }
}
