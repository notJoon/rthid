//! rthid previews dithering of an image in the terminal.
//!
//! The source is loaded and shrunk once. `grid::GridCache` scales and rotates it into
//! the grid and, when Dithered or a companion PNG needs it, dithers the grid
//! into palette indices. `app::App` holds `settings::Settings` and the
//! `viewport::Viewport`. `ui::draw` turns them into a frame through `cells` (cell glyphs) or `graphics`
//! (terminal images), and `Export` writes the dithered grid to the companion PNG.
//!
//! Glossary:
//! - Grid: the scaled and rotated image. Its pixels are also called dots.
//! - Cell: one terminal character position.
//! - Half block: the `▀` glyph whose fg and bg colors show two stacked dots in one cell.
//! - Sextant and Quadrant: glyph sets for the sampled Dithered preview and minimap that split a
//!   cell into 2x3 or 2x2 subcells painted in the cell fg or bg color.
//! - Fit: the whole grid fitted to the preview.
//! - Zoom levels: Fit, 25%, 33%, 50%, 100%, 200%, 300% and 400%.
//! - 1:1: 100 percent, one dot per half cell.
//! - Minimap: a fitted inset when the image overflows, dimmed outside the visible region.
//! - Approximate: some sampled cell used more than two palette colors, so
//!   the extra samples took the nearer kept color.
//! - Companion PNG: the optional `--preview-png` file, rewritten after each new dither.

/// Interactive state and input handling.
mod app;
/// Cell rendering for Fit, zoom levels and the minimap.
mod cells;
/// Companion PNG writer and its status.
mod export;
/// Terminal image detection, placement and cleanup (Kitty, iTerm2, Sixel) for Original at Fit.
mod graphics;
/// Source image, scaled grid and dithered indices caches.
mod grid;
/// Single entry caches with build versions.
mod memo;
/// Modal input and preset editing.
mod modal;
/// Paths carried by terminal paste events.
mod paste;
/// Editable settings and palette presets.
mod settings;
/// Configuration storage and independent settings templates.
mod store;
/// Screen layout, panels and status line.
mod ui;
/// Preview coordinates for Fit, zoom levels and the minimap.
mod viewport;

use std::{
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use clap::{Parser, ValueEnum};
use image::{RgbaImage, imageops};
use ratatui::DefaultTerminal;
use ratatui::crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, MouseEventKind,
    },
    execute,
};

use crate::{
    app::{App, Request},
    cells::Glyphs,
    graphics::Mode as Graphics,
    modal::Entry,
    ui::Renderer,
};

/// Disables mouse capture on drop, so it is released even when the terminal session fails.
struct MouseCapture;

impl Drop for MouseCapture {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

/// Disables bracketed paste even when the terminal session fails.
struct BracketedPaste;

impl Drop for BracketedPaste {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableBracketedPaste);
    }
}

// Sources are shrunk on load to bound dither work per keypress.
// Offering a full resolution export would require keeping the unshrunk original.
const MAX_SOURCE_SIDE: u32 = 1024;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Source image to preview.
    image: Option<PathBuf>,
    /// Save the dithered preview to a PNG file.
    #[arg(long, value_name = "PATH")]
    preview_png: Option<PathBuf>,
    /// Load user presets and templates from this TOML file.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Glyph set for the sampled Dithered preview and minimap.
    #[arg(long, value_enum)]
    glyphs: Option<CliGlyphs>,
    /// Detect terminal graphics or use glyph rendering.
    #[arg(long, value_enum, default_value = "auto")]
    graphics: CliGraphics,
}

// TODO: These enums mirror cells::Glyphs and graphics::Mode to keep clap out of lower modules.
// Revisit deriving ValueEnum on domain enums if that separation stops mattering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CliGlyphs {
    Sextant,
    Quadrant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CliGraphics {
    Auto,
    Off,
}

