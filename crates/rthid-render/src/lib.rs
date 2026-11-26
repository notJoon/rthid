//! Rasterization and PNG encoding of owned palette index grids.
#![doc = include_str!("../README.md")]

use image::ImageEncoder;
use rthid_dither::dither::PaletteColor;
use std::io;

/// Convenience helper to turn indices into opaque RGBA for previews and examples.
/// Both `grid` and `out` are `(width, height)`. Uses nearest neighbor sampling.
/// Returns an empty vector if either output dimension is zero.
///
/// Panics if the grid is empty with nonempty output, if the grid size overflows
/// or does not match `idx.len()`, or if the output byte count overflows. Panics
/// report the caller's location. `color_of` must accept every index in `idx`. It
/// is called for sampled indices and any panic inside it propagates.
#[track_caller]
pub fn rasterize(
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

/// Integer dot size from 1 to 4 that keeps the long side within 2048 px,
/// unless the grid is already larger.
pub fn preview_scale(w: u32, h: u32) -> u32 {
    2048u32.checked_div(w.max(h)).unwrap_or(1).clamp(1, 4)
}

/// Owned palette indexed grid for output rendering.
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

impl Grid {
    /// Validates owned pixels for export at [`preview_scale`]. Fields remain mutable,
    /// so [`write_png`] validates again before writing.
    ///
    /// # Errors
    /// Returns `InvalidInput` for a size mismatch, an index outside the palette,
    /// or dimensions that cannot be exported at the preview scale.
    pub fn new(indices: Vec<u8>, w: u32, h: u32, palette: Vec<PaletteColor>) -> io::Result<Self> {
        let grid = Self {
            indices,
            w,
            h,
            palette,
        };
        grid.validate(preview_scale(w, h))?;
        Ok(grid)
    }

    fn validate(&self, scale: u32) -> io::Result<(u32, u32)> {
        let invalid = |message| io::Error::new(io::ErrorKind::InvalidInput, message);
        if (self.w as usize).checked_mul(self.h as usize) != Some(self.indices.len()) {
            return Err(invalid("PNG grid size mismatch"));
        }
        if self
            .indices
            .iter()
            .any(|&i| usize::from(i) >= self.palette.len())
        {
            return Err(invalid("PNG grid index out of palette"));
        }
        let scaled = |n: u32| {
            n.checked_mul(scale)
                .filter(|&v| v > 0)
                .ok_or_else(|| invalid("invalid PNG scale or scaled dimensions"))
        };
        let ow = scaled(self.w)?;
        let oh = scaled(self.h)?;
        (ow as usize)
            .checked_mul(oh as usize)
            .and_then(|n| n.checked_mul(4))
            .filter(|&n| n <= isize::MAX as usize)
            .ok_or_else(|| invalid("invalid PNG scale or scaled dimensions"))?;
        Ok((ow, oh))
    }
}

/// Encodes opaque RGBA PNG with each dot as a `scale` pixel square.
///
/// # Errors
/// Returns `InvalidInput` for invalid grid sizes, palette indices or scale.
/// Returns encoder and writer errors with the original error preserved.
pub fn write_png(writer: impl io::Write, grid: &Grid, scale: u32) -> io::Result<()> {
    let (ow, oh) = grid.validate(scale)?;
    let pixels = rasterize(
        &grid.indices,
        (grid.w as usize, grid.h as usize),
        (ow as usize, oh as usize),
        |i| grid.palette[usize::from(i)].rgb,
    );
    image::codecs::png::PngEncoder::new(writer)
        .write_image(&pixels, ow, oh, image::ExtendedColorType::Rgba8)
        .map_err(io::Error::other)
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
        for (indices, w, h, expected) in [
            (vec![], 1, 1, "PNG grid size mismatch"),
            (vec![1], 1, 1, "PNG grid index out of palette"),
            (vec![], 0, 1, "invalid PNG scale or scaled dimensions"),
            (vec![], u32::MAX, u32::MAX, "PNG grid size mismatch"),
        ] {
            let palette = make_palette(&["#000"]).unwrap();
            let error = Grid::new(indices.clone(), w, h, palette).err().unwrap();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), expected);
            let error = write_png(
                Vec::new(),
                &grid(&indices, w, h, &["#000"]),
                preview_scale(w, h),
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), expected);
        }
        let mut valid = Grid::new(vec![0], 1, 1, make_palette(&["#000"]).unwrap()).unwrap();
        for scale in [0, u32::MAX] {
            let error = write_png(Vec::new(), &valid, scale).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert_eq!(error.to_string(), "invalid PNG scale or scaled dimensions");
        }
        valid.indices.clear();
        assert_eq!(
            write_png(Vec::new(), &valid, 1).unwrap_err().to_string(),
            "PNG grid size mismatch"
        );
    }

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
    fn encoder_errors_keep_the_original_cause() {
        let grid = grid(&[0], 1, 1, &["#000"]);
        let error = write_png(io::Cursor::new(&mut [][..]), &grid, 1).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(error.get_ref().unwrap().is::<image::ImageError>());
    }

    #[test]
    fn png_dimensions_colors_and_invalid_inputs() {
        let colors = ["#000", "#fff"];
        let mut bytes = Vec::new();
        write_png(&mut bytes, &grid(&[0, 1], 2, 1, &colors), 4).unwrap();
        let image = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (8, 4));
        assert_eq!(image.get_pixel(3, 3).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(4, 3).0, [255; 4]);
        for (indices, w, h, scale) in [
            (&[0, 2][..], 2, 1, 4),
            (&[0][..], 0, 1, 4),
            (&[0][..], 1, 1, 0),
            (&[0][..], 1, 1, u32::MAX),
        ] {
            let mut bytes = Vec::new();
            assert!(write_png(&mut bytes, &grid(indices, w, h, &colors), scale).is_err());
            assert!(bytes.is_empty());
        }
        bytes.clear();
        write_png(&mut bytes, &grid(&[1], 1, 1, &colors), 1).unwrap();
        assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 1);
    }
}
