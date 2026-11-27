//! Rasterization and PNG encoding of owned palette index grids.
#![doc = include_str!("../README.md")]

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
            self.error.get_or_insert(error);
            stand_in
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().map_err(|error| {
            let stand_in = io::Error::new(error.kind(), error.to_string());
            self.error.get_or_insert(error);
            stand_in
        })
    }
}

/// Validates output dimensions before allocation or job submission.
///
/// # Errors
/// Returns `InvalidInput` for zero sides, sides above `i32::MAX`,
/// or an area above [`MAX_OUTPUT_PIXELS`].
pub fn check_output_size(size: (u32, u32)) -> io::Result<()> {
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
    Ok(())
}

/// Encodes dots as opaque RGBA PNG at `size` in pixels with nearest neighbor sampling.
/// Nothing is written when validation fails. A writer error may leave partial
/// output. The writer is flushed on success.
/// `Some(dpi)` records resolution in pixels per metre on both axes.
/// `None` omits resolution metadata and preserves the default encoder bytes.
///
/// # Errors
/// Returns `InvalidInput` for invalid grid dimensions or palette indices,
/// zero output sides, sides above `i32::MAX`, or an area above [`MAX_OUTPUT_PIXELS`].
/// Writer errors are returned unchanged, including flush errors. Other encoder
/// errors are wrapped with `io::Error::other`.
pub fn write_png(
    writer: impl io::Write,
    grid: &Grid,
    size: (u32, u32),
    dpi: Option<u32>,
) -> io::Result<()> {
    grid.validate()?;
    check_output_size(size)?;
    let (width, height) = size;
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
    let result = (|| {
        let mut encoder = png::Encoder::new(&mut writer, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        encoder.set_filter(png::Filter::Adaptive);
        encoder.set_pixel_dims(dpi.map(|dpi| {
            let pixels_per_metre = (f64::from(dpi) / 0.0254).round() as u32;
            png::PixelDimensions {
                xppu: pixels_per_metre,
                yppu: pixels_per_metre,
                unit: png::Unit::Meter,
            }
        }));
        let mut png = encoder.write_header()?;
        png.write_image_data(&pixels)?;
        png.finish()
    })();
    result.map_err(|error| {
        writer
            .error
            .take()
            .unwrap_or_else(|| io::Error::other(error))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
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
                None,
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), expected);
            assert!(bytes.is_empty());
        }
        let mut valid = Grid::new(vec![0], 1, 1, make_palette(&["#000"]).unwrap()).unwrap();
        valid.indices.clear();
        assert_eq!(
            write_png(Vec::new(), &valid, (1, 1), None)
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
            let error = write_png(&mut bytes, &grid, size, None).unwrap_err();
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
            let result = write_png(&mut writer, &grid, (1, 1), None);
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
        assert!(write_png(&mut writer, &grid, (0, 1), None).is_err());
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
        let error = write_png(&mut writer, &grid(&[0], 1, 1, &["#000"]), (1, 1), None).unwrap_err();
        assert_eq!(writer.bytes.len(), 16);
        assert!(writer.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(error.kind(), io::ErrorKind::StorageFull);
        assert_eq!(error.to_string(), "disk full mid stream");
    }

    #[test]
    fn resolution_metadata_is_optional_and_none_preserves_encoder_bytes() {
        for (grid, size) in [
            (grid(&[0], 1, 1, &["#123"]), (1, 1)),
            (grid(&[0, 1], 2, 1, &["#000", "#fff"]), (3, 7)),
            (
                grid(&[0, 1, 2, 2, 1, 0], 3, 2, &["#123", "#456", "#abc"]),
                (257, 129),
            ),
        ] {
            for dpi in [None, Some(600)] {
                let mut bytes = Vec::new();
                write_png(&mut bytes, &grid, size, dpi).unwrap();
                let reader = png::Decoder::new(io::Cursor::new(&bytes))
                    .read_info()
                    .unwrap();
                assert_eq!(
                    reader
                        .info()
                        .pixel_dims
                        .map(|dims| (dims.xppu, dims.yppu, dims.unit)),
                    dpi.map(|_| (23622, 23622, png::Unit::Meter))
                );
                if dpi.is_none() {
                    let pixels = rasterize(
                        &grid.indices,
                        (grid.w as usize, grid.h as usize),
                        (size.0 as usize, size.1 as usize),
                        |i| grid.palette[usize::from(i)].rgb,
                    );
                    let mut expected = Vec::new();
                    image::codecs::png::PngEncoder::new_with_quality(
                        &mut expected,
                        image::codecs::png::CompressionType::Fast,
                        image::codecs::png::FilterType::Adaptive,
                    )
                    .write_image(&pixels, size.0, size.1, image::ExtendedColorType::Rgba8)
                    .unwrap();
                    assert_eq!(bytes, expected);
                }
            }
        }
    }

    #[test]
    fn output_size_allows_fractional_dot_sizes() {
        let mut bytes = Vec::new();
        write_png(
            &mut bytes,
            &grid(&[0, 1], 2, 1, &["#000", "#fff"]),
            (3, 1),
            None,
        )
        .unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (3, 1));
        assert_eq!(
            image.into_raw(),
            [0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255, 255]
        );
    }
}
