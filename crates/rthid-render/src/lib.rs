//! Rasterization and PNG encoding of owned palette index grids.
#![doc = include_str!("../README.md")]

use image::ImageEncoder;
use rthid_dither::dither::PaletteColor;
use std::io;

/// Samples dots into opaque RGBA pixels with nearest neighbor sampling.
/// Empty output returns no bytes.
///
/// # Panics
/// Panics if grid dimensions do not match the indices, a size overflows,
/// a nonempty output has an empty grid, or `color_of` panics.
pub(crate) fn rasterize(
    idx: &[u8],
    grid: (usize, usize),
    out: (usize, usize),
    color_of: impl Fn(u8) -> [u8; 3],
) -> Vec<u8> {
    let (gw, gh) = grid;
    let (ow, oh) = out;
    let n = gw.checked_mul(gh).expect("grid size overflow");
    assert!(
        idx.len() == n,
        "idx has {} indices, expected {gw}x{gh} = {n}",
        idx.len()
    );
    let size = ow
        .checked_mul(oh)
        .and_then(|n| n.checked_mul(4))
        .expect("output size overflow");
    if size == 0 {
        return Vec::new();
    }
    assert!(
        gw > 0 && gh > 0,
        "source dimensions must be positive, got {gw}x{gh}"
    );
    let mut rgba = vec![0; size];
    for y in 0..oh {
        let row = y * gh / oh * gw;
        for x in 0..ow {
            let color = color_of(idx[row + x * gw / ow]);
            let offset = (y * ow + x) * 4;
            rgba[offset..offset + 3].copy_from_slice(&color);
            rgba[offset + 3] = 255;
        }
    }
    rgba
}

/// Maximum output area, bounding the RGBA buffer to about 1 GiB.
pub const MAX_OUTPUT_PIXELS: u64 = 1 << 28;

/// Output size in pixels using an integer dot size from 1 to 4, keeping the
/// long side within 2048 pixels unless the grid is already larger.
#[must_use]
pub fn preview_size(w: u32, h: u32) -> (u32, u32) {
    let scale = 2048u32.checked_div(w.max(h)).unwrap_or(1).clamp(1, 4);
    (w * scale, h * scale)
}

/// Owned palette index grid for output rendering.
#[derive(Debug, Clone)]
pub struct Grid {
    /// One palette index per dot, row major.
    pub indices: Vec<u8>,
    /// Grid width in dots.
    pub w: u32,
    /// Grid height in dots.
    pub h: u32,
    /// The same palette passed to `dither`, whose order the indices refer to.
    pub palette: Vec<PaletteColor>,
}

impl Grid {
    /// Validates grid dimensions and palette indices independently of output size.
    /// Fields remain public, so [`write_png`] validates again before writing.
    ///
    /// # Errors
    /// Returns `InvalidInput` for zero dimensions, a size mismatch or overflow,
    /// or an index outside the palette.
    pub fn new(indices: Vec<u8>, w: u32, h: u32, palette: Vec<PaletteColor>) -> io::Result<Self> {
        let grid = Self {
            indices,
            w,
            h,
            palette,
        };
        grid.validate()?;
        Ok(grid)
    }

    fn validate(&self) -> io::Result<()> {
        let invalid = |message| io::Error::new(io::ErrorKind::InvalidInput, message);
        let (w, h) = (self.w, self.h);
        if w == 0 || h == 0 {
            return Err(invalid(format!(
                "grid dimensions must be positive, got {w}x{h}"
            )));
        }
        let n = (w as usize)
            .checked_mul(h as usize)
            .ok_or_else(|| invalid(format!("grid size {w}x{h} overflows")))?;
        if n != self.indices.len() {
            return Err(invalid(format!(
                "grid has {} indices, expected {w}x{h} = {n}",
                self.indices.len()
            )));
        }
        if let Some((pos, i)) = self
            .indices
            .iter()
            .enumerate()
            .find(|&(_, &i)| usize::from(i) >= self.palette.len())
        {
            return Err(invalid(format!(
                "index {i} at position {pos} is outside the palette of {} colors",
                self.palette.len()
            )));
        }
        Ok(())
    }
}

