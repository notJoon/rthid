//! Source decoding and protection from overwriting the source.

use image::{RgbaImage, imageops};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufReader},
    path::Path,
};

// Sources are shrunk on load to bound dither work per keypress.
// Offering a full resolution export would require keeping the unshrunk original.
const MAX_SOURCE_SIDE: u32 = 1024;

// Reject oversized sources before decoding can exhaust memory.
const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;
const MAX_DECODE_SIDE: u32 = 16384;

/// Rejects a companion PNG destination that resolves to the source image.
pub fn check_not_source(source: &Path, preview: Option<&Path>) -> Result<(), crate::io::Cause> {
    if let Some(preview) = preview
        && let (Ok(a), Ok(b)) = (source.canonicalize(), preview.canonicalize())
        && a == b
    {
        return Err(crate::io::Cause::IsSource);
    }
    Ok(())
}

/// Loads preview RGBA pixels, shrinking images whose longest side exceeds 1024 pixels.
///
/// Resizing preserves the aspect ratio and uses the Triangle filter. The result does not retain
/// the original dimensions or full-resolution pixels; use an image decoder directly for those.
/// Decoding rejects dimensions above 16384 pixels per side and requests a 256 MiB allocation
/// limit from the decoder. Only regular files are accepted; Unix FIFOs are opened without blocking.
pub fn load_image(path: &Path) -> Result<RgbaImage, image::ImageError> {
    let file = open_regular(path)?;
    let mut reader = image::ImageReader::new(BufReader::new(file));
    if let Ok(format) = image::ImageFormat::from_path(path) {
        reader.set_format(format);
    }
    let mut reader = reader.with_guessed_format()?;
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

// Opens without blocking on FIFOs and checks the opened handle, so the path cannot be swapped
// for a special file between the check and the read.
fn open_regular(path: &Path) -> io::Result<File> {
    let not_regular =
        || io::Error::new(io::ErrorKind::InvalidInput, "source is not a regular file");
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NONBLOCK);
    let file = match options.open(path) {
        Ok(file) => file,
        // Sockets and similar entries cannot be opened at all.
        Err(error) => {
            return Err(if path.metadata().is_ok_and(|meta| !meta.is_file()) {
                not_regular()
            } else {
                error
            });
        }
    };
    if !file.metadata()?.is_file() {
        return Err(not_regular());
    }
    Ok(file)
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
    #[cfg(unix)]
    fn decoding_rejects_a_fifo_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fifo.png");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let (sent, result) = std::sync::mpsc::channel();
        let fifo = path.clone();
        std::thread::spawn(move || sent.send(load_image(&fifo).map(drop)));
        let result = result
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("opening a FIFO blocked");
        let image::ImageError::IoError(error) = result.unwrap_err() else {
            panic!("expected a file type error");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn preview_png_may_not_overwrite_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("photo.png");
        std::fs::write(&source, b"image").unwrap();
        let aliased = dir.path().join(".").join("photo.png");
        assert!(matches!(
            check_not_source(&source, Some(&aliased)),
            Err(crate::io::Cause::IsSource)
        ));
        assert!(check_not_source(&source, Some(&dir.path().join("dots.png"))).is_ok());
        assert!(check_not_source(&source, None).is_ok());
    }
}
