# rthid-render

Rasterization and PNG encoding of palette index grids, independent of file paths
and terminal state. Uses palette colors from rthid-dither.

## Quick start

This crate is not published to crates.io. Add it from your checkout:

```toml
[dependencies]
rthid-render = { path = "../rthid/crates/rthid-render" }
rthid-dither = { path = "../rthid/crates/rthid-dither" }
```

```rust
use rthid_dither::dither::Options;
use rthid_render::{Grid, preview_scale, write_png};

fn main() -> std::io::Result<()> {
    let grid = Grid::new(vec![0, 1], 2, 1, Options::default().palette)?;
    let mut bytes = Vec::new();
    write_png(&mut bytes, &grid, preview_scale(grid.w, grid.h))?;
    assert!(bytes.starts_with(b"\x89PNG"));
    Ok(())
}
```

`rasterize` expands indices into opaque RGBA with nearest neighbor sampling.
`Grid::new` validates dimensions and palette indices at the preview scale.
Fields remain public, so `write_png` validates again at the requested scale.
Invalid grids and scales return `InvalidInput`. Encoder errors preserve their
original cause. `preview_scale` selects an integer dot size from 1 to 4.

The renderer encodes to any `std::io::Write` and owns no paths.

See the crate docs (`cargo doc -p rthid-render --open`) for the API.
