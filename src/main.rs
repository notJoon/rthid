//! rthid previews dithering of an image in the terminal.
//!
//! The source is loaded and shrunk once. `pipeline::Pipeline` scales and rotates it into
//! the grid and, when Dithered or a companion PNG needs it, dithers the grid
//! into palette indices. `app::App` holds `params::Params` and the `view::View`. `ui::draw`
//! turns them into a frame through `render` (cell glyphs) or `graphics`
//! (terminal images), and `Export` writes the dithered grid to the companion PNG.
//!
//! Glossary:
//! - Grid: the scaled and rotated image. Its pixels are also called dots.
//! - Cell: one terminal character position.
//! - Half block: the `▀` glyph whose fg and bg colors show two stacked dots in one cell.
//! - Sextant and Quadrant: glyph sets for the dithered Overview and minimap that split a
//!   cell into 2x3 or 2x2 subcells painted in the cell fg or bg color.
//! - Overview: the whole grid fitted to the preview.
//! - 1:1 (`exact` in code): one dot per half cell, panned from the view origin.
//! - Minimap: an Overview inset shown in 1:1, dimmed outside the visible region.
//! - Approximate: some Overview cell sampled more than two palette colors, so
//!   the extra samples took the nearer kept color.
//! - Companion PNG: the optional `--preview-png` file, rewritten after each new dither.

/// Interactive state and input handling.
mod app;
/// Companion PNG writer and its status.
mod export;
/// Terminal image detection, placement and cleanup (Kitty, iTerm2, Sixel) for the Original Overview.
mod graphics;
/// Single entry caches with build versions.
mod memo;
/// Editable settings and palette presets.
mod params;
/// Image pipeline from source to grid to dithered indices.
mod pipeline;
/// Cell rendering for Overview, 1:1 and the minimap.
mod render;
/// Shared test fixtures.
#[cfg(test)]
mod test_support;
/// Screen layout, panels and status line.
mod ui;
/// Preview coordinates for Overview, 1:1 and the minimap.
mod view;

use std::{
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use clap::{Parser, ValueEnum};
use image::imageops;
use ratatui::DefaultTerminal;
use ratatui::crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, MouseEventKind},
    execute,
};

use crate::{app::App, graphics::Mode as Graphics, render::Glyphs, ui::Renderer};

/// Disables mouse capture on drop, so it is released even when the terminal session fails.
struct MouseCapture;

impl Drop for MouseCapture {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

// Sources are shrunk on load to bound dither work per keypress.
// Offering a full resolution export would require keeping the unshrunk original.
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

// TODO: These enums mirror render::Glyphs and graphics::Mode to keep clap out of lower modules.
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

// Pointer motion alone changes nothing on screen.
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
    let terminal_result = ratatui::run(|terminal| {
        let mut renderer = ui::Renderer::new(graphics::detect(graphics, term_program.as_deref()));
        execute!(io::stdout(), EnableMouseCapture)?;
        event_loop(terminal, &mut app, &mut renderer, debug_log.as_mut())
    });
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
        ])
        .unwrap();
        assert_eq!(args.image, PathBuf::from("photo.png"));
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
        let dir = crate::test_support::TempDir::new();
        let source = dir.join("photo.png");
        std::fs::write(&source, b"image").unwrap();
        let aliased = dir.join(".").join("photo.png");
        assert!(check_not_source(&source, Some(&aliased)).is_err());
        assert!(check_not_source(&source, Some(&dir.join("dots.png"))).is_ok());
        assert!(check_not_source(&source, None).is_ok());
    }
}
