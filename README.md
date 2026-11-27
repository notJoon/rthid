# rthid

rthid previews dithering of an image in the terminal.

```sh
cargo run -- [image]
```

Omit the image path to start with an empty preview. Press `o` to browse directories
or type an image path, including `~/` paths. Use `Up` / `Down` to select,
`Enter` to open, and `Esc` to close. In dialog lists, click to select and click
again to open, apply, or edit. The wheel moves the selection. You can also drop an image into a terminal
that supports bracketed paste. Open errors appear in the status line and keep
the current image and dialog available.

See [docs/preview.md](docs/preview.md) for keys and options.

Press `P` to manage palette presets and `T` to save or apply settings templates.
Configuration uses `--config path`, then `RTHID_CONFIG`, then an absolute
`XDG_CONFIG_HOME/rthid/config.toml`, then `$HOME/.config/rthid/config.toml`.
For example:

```toml
version = 1

[[palette]]
name = "Paper"
colors = ["#123456", "#fff"]
mode = "color"

[[template]]
name = "Zine"
palette = "Paper"
colors = ["#123456", "#fff"]
mode = "color"
algorithm = "bayer4"
seed = 0
strength = 0.8
```

Algorithm names: `atkinson`, `floyd-steinberg`, `jarvis-judice-ninke`,
`stucki`, `sierra-lite`, `bayer2`, `bayer4`, `bayer8`, `noise`.

## Benchmarks and profiling

Run the CPU benchmarks with `mise run bench`; CodSpeed also runs them in CI.
For profiling, install samply with `cargo install samply`, build with
`cargo build --profile profiling` and run `samply record target/profiling/rthid`.
