mod app;
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

struct MouseCapture;

impl Drop for MouseCapture {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

use crate::{app::App, view::Glyphs};

// Sources are shrunk on load to bound dither work per keypress.
// Keep the original around once export needs full resolution.
const MAX_SOURCE_SIDE: u32 = 1024;

fn parse_args(
    args: impl IntoIterator<Item = OsString>,
) -> io::Result<(PathBuf, Option<PathBuf>, Option<Glyphs>)> {
    let mut args = args.into_iter();
    let mut source = None;
    let mut preview = None;
    let mut glyphs = None;
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Usage: rthid [image] [--preview-png path] [--glyphs sextant|quadrant]",
        )
    };
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
        } else if arg == "--glyphs" {
            if glyphs.is_some() {
                return Err(invalid());
            }
            glyphs = Some(
                match args.next().as_deref().and_then(|value| value.to_str()) {
                    Some("sextant") => Glyphs::Sextant,
                    Some("quadrant") => Glyphs::Quadrant,
                    _ => return Err(invalid()),
                },
            );
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
    let (path, preview_png, glyphs) = parse_args(std::env::args_os().skip(1))?;
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
    app.view.glyphs = glyphs
        .unwrap_or_else(|| Glyphs::for_terminal(std::env::var("TERM_PROGRAM").ok().as_deref()));
    let mouse_capture = MouseCapture;
    let terminal_result = ratatui::run(|terminal| {
        execute!(io::stdout(), EnableMouseCapture)?;
        let mut redraw = true;
        while !app.quit {
            if redraw {
                app.refresh();
                let frame = terminal.draw(|frame| app.draw(frame))?;
                if let Some(log) = &mut debug_log {
                    app.log_minimap(log, &frame)?;
                }
            }
            redraw = false;
            // With a companion PNG, wake periodically so background write results reach the status line.
            if app.preview_png.is_some() && !event::poll(Duration::from_millis(50))? {
                redraw = app.refresh();
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
                    Event::Key(key) => app.handle_key(key),
                    Event::Mouse(mouse) => app.handle_mouse(mouse),
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
    use std::ffi::OsString;

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
            ("tests/fixtures/hsl.jpg".into(), None, None)
        );
        assert_eq!(
            parse(&["photo.png", "--preview-png", "dots.png"]).unwrap(),
            ("photo.png".into(), Some("dots.png".into()), None)
        );
        assert_eq!(
            parse(&["--preview-png", "dots.png", "photo.png"]).unwrap(),
            ("photo.png".into(), Some("dots.png".into()), None)
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