// Preserve original writer errors because the PNG backend erases their kinds.
struct PngWriter<W> {
    inner: W,
    error: Option<io::Error>,
}

impl<W: io::Write> io::Write for PngWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.inner.write(bytes).map_err(|error| {
            let stand_in = io::Error::new(error.kind(), error.to_string());
            self.error = Some(error);
            stand_in
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().map_err(|error| {
            let stand_in = io::Error::new(error.kind(), error.to_string());
            self.error = Some(error);
            stand_in
        })
    }
}

/// Encodes dots as opaque RGBA PNG at `size` in pixels with nearest neighbor sampling.
/// Nothing is written when validation fails. A writer error may leave partial
/// output. The writer is flushed on success.
///
/// # Errors
/// Returns `InvalidInput` for invalid grid dimensions or palette indices,
/// zero output sides, sides above `i32::MAX`, or an area above [`MAX_OUTPUT_PIXELS`].
/// Writer errors are returned unchanged, including flush errors. Other encoder
/// errors are wrapped with `io::Error::other`.
pub fn write_png(writer: impl io::Write, grid: &Grid, size: (u32, u32)) -> io::Result<()> {
    grid.validate()?;
    let (width, height) = size;
    let invalid = |message| io::Error::new(io::ErrorKind::InvalidInput, message);
    if width == 0 || height == 0 {
        return Err(invalid(format!(
            "output size {width}x{height} must be positive"
        )));
    }
    if width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(invalid(format!(
            "output size {width}x{height} exceeds the PNG side limit"
        )));
    }
    if u64::from(width) * u64::from(height) > MAX_OUTPUT_PIXELS {
        return Err(invalid(format!(
            "output size {width}x{height} exceeds {MAX_OUTPUT_PIXELS} pixels"
        )));
    }
    let pixels = rasterize(
        &grid.indices,
        (grid.w as usize, grid.h as usize),
        (width as usize, height as usize),
        |i| grid.palette[usize::from(i)].rgb,
    );
    let mut writer = PngWriter {
        inner: writer,
        error: None,
    };
    image::codecs::png::PngEncoder::new(&mut writer)
        .write_image(&pixels, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|error| {
            writer.error.take().unwrap_or_else(|| match error {
                image::ImageError::IoError(error) => error,
                error => io::Error::other(error),
            })
        })?;
    writer.inner.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rthid_dither::dither::make_palette;

    fn grid(indices: &[u8], w: u32, h: u32, colors: &[&str]) -> Grid {
        Grid {
            indices: indices.to_vec(),
            w,
            h,
            palette: make_palette(colors).unwrap(),
        }
    }

    #[test]
    fn rasterize_scales_nearest_neighbor() {
        let result = rasterize(&[0, 1], (2, 1), (4, 2), |i| {
            if i == 0 {
                [10, 20, 30]
            } else {
                [200, 210, 220]
            }
        });
        let row = [
            10, 20, 30, 255, 10, 20, 30, 255, 200, 210, 220, 255, 200, 210, 220, 255,
        ];
        assert_eq!(result, row.repeat(2));
    }

    #[test]
    fn rasterize_empty_output_accepts_empty_grids() {
        for grid in [(0, 0), (0, 1), (1, 0)] {
            for out in [(0, 0), (0, 2), (2, 0)] {
                assert!(rasterize(&[], grid, out, |_| panic!("empty output")).is_empty());
            }
            assert!(std::panic::catch_unwind(|| rasterize(&[], grid, (1, 1), |_| [0; 3])).is_err());
        }
        assert!(rasterize(&[0], (1, 1), (0, 2), |_| panic!("empty output")).is_empty());
    }

    #[test]
    #[should_panic(expected = "idx has 1 indices, expected 2x3 = 6")]
    fn index_mismatch_reports_actual_and_expected_size() {
        rasterize(&[0], (2, 3), (1, 1), |_| [0; 3]);
    }

    #[test]
    fn rasterize_rejects_size_overflow() {
        for (grid, out) in [
            ((usize::MAX, 2), (1, 1)),
            ((1, 1), (usize::MAX, 2)),
            ((1, 1), (usize::MAX / 4 + 1, 1)),
        ] {
            assert!(std::panic::catch_unwind(|| rasterize(&[0], grid, out, |_| [0; 3])).is_err());
        }
    }

    #[test]
    #[should_panic(expected = "unsupported index 7")]
    fn rasterize_propagates_callback_panics() {
        rasterize(&[7], (1, 1), (1, 1), |i| panic!("unsupported index {i}"));
    }

    #[test]
    fn constructor_and_writer_report_the_failed_grid_check() {
        let huge_size_error = if usize::BITS == 32 {
            "grid size 4294967295x4294967295 overflows"
        } else {
            "grid has 0 indices, expected 4294967295x4294967295 = 18446744065119617025"
        };
        for (indices, w, h, palette_len, expected) in [
            (vec![], 1, 1, 1, "grid has 0 indices, expected 1x1 = 1"),
            (
                vec![0, 1],
                2,
                1,
                1,
                "index 1 at position 1 is outside the palette of 1 colors",
            ),
            (
                vec![0],
                1,
                1,
                0,
                "index 0 at position 0 is outside the palette of 0 colors",
            ),
            (vec![], 0, 1, 1, "grid dimensions must be positive, got 0x1"),
            (vec![], 1, 0, 1, "grid dimensions must be positive, got 1x0"),
            (vec![], u32::MAX, u32::MAX, 1, huge_size_error),
        ] {
            let mut palette = make_palette(&["#000"]).unwrap();
            palette.truncate(palette_len);
            let error = Grid::new(indices.clone(), w, h, palette.clone()).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), expected);
            let mut bytes = Vec::new();
            let error = write_png(
                &mut bytes,
                &Grid {
                    indices,
                    w,
                    h,
                    palette,
                },
                (1, 1),
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), expected);
            assert!(bytes.is_empty());
        }
        let mut valid = Grid::new(vec![0], 1, 1, make_palette(&["#000"]).unwrap()).unwrap();
        valid.indices.clear();
        assert_eq!(
            write_png(Vec::new(), &valid, (1, 1))
                .unwrap_err()
                .to_string(),
            "grid has 0 indices, expected 1x1 = 1"
        );
    }

    #[test]
    fn output_validation_precedes_allocation_and_writing() {
        let grid = grid(&[0], 1, 1, &["#000"]);
        for (size, expected) in [
            ((0, 1), "output size 0x1 must be positive"),
            ((1, 0), "output size 1x0 must be positive"),
            (
                (1 << 31, 1),
                "output size 2147483648x1 exceeds the PNG side limit",
            ),
            (
                (1, 1 << 31),
                "output size 1x2147483648 exceeds the PNG side limit",
            ),
            (
                (1 << 20, 1 << 20),
                "output size 1048576x1048576 exceeds 268435456 pixels",
            ),
        ] {
            let mut bytes = Vec::new();
            let error = write_png(&mut bytes, &grid, size).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), expected);
            assert!(bytes.is_empty());
        }
    }

    #[test]
    fn preview_size_matches_the_old_scale_table() {
        for (w, h, old_scale) in [
            (1, 1, 4),
            (100, 20, 4),
            (20, 1024, 2),
            (600, 400, 3),
            (1200, 400, 1),
            (2048, 2048, 1),
            (4096, 1, 1),
            (0, 0, 1),
            (u32::MAX, 1, 1),
        ] {
            assert_eq!(
                preview_size(w, h),
                (w * old_scale, h * old_scale),
                "{w}x{h}"
            );
        }
    }

    struct CountingWriter {
        writes: usize,
        flushes: usize,
        fail_write: bool,
        fail_flush: bool,
    }

    impl io::Write for CountingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.writes += 1;
            if self.fail_write {
                return Err(io::Error::new(io::ErrorKind::StorageFull, "disk full"));
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            if self.fail_flush {
                return Err(io::Error::new(io::ErrorKind::StorageFull, "flush failed"));
            }
            Ok(())
        }
    }

    #[test]
    fn writer_errors_and_flush_are_preserved() {
        let grid = grid(&[0], 1, 1, &["#000"]);
        for (fail_write, fail_flush) in [(false, false), (true, false), (false, true)] {
            let mut writer = CountingWriter {
                writes: 0,
                flushes: 0,
                fail_write,
                fail_flush,
            };
            let result = write_png(&mut writer, &grid, (1, 1));
            assert!(writer.writes > 0);
            assert_eq!(writer.flushes, usize::from(!fail_write));
            if fail_write || fail_flush {
                let error = result.unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::StorageFull);
                assert_eq!(
                    error.to_string(),
                    if fail_write {
                        "disk full"
                    } else {
                        "flush failed"
                    }
                );
            } else {
                result.unwrap();
            }
        }
        let mut writer = CountingWriter {
            writes: 0,
            flushes: 0,
            fail_write: false,
            fail_flush: false,
        };
        assert!(write_png(&mut writer, &grid, (0, 1)).is_err());
        assert_eq!((writer.writes, writer.flushes), (0, 0));
    }

    #[test]
    fn writer_errors_survive_partial_output() {
        struct PartialWriter {
            bytes: Vec<u8>,
        }

        impl io::Write for PartialWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.bytes.len() == 16 {
                    return Err(io::Error::new(
                        io::ErrorKind::StorageFull,
                        "disk full mid stream",
                    ));
                }
                let count = bytes.len().min(16 - self.bytes.len());
                self.bytes.extend_from_slice(&bytes[..count]);
                Ok(count)
            }

            fn flush(&mut self) -> io::Result<()> {
                panic!("failed output must not be flushed")
            }
        }

        let mut writer = PartialWriter { bytes: Vec::new() };
        let error = write_png(&mut writer, &grid(&[0], 1, 1, &["#000"]), (1, 1)).unwrap_err();
        assert_eq!(writer.bytes.len(), 16);
        assert!(writer.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
        assert_eq!(error.to_string(), "disk full mid stream");
    }

    #[test]
    fn integer_output_gives_solid_squares_and_unchanged_png_bytes() {
        let grid = grid(&[0, 1], 2, 1, &["#000", "#fff"]);
        let mut bytes = Vec::new();
        write_png(&mut bytes, &grid, preview_size(grid.w, grid.h)).unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (8, 4));
        for (x, _, pixel) in image.enumerate_pixels() {
            assert_eq!(pixel.0, if x < 4 { [0, 0, 0, 255] } else { [255; 4] });
        }
        let mut old_bytes = Vec::new();
        let row = [[0, 0, 0, 255].repeat(4), [255; 4].repeat(4)].concat();
        image::codecs::png::PngEncoder::new(&mut old_bytes)
            .write_image(&row.repeat(4), 8, 4, image::ExtendedColorType::Rgba8)
            .unwrap();
        assert_eq!(bytes, old_bytes);
    }

    #[test]
    fn output_size_allows_fractional_dot_sizes() {
        let mut bytes = Vec::new();
        write_png(&mut bytes, &grid(&[0, 1], 2, 1, &["#000", "#fff"]), (3, 1)).unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (3, 1));
        assert_eq!(
            image.into_raw(),
            [0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255]
        );
    }
}
