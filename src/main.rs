/// Interactive state and image processing.
mod app;
/// Companion PNG export and status.
mod export;
/// Terminal graphics detection, rendering and cleanup.
mod graphics;
/// Single entry caches with build versions.
mod memo;
/// Terminal layout and rendering.
mod ui;
/// Preview coordinates and cell rendering.
mod view;

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use clap::{Parser, ValueEnum};
use image::imageops;
use ratatui::crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, MouseEventKind},
    execute,
};

use crate::{app::App, graphics::Mode as Graphics, view::Glyphs};

struct MouseCapture;

impl Drop for MouseCapture {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

// Sources are shrunk on load to bound dither work per keypress.
// Keep the original around once export needs full resolution.
const MAX_SOURCE_SIDE: u32 = 1024;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// Source image to preview.
    #[arg(default_value = "tests/fixtures/hsl.jpg")]
    image: PathBuf,
    /// Save the dithered preview to a PNG file.
    #[arg(long, value_name = "PATH")]
    preview_png: Option<PathBuf>,
    /// Glyph set for the dithered overview and minimap.
    #[arg(long, value_enum)]
    glyphs: Option<CliGlyphs>,
    /// Detect terminal graphics or use glyph rendering.
    #[arg(long, value_enum, default_value = "auto")]
    graphics: CliGraphics,
}

// TODO: These enums mirror view::Glyphs and graphics::Mode to keep clap out of lower modules.
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

fn needs_redraw(event: &Event) -> bool {
    !matches!(event, Event::Mouse(mouse) if mouse.kind == MouseEventKind::Moved)
}

fn main() -> std::io::Result<()> {
    let Args {
        image: path,
        preview_png,
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
    check_not_source(&path, preview_png.as_deref())?;
    let mut debug_log = std::env::var_os("RTHID_DEBUG_LOG")
        .map(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        })
        .transpose()?;
    let source = image::open(&path).unwrap_or_else(|error| {
        eprintln!("Could not load {}: {error}", path.display());
        std::process::exit(1);
    });
    let source = if source.width().max(source.height()) > MAX_SOURCE_SIDE {
        source.resize(
            MAX_SOURCE_SIDE,
            MAX_SOURCE_SIDE,
            imageops::FilterType::Triangle,
        )
    } else {
        source
    }
    .to_rgba8();
    let mut app = App::new(source, preview_png);
    let term_program = std::env::var("TERM_PROGRAM").ok();
    app.view.glyphs = glyphs.unwrap_or_else(|| Glyphs::for_terminal(term_program.as_deref()));
    let mouse_capture = MouseCapture;
    // TODO: Extract an event_loop function that takes its dependencies explicitly.
    let terminal_result = ratatui::run(|terminal| {
        let mut renderer = ui::Renderer::new(graphics::detect(graphics, term_program.as_deref()));
        execute!(io::stdout(), EnableMouseCapture)?;
        let mut layout = ui::layout(terminal.size()?);
        let mut redraw = true;
        while !app.quit {
            if redraw {
                app.poll_export();
                app.sync(layout.preview);
                let frame = terminal.draw(|frame| {
                    ui::draw(frame, &app, &layout, &mut renderer);
                })?;
                if let Some(log) = &mut debug_log {
                    ui::log_minimap(&app, log, &frame, &app.geometry(layout.preview))?;
                }
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
                    Event::Resize(width, height) => {
                        layout = ui::layout(ratatui::layout::Size::new(width, height));
                        app.clamp_view(layout.preview);
                    }
                    _ => {}
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        Ok(())
    });
    drop(mouse_capture);
    let png_result = app.finish_png();
    terminal_result.and(png_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_defaults_and_definition() {
        Args::command().debug_assert();
        let args = Args::try_parse_from(["rthid"]).unwrap();
        assert_eq!(args.image, PathBuf::from("tests/fixtures/hsl.jpg"));
        assert_eq!(args.preview_png, None);
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
            for args in [
                vec!["rthid", "--glyphs", value],
                vec![
                    "rthid",
                    "photo.png",
                    "--glyphs",
                    value,
                    "--preview-png",
                    "dots.png",
                ],
                vec![
                    "rthid",
                    "--preview-png",
                    "dots.png",
                    "--glyphs",
                    value,
                    "photo.png",
                ],
            ] {
                let parsed = Args::try_parse_from(&args).unwrap();
                assert_eq!(parsed.glyphs, Some(expected));
                if args.len() > 3 {
                    assert_eq!(parsed.image, PathBuf::from("photo.png"));
                    assert_eq!(parsed.preview_png, Some(PathBuf::from("dots.png")));
                }
            }
        }
        let args = Args::try_parse_from(["rthid", "--glyphs=quadrant"]).unwrap();
        assert!(matches!(args.glyphs, Some(CliGlyphs::Quadrant)));
    }

    #[test]
    fn cli_rejects_invalid_arguments() {
        for args in [
            vec!["--preview-png"],
            vec!["--preview-png", ""],
            vec!["--preview-png="],
            vec!["--preview-png", "-dots.png"],
            vec!["--preview-png", "--wat"],
            vec!["--preview-png", "a", "--preview-png", "b"],
            vec!["--graphics"],
            vec!["--graphics", ""],
            vec!["--graphics", "kitty"],
            vec!["--graphics", "Auto"],
            vec!["--graphics", "off", "--graphics", "auto"],
            vec!["--glyphs"],
            vec!["--glyphs", ""],
            vec!["--glyphs", "unknown"],
            vec!["--glyphs", "Quadrant"],
            vec!["--glyphs", "--preview-png"],
            vec!["--glyphs", "quadrant", "--glyphs", "sextant"],
            vec!["--glyphs", "sextant", "--glyphs", "sextant"],
            vec!["--wat"],
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
            assert!([&moved, &event, &moved].into_iter().any(needs_redraw));
        }
    }

    #[test]
    fn preview_png_may_not_overwrite_the_source() {
        let dir = std::env::temp_dir().join(format!("rthid-cli-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("photo.png");
        std::fs::write(&source, b"image").unwrap();
        let aliased = dir.join(".").join("photo.png");
        assert!(check_not_source(&source, Some(&aliased)).is_err());
        assert!(check_not_source(&source, Some(&dir.join("dots.png"))).is_ok());
        assert!(check_not_source(&source, None).is_ok());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
