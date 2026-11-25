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
    time::{Duration, Instant},
};

use clap::{Parser, ValueEnum};
#[cfg(test)]
use image::RgbaImage;
use ratatui::DefaultTerminal;
use ratatui::crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, MouseEventKind,
    },
    execute,
};
use rthid_io::{
    dir::list_dir,
    image::{check_not_source, load_image},
    path::expand_tilde,
};

use crate::{
    app::{App, Request},
    cells::Glyphs,
    graphics::Mode as Graphics,
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

fn handle_request(
    app: &mut App,
    request: Request,
    preview_png: Option<&Path>,
    mut debug_log: Option<&mut File>,
) -> io::Result<()> {
    let request = match request {
        Request::Open(path, dir) => {
            let home_path = path == Path::new("~") || path.starts_with("~/");
            let home = std::env::var_os("HOME").map(PathBuf::from);
            let path = path
                .to_str()
                .map(|text| expand_tilde(text, home.as_deref()))
                .unwrap_or(path);
            let path = if path.is_relative() && !home_path {
                dir.map_or_else(|| path.clone(), |dir| dir.join(&path))
            } else {
                path
            };
            if path.is_dir() {
                if !matches!(app.modal, Some(modal::Modal::OpenFile(_))) {
                    app.modal = Some(modal::Modal::open_file(path.clone()));
                }
                Request::List(path)
            } else {
                Request::Open(path, None)
            }
        }
        request => request,
    };
    match request {
        Request::List(path) => {
            let started = Instant::now();
            let result = list_dir(&path);
            if let Some(log) = &mut debug_log {
                writeln!(
                    log,
                    "[timing] list {} {}",
                    started.elapsed().as_millis(),
                    path.display()
                )?;
            }
            match result {
                Ok((dir, entries)) => app.set_listing(dir, entries),
                Err(error) => {
                    app.set_notice(format!("open: {}: {error}", path.display()));
                }
            }
        }
        Request::Open(path, _) => {
            let started = Instant::now();
            let result = check_not_source(&path, preview_png)
                .map_err(|error| error.to_string())
                .and_then(|()| load_image(&path));
            if let Some(log) = &mut debug_log {
                writeln!(
                    log,
                    "[timing] decode {} {}",
                    started.elapsed().as_millis(),
                    path.display()
                )?;
            }
            match result {
                Ok(image) => app.open_image(path, image),
                Err(error) => {
                    app.set_notice(format!("open: {error}"));
                }
            }
        }
    }
    Ok(())
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
    let source = if let Some(path) = path {
        let started = Instant::now();
        let image = load_image(&path);
        if let Some(log) = &mut debug_log {
            writeln!(
                log,
                "[timing] decode {} {}",
                started.elapsed().as_millis(),
                path.display()
            )?;
        }
        let image = image.unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(1);
        });
        Some((path, image))
    } else {
        None
    };
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
            poll_export(app, &mut debug_log)?;
            let started = Instant::now();
            let rebuilt = app.sync(layout.preview);
            if rebuilt && let Some(log) = &mut debug_log {
                writeln!(log, "[timing] sync {}", started.elapsed().as_millis())?;
            }
            let frame = terminal.draw(|frame| {
                ui::draw(frame, app, &layout, renderer);
            })?;
            if let Some(log) = &mut debug_log {
                ui::log_minimap(app, log, &frame, &app.geometry(layout.preview))?;
            }
        }
        if let Some(request) = app.take_request() {
            handle_request(app, request, preview_png, debug_log.as_deref_mut())?;
            redraw = true;
            continue;
        }
        redraw = false;
        // With a companion PNG, wake periodically so background write results reach the status line.
        if app.has_export() && !event::poll(Duration::from_millis(50))? {
            redraw = poll_export(app, &mut debug_log)?;
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
                Event::Mouse(mouse) => {
                    app.handle_mouse(mouse, &app.geometry(layout.preview), layout.screen)
                }
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
        {
            let started = Instant::now();
            let result = store::save(path, &app.palettes, &app.templates);
            if let Some(log) = &mut debug_log {
                writeln!(log, "[timing] save {}", started.elapsed().as_millis())?;
            }
            if let Err(error) = result {
                redraw |= app.set_notice(format!("config: {error}"));
            }
        }
    }
    Ok(())
}

fn poll_export(app: &mut App, debug_log: &mut Option<&mut File>) -> io::Result<bool> {
    let changed = app.poll_export();
    if let Some(elapsed) = app.take_encode_elapsed()
        && let Some(log) = debug_log
    {
        writeln!(log, "[timing] encode {}", elapsed.as_millis())?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn request_timings_include_operation_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("photo.png");
        RgbaImage::new(2, 2).save(&image).unwrap();
        let log_path = dir.path().join("debug.log");
        let mut log = File::create(&log_path).unwrap();
        let mut app = App::empty();
        for request in [
            Request::List(dir.path().into()),
            Request::Open(image.clone(), None),
        ] {
            handle_request(&mut app, request, None, Some(&mut log)).unwrap();
        }
        let text = std::fs::read_to_string(log_path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        for (line, operation, path) in [
            (lines[0], "list", dir.path()),
            (lines[1], "decode", image.as_path()),
        ] {
            let rest = line
                .strip_prefix(&format!("[timing] {operation} "))
                .unwrap();
            let (milliseconds, logged_path) = rest.split_once(' ').unwrap();
            milliseconds.parse::<u128>().unwrap();
            assert_eq!(logged_path, path.display().to_string());
        }
    }

    #[test]
    fn open_requests_list_directories_and_keep_failures_open() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = App::empty();
        app.handle_paste(&format!("'{}'", temp.path().display()));
        assert!(app.modal.is_none());
        assert!(app.notice().unwrap().starts_with("Loading "));
        let request = app.take_request().unwrap();
        handle_request(&mut app, request, None, None).unwrap();
        assert!(app.notice().is_none());
        let Some(modal::Modal::OpenFile(picker)) = &app.modal else {
            panic!("open dialog expected")
        };
        assert_eq!(picker.dir, temp.path().canonicalize().unwrap());
        handle_request(
            &mut app,
            Request::Open(
                PathBuf::from("missing.png"),
                Some(temp.path().to_path_buf()),
            ),
            None,
            None,
        )
        .unwrap();
        assert!(matches!(app.modal, Some(modal::Modal::OpenFile(_))));
        assert!(app.notice().unwrap().contains("Could not load"));
        assert!(
            app.notice()
                .unwrap()
                .contains(&temp.path().display().to_string())
        );
        let photo = temp.path().join("photo.png");
        RgbaImage::new(2, 2).save(&photo).unwrap();
        for path in [PathBuf::from("photo.png"), photo.clone()] {
            handle_request(
                &mut app,
                Request::Open(path, Some(temp.path().to_path_buf())),
                None,
                None,
            )
            .unwrap();
            assert_eq!(app.source_path(), Some(photo.as_path()));
        }
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
}
