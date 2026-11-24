use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::widgets::{Block, Paragraph};

fn main() -> std::io::Result<()> {
    ratatui::run(|terminal| {
        loop {
            terminal.draw(|f| {
                f.render_widget(
                    Paragraph::new("q: quit").block(Block::bordered().title("rthid")),
                    f.area(),
                )
            })?;
            if let Event::Key(k) = event::read()?
                && k.code == KeyCode::Char('q')
            {
                return Ok(());
            }
        }
    })
}
