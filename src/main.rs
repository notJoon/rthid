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
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use clap::{Parser, ValueEnum};
use ratatui::DefaultTerminal;
use ratatui::crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, MouseEventKind,
    },
    execute,
};
use rthid_io::{
    image::{check_not_source, load_image},
    io::{Done, IO},
};

use crate::{app::App, cells::Glyphs, graphics::Mode as Graphics, ui::Renderer};

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

fn log_done(done: &Done, debug_log: &mut Option<&mut File>) -> io::Result<()> {
    if let Some(log) = debug_log {
        writeln!(
            log,
            "[timing] {} {} {}",
            done.op(),
            done.elapsed.as_millis(),
            done.path().display()
        )?;
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
        if let Some(log) = &mut debug_log {
            writeln!(
                log,
                "[timing] decode {} {}",
                started.elapsed().as_millis(),
                path.display()
            )?;
        }
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
    let notice = loaded.notice();
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
    let mut io = IO::new(std::env::var_os("HOME").map(PathBuf::from))?;
    let mouse_capture = MouseCapture;
    let bracketed_paste = BracketedPaste;
    let worker_panic = Arc::new(Mutex::new(None::<String>));
    let mut pending = Vec::new();
    let mut terminal_result = ratatui::run(|terminal| {
        let panic_log = debug_log.as_ref().map(File::try_clone).transpose()?;
        let worker_panic = Arc::clone(&worker_panic);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().name() == Some(rthid_io::io::WORKER_NAME) {
                let message = info
                    .payload()
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| info.payload().downcast_ref::<&str>().copied())
                    .unwrap_or("unknown panic");
                *worker_panic.lock().unwrap() = Some(message.lines().next().unwrap_or("").into());
                if let Some(mut log) = panic_log.as_ref() {
                    let _ = writeln!(log, "IO worker panic: {info}");
                }
            } else {
                previous(info);
            }
        }));
        let mut renderer = ui::Renderer::new(graphics::detect(graphics, term_program.as_deref()));
        execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
        event_loop(
            terminal,
            &mut app,
            &mut renderer,
            debug_log.as_mut(),
            config_path.as_deref(),
            &mut io,
            &mut pending,
        )
    });
    drop(bracketed_paste);
    drop(mouse_capture);
    terminal_result = terminal_result.and(save_on_quit(
        config_path.as_deref(),
        &pending,
        &mut debug_log.as_mut(),
    ));
    app.finish();
    for job in app.take_jobs() {
        io.submit(job);
    }
    for done in io.finish() {
        terminal_result = terminal_result.and(log_done(&done, &mut debug_log.as_mut()));
        if matches!(done.outcome, rthid_io::io::Outcome::Exported(_)) {
            app.complete(done);
        }
    }
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
        if let Some(log) = debug_log {
            writeln!(log, "[timing] save {}", started.elapsed().as_millis())?;
        }
        result?;
    }
    Ok(())
}

// Keep only the last edit for each list and name while a save is pending.
fn queue_changes(pending: &mut Vec<store::Change>, changes: Vec<store::Change>) {
    for change in changes {
        pending.retain(|older| older.key() != change.key());
        pending.push(change);
    }
}

// Syncs and draws when something changed, then waits for input and applies
// every queued event before the next draw. Runs until the app requests quit.
fn event_loop(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    renderer: &mut Renderer,
    mut debug_log: Option<&mut File>,
    config_path: Option<&Path>,
    io: &mut IO,
    pending: &mut Vec<store::Change>,
) -> io::Result<()> {
    let mut layout = ui::layout(terminal.size()?);
    let mut redraw = true;
    let mut retry_at: Option<Instant> = None;
    while !app.quit {
        while let Some(done) = io.poll() {
            log_done(&done, &mut debug_log)?;
            redraw |= app.complete(done);
        }
        if redraw {
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
        for job in app.take_jobs() {
            let open = matches!(job, rthid_io::io::Job::Open { .. });
            let id = io.submit(job);
            if open {
                app.open_submitted(id);
            }
        }
        redraw = false;
        let timeout = retry_at.map(|deadline| deadline.saturating_duration_since(Instant::now()));
        let timeout = if io.busy() {
            Some(
                timeout
                    .unwrap_or(Duration::from_millis(30))
                    .min(Duration::from_millis(30)),
            )
        } else {
            timeout
        };
        if timeout.is_none() || event::poll(timeout.unwrap())? {
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
        }
        let changes = app.take_changes();
        let new_changes = !changes.is_empty();
        queue_changes(pending, changes);
        if !pending.is_empty()
            && (new_changes || retry_at.is_some_and(|deadline| Instant::now() >= deadline))
            && let Some(path) = config_path
        {
            let started = Instant::now();
            let result = store::save(path, Duration::from_millis(50), pending);
            if let Some(log) = &mut debug_log {
                writeln!(log, "[timing] save {}", started.elapsed().as_millis())?;
            }
            match result {
                Ok(loaded) => {
                    pending.clear();
                    retry_at = None;
                    app.saved(loaded);
                }
                Err(error) => {
                    retry_at = Some(Instant::now() + Duration::from_secs(2));
                    app.save_failed(Vec::new(), error.to_string());
                }
            }
            redraw = true;
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
    fn pending_saves_keep_the_last_change_per_list_and_name() {
        use store::Change;
        let mut pending = vec![
            Change::RemovePreset("Shared".into()),
            Change::RemoveTemplate("Shared".into()),
        ];
        let last = Change::UpsertTemplate(settings::Template::capture(
            "Shared".into(),
            &settings::Settings::default(),
        ));
        queue_changes(
            &mut pending,
            vec![last.clone(), Change::RemovePreset("Other".into())],
        );
        queue_changes(&mut pending, vec![Change::RemovePreset("Other".into())]);
        assert_eq!(
            pending,
            vec![
                Change::RemovePreset("Shared".into()),
                last,
                Change::RemovePreset("Other".into())
            ]
        );
    }

    #[test]
    fn completion_timings_include_operation_duration_and_path() {
        use rthid_io::io::{Listing, Opened, Outcome};
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
            Outcome::Exported(Ok(path)),
        ] {
            log_done(
                &Done {
                    id: 1,
                    elapsed: Duration::from_millis(7),
                    outcome,
                },
                &mut Some(&mut log),
            )
            .unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(log_path).unwrap(),
            "[timing] list 7 photo.png\n[timing] decode 7 photo.png\n[timing] encode 7 photo.png\n"
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
