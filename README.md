# rthid

rthid previews dithering of an image in the terminal.

```sh
cargo run -- [image]
```

Omit the image path to start with an empty preview. Press `o` to browse directories
or type an image path, including `~/` paths. Use `Up` / `Down` to select,
`Enter` to open, and `Esc` to close. You can also drop an image into a terminal
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
strength = 0.8
```
