//! Source decoding and protection from overwriting the source.

use image::{RgbaImage, imageops};
use std::{io, path::Path};

// Sources are shrunk on load to bound dither work per keypress.
// Offering a full resolution export would require keeping the unshrunk original.
const MAX_SOURCE_SIDE: u32 = 1024;

// Reject oversized sources before decoding can exhaust memory.
const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;
const MAX_DECODE_SIDE: u32 = 16384;

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
    if !path.metadata()?.is_file() {
        return Err(
            io::Error::new(io::ErrorKind::InvalidInput, "source is not a regular file").into(),
        );
    }
    let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits.max_image_width = Some(MAX_DECODE_SIDE);
    limits.max_image_height = Some(MAX_DECODE_SIDE);
    reader.limits(limits);
    let source = reader.decode()?;
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
    #[cfg(unix)]
    fn decoding_rejects_non_regular_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("socket");
        let _socket = std::os::unix::net::UnixListener::bind(&path).unwrap();
        for path in [path.as_path(), dir.path()] {
            let image::ImageError::IoError(error) = load_image(path).unwrap_err() else {
                panic!("expected a file type error");
            };
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
    }

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