// The companion PNG is replaced on every re-dither, so it must never be the source image.
// A missing destination cannot be the source, which has to exist to load.
fn check_not_source(source: &Path, preview: Option<&Path>) -> io::Result<()> {
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

// Shrinks sources on load to bound processing work.
fn load_image(path: &Path) -> Result<RgbaImage, String> {
    let source =
        image::open(path).map_err(|error| format!("Could not load {}: {error}", path.display()))?;
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

fn list_dir(dir: &Path) -> io::Result<(PathBuf, Vec<Entry>)> {
    let dir = dir.canonicalize()?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let metadata = entry.metadata()?;
        let is_dir = metadata.is_dir();
        if is_dir
            || (metadata.is_file()
                && image::ImageFormat::from_path(entry.path())
                    .is_ok_and(|format| format.reading_enabled()))
        {
            entries.push(Entry { name, dir: is_dir });
        }
    }
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    if dir.parent().is_some() {
        entries.insert(
            0,
            Entry {
                name: "..".into(),
                dir: true,
            },
        );
    }
    Ok((dir, entries))
}

fn expand_tilde(text: &str, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        if text == "~" {
            return home.to_path_buf();
        }
        if let Some(rest) = text.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(text)
}

fn handle_request(app: &mut App, request: Request, preview_png: Option<&Path>) {
    let request = match request {
        Request::Open(path) => {
            let home = std::env::var_os("HOME").map(PathBuf::from);
            let path = path
                .to_str()
                .map(|text| expand_tilde(text, home.as_deref()))
                .unwrap_or(path);
            if path.is_dir() {
                Request::List(path)
            } else {
                Request::Open(path)
            }
        }
        request => request,
    };
    match request {
        Request::List(path) => match list_dir(&path) {
            Ok((dir, entries)) => app.set_listing(dir, entries),
            Err(error) => {
                app.set_notice(format!("open: {}: {error}", path.display()));
            }
        },
        Request::Open(path) => {
            match check_not_source(&path, preview_png)
                .map_err(|error| error.to_string())
                .and_then(|()| load_image(&path))
            {
                Ok(image) => app.open_image(path, image),
                Err(error) => {
                    app.set_notice(format!("open: {error}"));
                }
            }
        }
    }
}

// Pointer motion alone changes nothing on screen.
fn needs_redraw(event: &Event) -> bool {
    !matches!(event, Event::Mouse(mouse) if mouse.kind == MouseEventKind::Moved)
}

fn main() -> std::io::Result<()> {
    let Args {
        image: path,
        preview_png,
        config,
        glyphs,
        graphics,
    } = Args::parse();
    let glyphs = glyphs.map(|glyphs| match glyphs {
        CliGlyphs::Sextant => Glyphs::Sextant,
        CliGlyphs::Quadrant => Glyphs::Quadrant,
    });
    let graphics = match graphics {
        CliGraphics::Auto => Graphics::Auto,
        CliGraphics::Off => Graphics::Off,
    };
    if let Some(path) = &path {
        check_not_source(path, preview_png.as_deref())?;
    }
    let mut debug_log = std::env::var_os("RTHID_DEBUG_LOG")
        .map(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        })
        .transpose()?;
    let source = path.map(|path| {
        let image = load_image(&path).unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(1);
        });
        (path, image)
    });
    let config_path = store::config_path(config);
    let loaded = store::load_or_builtin(config_path.as_deref());
    let mut messages = Vec::new();
    if let Some(first) = loaded.warnings.first() {
        messages.push(format!(
            "{} entries skipped: {first}",
            loaded.warnings.len()
        ));
    }
    if let Some(reason) = &loaded.save_blocked {
        messages.push(format!(
            "saving disabled: {}",
            reason.lines().next().unwrap_or(reason)
        ));
    }
    let notice = (!messages.is_empty()).then(|| format!("config: {}", messages.join(" | ")));
    let mut app = App::new(
        source,
        preview_png.clone(),
        loaded.palettes,
        loaded.templates,
        notice,
        loaded.save_blocked,
    );
    let term_program = std::env::var("TERM_PROGRAM").ok();
    app.glyphs = glyphs.unwrap_or_else(|| Glyphs::for_terminal(term_program.as_deref()));
    let mouse_capture = MouseCapture;
    let bracketed_paste = BracketedPaste;
    let terminal_result = ratatui::run(|terminal| {
        let mut renderer = ui::Renderer::new(graphics::detect(graphics, term_program.as_deref()));
        execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
        event_loop(
            terminal,
            &mut app,
            &mut renderer,
            debug_log.as_mut(),
            config_path.as_deref(),
            preview_png.as_deref(),
        )
    });
    drop(bracketed_paste);
    drop(mouse_capture);
    let png_result = app.finish_png();
    terminal_result.and(png_result)
}

