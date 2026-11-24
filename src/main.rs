mod app;
mod ui;

use std::time::Duration;

use image::imageops;
use ratatui::crossterm::event::{self, Event};

use crate::app::App;

// Sources are shrunk on load to bound dither work per keypress.
// Keep the original around once export needs full resolution.
const MAX_SOURCE_SIDE: u32 = 1024;

fn main() -> std::io::Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .unwrap_or_else(|| "tests/fixtures/hsl.jpg".into());
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
            eprintln!(
                "Could not load {}: {error}",
                std::path::Path::new(&path).display()
            );
            std::process::exit(1);
        }
    };
    let mut app = App::new(source);
    ratatui::run(|terminal| {
        while !app.quit {
            app.refresh();
            terminal.draw(|frame| app.draw(frame))?;
            // Drain queued keys so held-down repeats cost one re-dither, not one each.
            loop {
                if let Event::Key(key) = event::read()? {
                    app.handle_key(key);
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        Ok(())
    })
}
