# rthid-dither

Terminal independent palette dithering, PNG export and atomic file replacement,
used by the rthid binary.

| Module | Purpose |
| --- | --- |
| `dither` | Atkinson dithering, palettes from hex colors and rasterization of palette indices |
| `png` | PNG export and a background writer that writes the latest queued grid |
| `atomic` | File replacement through a temporary file beside the destination |

See the crate docs (`cargo doc -p rthid-dither --open`) for the API.