// Syncs and draws when something changed, then waits for input and applies
// every queued event before the next draw. Runs until the app requests quit.
fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    renderer: &mut Renderer,
    mut debug_log: Option<&mut File>,
    config_path: Option<&Path>,
    preview_png: Option<&Path>,
) -> io::Result<()> {
    let mut layout = ui::layout(terminal.size()?);
    let mut redraw = true;
    while !app.quit {
        if redraw {
            app.poll_export();
            app.sync(layout.preview);
            let frame = terminal.draw(|frame| {
                ui::draw(frame, app, &layout, renderer);
            })?;
            if let Some(log) = &mut debug_log {
                ui::log_minimap(app, log, &frame, &app.geometry(layout.preview))?;
            }
        }
        if let Some(request) = app.take_request() {
            handle_request(app, request, preview_png);
            redraw = true;
            continue;
        }
        redraw = false;
        // With a companion PNG, wake periodically so background write results reach the status line.
        if app.has_export() && !event::poll(Duration::from_millis(50))? {
            redraw = app.poll_export();
            continue;
        }
        // Drain queued input so held-down repeats cost one re-dither, not one each.
        loop {
            let event = event::read()?;
            if let Some(log) = &mut debug_log {
                writeln!(log, "event {event:?}")?;
            }
            redraw |= needs_redraw(&event);
            match event {
                Event::Key(key) => app.handle_key(key, &app.geometry(layout.preview)),
                Event::Mouse(mouse) => app.handle_mouse(mouse, &app.geometry(layout.preview)),
                Event::Paste(text) => app.handle_paste(&text),
                Event::Resize(width, height) => {
                    layout = ui::layout(ratatui::layout::Size::new(width, height));
                    app.clamp_viewport(layout.preview);
                }
                _ => {}
            }
            if !event::poll(Duration::ZERO)? {
                break;
            }
        }
        if app.take_save()
            && let Some(path) = config_path
            && let Err(error) = store::save(path, &app.palettes, &app.templates)
        {
            redraw |= app.set_notice(format!("config: {error}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn directory_listing_filters_and_sorts() {
        let temp = tempfile::tempdir().unwrap();
        for name in [".hidden.png", "z.png", "b.png", "notes.txt"] {
            std::fs::write(temp.path().join(name), b"").unwrap();
        }
        for name in ["c", "a"] {
            std::fs::create_dir(temp.path().join(name)).unwrap();
        }
        let (dir, entries) = list_dir(temp.path()).unwrap();
        assert_eq!(dir, temp.path().canonicalize().unwrap());
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.name.as_str(), entry.dir))
                .collect::<Vec<_>>(),
            [
                ("..", true),
                ("a", true),
                ("c", true),
                ("b.png", false),
                ("z.png", false)
            ]
        );
        assert!(list_dir(&temp.path().join("missing")).is_err());
    }

    #[test]
    fn tilde_expansion_only_uses_a_home_prefix() {
        for (text, home, expected) in [
            ("~", Some(Path::new("/home/me")), "/home/me"),
            (
                "~/photo.png",
                Some(Path::new("/home/me")),
                "/home/me/photo.png",
            ),
            (
                "~other/photo.png",
                Some(Path::new("/home/me")),
                "~other/photo.png",
            ),
            ("photo~.png", Some(Path::new("/home/me")), "photo~.png"),
            ("~/photo.png", None, "~/photo.png"),
            ("~", None, "~"),
        ] {
            assert_eq!(expand_tilde(text, home), PathBuf::from(expected));
        }
    }

    #[test]
    fn open_requests_list_directories_and_keep_failures_open() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = App::empty();
        app.modal = Some(modal::Modal::open_file(temp.path().to_path_buf()));
        app.request_open(temp.path().to_path_buf());
        let request = app.take_request().unwrap();
        handle_request(&mut app, request, None);
        assert!(app.notice().is_none());
        let Some(modal::Modal::OpenFile(picker)) = &app.modal else {
            panic!("open dialog expected")
        };
        assert_eq!(picker.dir, temp.path().canonicalize().unwrap());
        handle_request(
            &mut app,
            Request::Open(temp.path().join("missing.png")),
            None,
        );
        assert!(matches!(app.modal, Some(modal::Modal::OpenFile(_))));
        assert!(app.notice().unwrap().contains("Could not load"));
        app.open_image(temp.path().join("photo.png"), RgbaImage::new(2, 2));
        assert!(app.modal.is_none());
        assert!(app.notice().is_none());
    }

    #[test]
    fn cli_defaults_and_definition() {
        Args::command().debug_assert();
        let args = Args::try_parse_from(["rthid"]).unwrap();
        assert_eq!(args.image, None);
        assert_eq!(args.preview_png, None);
        assert_eq!(args.config, None);
        assert!(args.glyphs.is_none());
        assert!(matches!(args.graphics, CliGraphics::Auto));
    }

    #[test]
    fn cli_accepts_enum_values_and_paths() {
        for (value, expected) in [("auto", CliGraphics::Auto), ("off", CliGraphics::Off)] {
            let args = Args::try_parse_from(["rthid", "--graphics", value]).unwrap();
            assert_eq!(args.graphics, expected);
        }
        for (value, expected) in [
            ("sextant", CliGlyphs::Sextant),
            ("quadrant", CliGlyphs::Quadrant),
        ] {
            let args = Args::try_parse_from(["rthid", "--glyphs", value]).unwrap();
            assert_eq!(args.glyphs, Some(expected));
        }
        let args = Args::try_parse_from([
            "rthid",
            "photo.png",
            "--graphics",
            "off",
            "--glyphs",
            "quadrant",
            "--preview-png",
            "dots.png",
            "--config",
            "presets.toml",
        ])
        .unwrap();
        assert_eq!(args.image, Some(PathBuf::from("photo.png")));
        assert_eq!(args.config, Some(PathBuf::from("presets.toml")));
        assert_eq!(args.preview_png, Some(PathBuf::from("dots.png")));
        assert_eq!(args.graphics, CliGraphics::Off);
        assert_eq!(args.glyphs, Some(CliGlyphs::Quadrant));
    }

    #[test]
    fn cli_rejects_invalid_arguments() {
        for args in [
            vec!["--graphics", "kitty"],
            vec!["--glyphs", "unknown"],
            vec!["--preview-png", ""],
            vec!["--config", ""],
            vec!["a", "b"],
        ] {
            assert!(
                Args::try_parse_from(std::iter::once("rthid").chain(args.iter().copied())).is_err(),
                "{args:?}"
            );
        }
    }

    #[test]
    fn pointer_motion_alone_does_not_request_redraw() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
        let mouse = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: 1,
                row: 2,
                modifiers: KeyModifiers::NONE,
            })
        };
        let moved = mouse(MouseEventKind::Moved);
        assert!(!needs_redraw(&moved));
        for event in [
            mouse(MouseEventKind::ScrollDown),
            Event::Resize(80, 24),
            Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
        ] {
            assert!(needs_redraw(&event));
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
