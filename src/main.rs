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
    fmt::Display,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use clap::{Parser, ValueEnum};
use ratatui::crossterm::event::{self, Event};
use rthid_io::{
    image::{check_not_source, load_image},
    io::{Done, IO, Outcome},
};
use rthid_term::{BufferedTerminal, SessionOptions, run_terminal_with_options, synchronized_draw};

use crate::{app::App, cells::Glyphs, graphics::Mode as Graphics, ui::Renderer};

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

// Writes one timing line in milliseconds, followed by the path when there is one.
fn log_timing(
    debug_log: &mut Option<&mut File>,
    op: impl Display,
    elapsed: Duration,
    path: Option<&Path>,
) -> io::Result<()> {
    if let Some(log) = debug_log {
        let ms = elapsed.as_millis();
        match path {
            Some(path) => writeln!(log, "[timing] {op} {ms} {}", path.display())?,
            None => writeln!(log, "[timing] {op} {ms}")?,
        }
    }
    Ok(())
}

fn log_done(done: &Done, debug_log: &mut Option<&mut File>) -> io::Result<()> {
    log_timing(debug_log, done.op(), done.elapsed, Some(done.path()))
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
        check_not_source(path, preview_png.as_deref())
            .map_err(|cause| io::Error::new(io::ErrorKind::InvalidInput, cause.to_string()))?;
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
        log_timing(
            &mut debug_log.as_mut(),
            "decode",
            started.elapsed(),
            Some(&path),
        )?;
        let image = image.unwrap_or_else(|error| {
            eprintln!("Could not load {}: {error}", path.display());
            std::process::exit(1);
        });
        Some((path, image))
    } else {
        None
    };
    let config_path = store::config_path(config);
    let loaded = store::load_or_builtin(config_path.as_deref());
    let mut app = App::new(source, preview_png.clone(), loaded);
    let term_program = std::env::var("TERM_PROGRAM").ok();
    app.glyphs = glyphs.unwrap_or_else(|| Glyphs::for_terminal(term_program.as_deref()));
    let mut io = IO::new(std::env::var_os("HOME").map(PathBuf::from))?;
    let worker_panic = Arc::new(Mutex::new(None::<String>));
    #[allow(clippy::needless_update, reason = "Keep future input modes disabled.")]
    let options = SessionOptions {
        mouse_capture: true,
        bracketed_paste: true,
        ..SessionOptions::default()
    };
    let mut terminal_result = run_terminal_with_options(options, |terminal| {
        let panic_log = debug_log.as_ref().map(File::try_clone).transpose()?;
        let worker_panic = Arc::clone(&worker_panic);
        let previous = std::panic::take_hook();
        // TODO: Worker panics already arrive as `Cause::Panicked` in `Done`. Keep this hook only to
        //       silence stderr for the worker thread and report the panic from that result.
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().name() == Some(rthid_io::io::WORKER_NAME) {
                let message = rthid_io::io::panic_message(info.payload());
                *worker_panic.lock().unwrap() = Some(message.lines().next().unwrap_or("").into());
                if let Some(mut log) = panic_log.as_ref() {
                    let _ = writeln!(log, "IO worker panic: {info}");
                }
            } else {
                previous(info);
            }
        }));
        // Detection queries stdout directly, before any frame output is buffered.
        let mut renderer = ui::Renderer::new(graphics::detect(graphics, term_program.as_deref()));
        event_loop(
            terminal,
            &mut app,
            &mut renderer,
            debug_log.as_mut(),
            config_path.as_deref(),
            &mut io,
        )
    });
    app.finish();
    for (job, tag) in app.take_jobs() {
        io.submit(job, tag);
    }
    for done in io.finish() {
        terminal_result = terminal_result.and(log_done(&done, &mut debug_log.as_mut()));
        if matches!(done.outcome, Outcome::Exported(_) | Outcome::Updated(_)) {
            app.complete(done);
        }
    }
    // Edits whose background save did not finish are saved here.
    terminal_result = terminal_result.and(save_on_quit(
        config_path.as_deref(),
        &app.library.unsaved(),
        &mut debug_log.as_mut(),
    ));
    if debug_log.is_none()
        && let Some(message) = worker_panic.lock().unwrap().as_ref()
    {
        eprintln!("IO worker panicked: {message}");
    }
    terminal_result.and(app.export_result())
}

fn save_on_quit(
    path: Option<&Path>,
    pending: &[store::Change],
    debug_log: &mut Option<&mut File>,
) -> io::Result<()> {
    if !pending.is_empty()
        && let Some(path) = path
    {
        let started = Instant::now();
        let result = store::save(path, Duration::from_secs(1), pending);
        log_timing(debug_log, "save", started.elapsed(), Some(path))?;
        result?;
    }
    Ok(())
}

