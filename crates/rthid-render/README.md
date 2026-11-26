# rthid-render

Rasterization and PNG encoding of palette index grids, independent of file paths
and terminal state. Uses palette colors from rthid-dither. A dot is one grid
cell. A pixel is one output image pixel.

## Quick start

This crate is not published to crates.io. Add it from your checkout:

```toml
[dependencies]
rthid-render = { path = "../rthid/crates/rthid-render" }
rthid-dither = { path = "../rthid/crates/rthid-dither" }
```

```rust
use rthid_dither::dither::Options;
use rthid_render::{Grid, preview_size, write_png};

fn main() -> std::io::Result<()> {
    let grid = Grid::new(vec![0, 1], 2, 1, Options::default().palette)?;
    let mut bytes = Vec::new();
    write_png(&mut bytes, &grid, preview_size(grid.w, grid.h), None)?;
    assert!(bytes.starts_with(b"\x89PNG"));
    Ok(())
}
```

## Dither an RGBA buffer and save a PNG

```rust
use rthid_dither::dither::{Options, dither};
use rthid_render::{Grid, write_png};
use std::{fs::File, io::BufWriter};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (w, h): (usize, usize) = (2, 1);
    let rgba = [0, 0, 0, 255, 255, 255, 255, 255];
    let options = Options::default();
    let indices = dither(&rgba, w, h, &options);
    let grid = Grid::new(indices, w.try_into()?, h.try_into()?, options.palette)?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("preview.png");
    write_png(
        BufWriter::new(File::create(path)?),
        &grid,
        (1181, 2126),
        Some(600),
    )?;
    Ok(())
}
```

`Grid::new` validates grid dimensions and palette indices independently of output
size. Fields remain public, so `write_png` validates again before writing.
`write_png` samples dots with nearest neighbor sampling at the requested size in
pixels, including sizes with a fractional number of pixels per dot.
Pass `Some(dpi)` to record resolution in pixels per metre on both axes, rounded
from `dpi / 0.0254`. Pass `None` to omit resolution metadata and keep the default
encoder bytes. The caller chooses pixel dimensions independently of this metadata.
`check_output_size` validates dimensions before submitting an output job.
`preview_size` uses an integer dot size from 1 to 4 pixels for previews.

Invalid grids and output sizes return `InvalidInput` before allocation or writing.
Each output side must be positive and at most `i32::MAX`. Output area is limited
to `MAX_OUTPUT_PIXELS`, bounding the RGBA buffer to about 1 GiB. Writer errors are
returned unchanged. Other encoder errors are wrapped in `std::io::Error`.
A writer error may leave partial output. The writer is flushed on success.

The renderer encodes to any `std::io::Write` and owns no paths.

See the crate docs (`cargo doc -p rthid-render --open`) for the API.
