mod app;
/// Companion PNG export and status.
mod export;
/// Terminal graphics detection, rendering and cleanup.
mod graphics;
/// Single entry caches with build versions.
mod memo;
mod ui;
mod view;

use std::{
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

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

fn parse_args(
    args: impl IntoIterator<Item = OsString>,
) -> io::Result<(PathBuf, Option<PathBuf>, Option<Glyphs>, Graphics)> {
    let mut args = args.into_iter();
    let mut source = None;
    let mut preview = None;
    let mut glyphs = None;
    let mut graphics = None;
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Usage: rthid [image] [--preview-png path] [--glyphs sextant|quadrant] [--graphics auto|off]",
        )
    };
    fn choice<T: Copy>(
        slot: &mut Option<T>,
        next: Option<OsString>,
        choices: &[(&str, T)],
    ) -> Option<()> {
        if slot.is_some() {
            return None;
        }
        let next = next?;
        *slot = Some(choices.iter().find(|(name, _)| next == *name)?.1);
        Some(())
    }
    while let Some(arg) = args.next() {
        if arg == "--preview-png" {
            if preview.is_some() {
                return Err(invalid());
            }
            let value = args.next().ok_or_else(invalid)?;
            if value.is_empty() || value.as_encoded_bytes().starts_with(b"-") {
                return Err(invalid());
            }
            preview = Some(PathBuf::from(value));
        } else if arg == "--graphics" {
            choice(
                &mut graphics,
                args.next(),
                &[("auto", Graphics::Auto), ("off", Graphics::Off)],
            )
            .ok_or_else(invalid)?;
        } else if arg == "--glyphs" {
            choice(
                &mut glyphs,
                args.next(),
                &[("sextant", Glyphs::Sextant), ("quadrant", Glyphs::Quadrant)],
            )
            .ok_or_else(invalid)?;
        } else if arg.as_encoded_bytes().starts_with(b"-") || source.is_some() {
            return Err(invalid());
        } else {
            source = Some(PathBuf::from(arg));
        }
    }
    Ok((
        source.unwrap_or_else(|| "tests/fixtures/hsl.jpg".into()),
        preview,
        glyphs,
        graphics.unwrap_or_default(),
    ))
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
    let (path, preview_png, glyphs, graphics) = parse_args(std::env::args_os().skip(1))?;
    check_not_source(&path, preview_png.as_deref())?;
    let mut debug_log = std::env::var_os("RTHID_DEBUG_LOG")
        .map(|path| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        })
        .transpose()?;
    let source = match image::open(&path) {
        Ok(image) if image.width().max(image.height()) > MAX_SOURCE_SIDE => image
            .resize(
                MAX_SOURCE_SIDE,
                MAX_SOURCE_SIDE,
                imageops::FilterType::Triangle,
            )
            .to_rgba8(),
        Ok(image) => image.to_rgba8(),
        Err(error) => {
            eprintln!("Could not load {}: {error}", path.display());
            std::process::exit(1);
        }
    };
    let mut app = App::new(source, preview_png);
    let term_program = std::env::var("TERM_PROGRAM").ok();
    app.view.glyphs = glyphs.unwrap_or_else(|| Glyphs::for_terminal(term_program.as_deref()));
    let mouse_capture = MouseCapture;
    let mut renderer = ui::Renderer::default();
    let terminal_result = ratatui::run(|terminal| {
        if let Some((graphics, cleanup)) = graphics::detect(graphics, term_program.as_deref()) {
            renderer.graphics = Some(graphics);
            renderer.kitty_cleanup = cleanup;
        }
        execute!(io::stdout(), EnableMouseCapture)?;
        let size = terminal.size()?;
        let mut layout = ui::layout(ratatui::layout::Rect::new(0, 0, size.width, size.height));
        let mut redraw = true;
        while !app.quit {
            if redraw {
                app.sync(&layout);
                let frame = terminal.draw(|frame| ui::draw(frame, &app, &layout, &mut renderer))?;
                if let Some(log) = &mut debug_log {
                    ui::log_minimap(&app, log, &frame, &layout, &mut renderer)?;
                }
            }
            redraw = false;
            // With a companion PNG, wake periodically so background write results reach the status line.
            if app.export.is_some() && !event::poll(Duration::from_millis(50))? {
                redraw = app.export.as_mut().is_some_and(export::Export::poll);
                continue;
            }
            // Drain queued input so held-down repeats cost one re-dither, not one each.
            loop {
                let event = event::read()?;
                if let Some(log) = &mut debug_log {
                    writeln!(log, "event {event:?}")?;
                }
                redraw |= needs_redraw(&event);
                let geometry = view::geometry(&app.view, app.grid_size(), layout.preview);
                match event {
                    Event::Key(key) => app.handle_key(key, &geometry),
                    Event::Mouse(mouse) => app.handle_mouse(mouse, &geometry),
                    Event::Resize(width, height) => {
                        layout = ui::layout(ratatui::layout::Rect::new(0, 0, width, height));
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
    drop(renderer);
    drop(mouse_capture);
    let png_result = app.finish_png();
    terminal_result.and(png_result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn cli_selects_graphics_and_rejects_invalid_or_duplicate_values() {
        let parse = |args: &[&str]| parse_args(args.iter().map(OsString::from));
        assert_eq!(parse(&[]).unwrap().3, Graphics::Auto);
        for (value, expected) in [("auto", Graphics::Auto), ("off", Graphics::Off)] {
            assert_eq!(
                parse(&["photo.png", "--graphics", value, "--glyphs", "quadrant"])
                    .unwrap()
                    .3,
                expected
            );
        }
        for args in [
            vec!["--graphics"],
            vec!["--graphics", ""],
            vec!["--graphics", "kitty"],
            vec!["--graphics", "Auto"],
            vec!["--graphics", "off", "--graphics", "auto"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn cli_selects_glyphs_and_rejects_invalid_or_duplicate_values() {
        let parse = |args: &[&str]| parse_args(args.iter().map(OsString::from));
        assert_eq!(parse(&[]).unwrap().2, None);
        for (value, expected) in [
            ("sextant", view::Glyphs::Sextant),
            ("quadrant", view::Glyphs::Quadrant),
        ] {
            for args in [
                vec!["--glyphs", value],
                vec!["photo.png", "--glyphs", value, "--preview-png", "dots.png"],
                vec!["--preview-png", "dots.png", "--glyphs", value, "photo.png"],
            ] {
                assert_eq!(parse(&args).unwrap().2, Some(expected));
            }
        }
        for args in [
            vec!["--glyphs"],
            vec!["--glyphs", ""],
            vec!["--glyphs", "unknown"],
            vec!["--glyphs", "Quadrant"],
            vec!["--glyphs", "--preview-png"],
            vec!["--glyphs", "quadrant", "--glyphs", "sextant"],
            vec!["--glyphs", "sextant", "--glyphs", "sextant"],
        ] {
            assert_eq!(
                parse(&args).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
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
    fn cli_accepts_image_and_companion_and_rejects_invalid_arguments() {
        let parse = |args: &[&str]| parse_args(args.iter().map(OsString::from));
        assert_eq!(
            parse(&[]).unwrap(),
            ("tests/fixtures/hsl.jpg".into(), None, None, Graphics::Auto)
        );
        assert_eq!(
            parse(&["photo.png", "--preview-png", "dots.png"]).unwrap(),
            (
                "photo.png".into(),
                Some("dots.png".into()),
                None,
                Graphics::Auto
            )
        );
        assert_eq!(
            parse(&["--preview-png", "dots.png", "photo.png"]).unwrap(),
            (
                "photo.png".into(),
                Some("dots.png".into()),
                None,
                Graphics::Auto
            )
        );
        for args in [
            vec!["--preview-png"],
            vec!["--wat"],
            vec!["a", "b"],
            vec!["--preview-png", "a", "--preview-png", "b"],
            vec!["--preview-png", "--wat"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
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