// Syncs and draws when something changed, then waits for input and applies
// every queued event before the next draw. Runs until the app requests quit.
fn event_loop(
    terminal: &mut BufferedTerminal,
    app: &mut App,
    renderer: &mut Renderer,
    mut debug_log: Option<&mut File>,
    config_path: Option<&Path>,
    io: &mut IO,
) -> io::Result<()> {
    let mut layout = ui::layout(terminal.size()?);
    let mut redraw = true;
    while !app.quit {
        while let Some(done) = io.poll() {
            log_done(&done, &mut debug_log)?;
            redraw |= app.complete(done);
        }
        if redraw {
            let started = Instant::now();
            let rebuilt = app.sync(layout.preview);
            if rebuilt {
                log_timing(&mut debug_log, "sync", started.elapsed(), None)?;
            }
            synchronized_draw(terminal, |terminal| {
                let frame = terminal.draw(|frame| {
                    ui::draw(frame, app, &layout, renderer);
                })?;
                if let Some(log) = &mut debug_log {
                    ui::log_minimap(app, log, &frame, &app.geometry(layout.preview))?;
                }
                Ok(())
            })?;
        }
        for (job, tag) in app.take_jobs() {
            io.submit(job, tag);
        }
        redraw = false;
        // TODO: The 30 ms tick only polls for worker results. Have the worker wake the event loop
        //       when a job finishes instead.
        let timeout = [
            app.library
                .retry_at()
                .map(|deadline| deadline.saturating_duration_since(Instant::now())),
            io.busy().then_some(Duration::from_millis(30)),
        ]
        .into_iter()
        .flatten()
        .min();
        if timeout.map_or(Ok(true), event::poll)? {
            // Drain queued input so held-down repeats cost one re-dither, not one each.
            loop {
                let event = event::read()?;
                if let Some(log) = &mut debug_log {
                    writeln!(log, "event {event:?}")?;
                }
                match &event {
                    Event::Key(key) => app.handle_key(*key, &app.geometry(layout.preview)),
                    Event::Mouse(mouse) => {
                        redraw |=
                            app.handle_mouse(*mouse, &app.geometry(layout.preview), layout.screen);
                    }
                    Event::Paste(text) => app.handle_paste(text),
                    Event::Resize(width, height) => {
                        layout = ui::layout(ratatui::layout::Size::new(*width, *height));
                        app.clamp_viewport(layout.preview);
                    }
                    _ => {}
                }
                redraw |= !matches!(event, Event::Mouse(_));
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        // Quitting saves synchronously instead, so no job is left behind.
        if !app.quit
            && let Some(path) = config_path
        {
            app.queue_save(path, Instant::now());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn quit_retries_pending_changes_and_returns_save_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let changes = vec![store::Change::UpsertTemplate(settings::Template::capture(
            "Pending".into(),
            &settings::Settings::default(),
        ))];
        std::fs::write(&path, "version = [").unwrap();
        assert!(store::save(&path, Duration::from_millis(50), &changes).is_err());
        assert!(save_on_quit(Some(&path), &changes, &mut None).is_err());
        std::fs::write(&path, "version = 1").unwrap();
        save_on_quit(Some(&path), &changes, &mut None).unwrap();
        assert!(store::load(&path).templates.find("Pending").is_some());
    }

    #[test]
    fn completion_timings_include_operation_duration_and_path() {
        use rthid_io::{
            dir::Listing,
            io::{Opened, Updated},
        };
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("debug.log");
        let mut log = File::create(&log_path).unwrap();
        let path = PathBuf::from("photo.png");
        for outcome in [
            Outcome::Listed(Ok(Listing {
                dir: path.clone(),
                entries: vec![],
            })),
            Outcome::Opened(Ok(Opened::Image {
                path: path.clone(),
                image: image::RgbaImage::new(1, 1),
            })),
            Outcome::Exported(Ok(path.clone())),
            Outcome::Updated(Ok(Updated {
                path,
                text: String::new(),
            })),
        ] {
            log_done(
                &Done::new(0, Duration::from_millis(7), outcome),
                &mut Some(&mut log),
            )
            .unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(log_path).unwrap(),
            "[timing] list 7 photo.png\n[timing] decode 7 photo.png\n[timing] encode 7 photo.png\n[timing] save 7 photo.png\n"
        );
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
    fn pointer_motion_after_drag_requests_redraw() {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let mut app = App::drag_fixture();
        let layout = ui::layout(ratatui::layout::Size::new(100, 24));
        for (kind, column, dragging, redraw) in [
            (MouseEventKind::Down(MouseButton::Left), 10, false, false),
            (MouseEventKind::Drag(MouseButton::Left), 9, true, true),
            (MouseEventKind::Drag(MouseButton::Left), 9, true, false),
            (MouseEventKind::Moved, 9, false, true),
            (MouseEventKind::Moved, 10, false, false),
            (MouseEventKind::Up(MouseButton::Left), 10, false, false),
            (MouseEventKind::ScrollUp, 10, false, false),
            (MouseEventKind::ScrollDown, 10, false, true),
            (MouseEventKind::ScrollRight, 10, false, true),
        ] {
            let changed = app.mouse_at(
                kind,
                column,
                layout.preview.y + 1,
                &app.geometry(layout.preview),
                layout.screen,
            );
            assert_eq!(app.dragging(), dragging);
            assert_eq!(changed, redraw);
        }
    }
}
