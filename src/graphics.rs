use std::{
    io::{self, Write},
    time::Duration,
};

use crate::{
    memo::Memo,
    view::{self, PreviewGeometry, View},
};
use image::RgbaImage;
use ratatui::style::Color;
use ratatui::{
    Frame,
    buffer::{Buffer, CellDiffOption},
    layout::Rect,
};
use ratatui_image::{
    Resize, StatefulImage,
    picker::{
        Capability, Picker, ProtocolType,
        cap_parser::{Parser, QueryStdioOptions},
    },
    protocol::{StatefulProtocol, StatefulProtocolType, kitty::StatefulKitty},
};

/// Controls whether terminal graphics are queried.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Detect a supported image protocol.
    #[default]
    Auto,
    /// Use glyph rendering without querying the terminal.
    Off,
}

/// Deletes the pinned Kitty image after terminal teardown.
pub struct KittyCleanup {
    tmux: bool,
}

impl Drop for KittyCleanup {
    fn drop(&mut self) {
        let (start, escape, end) = Parser::tmux_start_escape_end(self.tmux);
        let mut stdout = io::stdout().lock();
        let _ = write!(
            stdout,
            "{start}{escape}_Ga=d,d=I,i={KITTY_IMAGE_ID}{escape}\\{end}"
        )
        .and_then(|()| stdout.flush());
    }
}

type GraphicsCache = Memo<(u64, u32, u32, bool, Rect, bool), (Rect, StatefulProtocol)>;

/// Terminal image protocol, cached image and placement state.
pub struct Graphics {
    picker: Picker,
    cache: GraphicsCache,
    error: Option<String>,
    placement: Option<(Rect, bool)>,
}

const KITTY_IMAGE_ID: u32 = 0x727468;

/// Queries supported terminal graphics unless disabled or running in Apple Terminal.
pub fn detect(mode: Mode, term_program: Option<&str>) -> Option<(Graphics, Option<KittyCleanup>)> {
    if mode == Mode::Off || term_program == Some("Apple_Terminal") {
        return None;
    }
    let picker = Picker::from_query_stdio_with_options(QueryStdioOptions {
        timeout: Duration::from_millis(200),
        ..QueryStdioOptions::default()
    })
    .ok()
    .filter(|picker| picker.protocol_type() != ProtocolType::Halfblocks)?;
    let cleanup = (picker.protocol_type() == ProtocolType::Kitty).then(|| KittyCleanup {
        tmux: picker.tmux_detected(),
    });
    Some((Graphics::new(picker), cleanup))
}

fn repaint_previous(graphics: &mut Graphics, placement: Option<(Rect, bool)>, buffer: &mut Buffer) {
    if graphics.picker.protocol_type() != ProtocolType::Kitty
        && graphics.placement != placement
        && let Some((rect, _)) = graphics.placement
    {
        for pos in rect.intersection(buffer.area).positions() {
            buffer[pos].set_diff_option(CellDiffOption::AlwaysUpdate);
        }
    }
    graphics.placement = placement;
}

impl Graphics {
    fn new(picker: Picker) -> Self {
        Self {
            picker,
            cache: Memo::default(),
            error: None,
            placement: None,
        }
    }

    /// Returns the protocol label used in the status line.
    pub fn name(&self) -> &'static str {
        match self.picker.protocol_type() {
            ProtocolType::Kitty => "kitty",
            ProtocolType::Iterm2 => "iTerm2",
            ProtocolType::Sixel => "sixel",
            ProtocolType::Halfblocks => "halfblocks",
        }
    }

    /// Returns the most recent encoding error.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Repaints the previous image placement when returning to glyph rendering.
    pub fn hide(&mut self, buffer: &mut Buffer) {
        repaint_previous(self, None, buffer);
    }

    /// Draws the original image and minimap, retaining any encoding error.
    pub fn draw_image(
        &mut self,
        frame: &mut Frame,
        original: &RgbaImage,
        grid_version: u64,
        view: &View,
        geometry: &PreviewGeometry,
        rgb_overviews: &mut Memo<(u64, u16, u16), Vec<Color>>,
    ) -> Option<&str> {
        let (gw, gh) = geometry.grid;
        // Overlay changes trigger image emission for iTerm2 and Sixel.
        let overlay = geometry.minimap.is_some();
        let key = (
            grid_version,
            if view.exact { geometry.origin.0 } else { 0 },
            if view.exact { geometry.origin.1 } else { 0 },
            view.exact,
            geometry.area,
            overlay,
        );
        let picker = &self.picker;
        let rebuilt = self.cache.update(key, || {
            let font = picker.font_size();
            let (rect, image) =
                view::original_image(original, view, geometry.area, (font.width, font.height));
            let protocol = if picker.protocol_type() == ProtocolType::Kitty {
                // Pin the image id so exit cleanup can delete it.
                StatefulProtocol::new(
                    image.into(),
                    font,
                    None,
                    StatefulProtocolType::Kitty(StatefulKitty::new(
                        KITTY_IMAGE_ID,
                        picker.tmux_detected(),
                        picker
                            .capabilities()
                            .contains(&Capability::KittyCompression),
                    )),
                )
            } else {
                picker.new_resize_protocol(image.into())
            };
            (rect, protocol)
        });
        if rebuilt {
            self.error = None;
        }
        let rect = self.cache.get().unwrap().0;
        repaint_previous(self, Some((rect, overlay)), frame.buffer_mut());
        let picker = &self.picker;
        let Some((rect, protocol)) = self.cache.get_mut() else {
            return self.error.as_deref();
        };
        frame.render_stateful_widget(
            StatefulImage::new().resize(Resize::Fit(None)),
            *rect,
            protocol,
        );
        if let Some(result) = protocol.last_encoding_result() {
            self.error = result.err().map(|error| format!("Graphics: {error}"));
        }
        if !overlay {
            return self.error.as_deref();
        }
        view::draw_minimap(
            view::Pixels::Rgb(original, grid_version),
            gw,
            gh,
            view,
            geometry.area,
            frame.buffer_mut(),
            rgb_overviews,
        );
        if matches!(
            picker.protocol_type(),
            ProtocolType::Iterm2 | ProtocolType::Sixel
        ) && let Some(minimap) = geometry.minimap
        {
            for pos in minimap.frame.positions() {
                frame.buffer_mut()[pos].set_diff_option(CellDiffOption::AlwaysUpdate);
            }
        }
        self.error.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::App,
        ui::{Layout, layout},
    };
    use ratatui::{Terminal, backend::TestBackend};

    fn terminal_layout(terminal: &Terminal<TestBackend>) -> Layout {
        let size = terminal.size().unwrap();
        layout(Rect::new(0, 0, size.width, size.height))
    }

    #[test]
    fn disabled_and_apple_terminal_skip_detection() {
        assert!(detect(Mode::Off, None).is_none());
        assert!(detect(Mode::Auto, Some("Apple_Terminal")).is_none());
    }

    #[test]
    fn graphics_cache_tracks_grid_and_view() {
        let mut app = App::new(RgbaImage::new(120, 120), None);
        let mut picker = ratatui_image::picker::Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        app.graphics = Some(Graphics::new(picker));
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let draw = |app: &mut App, terminal: &mut Terminal<TestBackend>| {
            app.sync(&terminal_layout(terminal));
            terminal
                .draw(|frame| app.draw(frame, &layout(frame.area())))
                .unwrap();
            app.graphics.as_ref().unwrap().cache.version()
        };
        assert_eq!(draw(&mut app, &mut terminal), 1);
        app.params.options.strength = 0.5;
        assert_eq!(draw(&mut app, &mut terminal), 1);
        app.view.x = 1;
        app.view.y = 1;
        assert_eq!(draw(&mut app, &mut terminal), 1);
        app.view.exact = true;
        assert_eq!(draw(&mut app, &mut terminal), 2);
        app.view.x = 2;
        assert_eq!(draw(&mut app, &mut terminal), 3);
        app.view.hide_minimap = true;
        assert_eq!(draw(&mut app, &mut terminal), 4);
        app.params.rotation = 2;
        assert_eq!(draw(&mut app, &mut terminal), 5);
        assert_eq!(draw(&mut app, &mut terminal), 5);
        app.params.scale = 50;
        assert_eq!(draw(&mut app, &mut terminal), 6);
        terminal.backend_mut().resize(90, 35);
        assert_eq!(draw(&mut app, &mut terminal), 7);
    }

    #[test]
    fn image_placement_repaints_only_previous_rect_for_iterm2_and_sixel() {
        use ratatui::{buffer::Buffer, layout::Rect};
        let area = Rect::new(0, 0, 10, 8);
        let old = Rect::new(2, 1, 6, 5);
        for protocol in [
            ProtocolType::Kitty,
            ProtocolType::Iterm2,
            ProtocolType::Sixel,
        ] {
            let mut picker = ratatui_image::picker::Picker::halfblocks();
            picker.set_protocol_type(protocol);
            let mut graphics = Graphics::new(picker);
            for placement in [
                Some((Rect::new(3, 2, 2, 2), false)),
                Some((old, true)),
                None,
            ] {
                graphics.placement = Some((old, false));
                let mut buffer = Buffer::empty(area);
                let baseline = buffer.clone();
                repaint_previous(&mut graphics, placement, &mut buffer);
                let emitted = baseline.diff(&buffer);
                let expected = if protocol == ProtocolType::Kitty {
                    0
                } else {
                    old.area()
                };
                assert_eq!(emitted.len(), expected as usize);
                assert!(
                    emitted
                        .iter()
                        .all(|(x, y, _)| old.contains((*x, *y).into()))
                );
                let baseline = Buffer::empty(area);
                let mut unchanged = baseline.clone();
                repaint_previous(&mut graphics, placement, &mut unchanged);
                assert!(baseline.diff(&unchanged).is_empty());
            }
        }
    }

    #[test]
    fn leaving_graphics_repaints_the_previous_image_in_one_draw() {
        for protocol in [
            ProtocolType::Kitty,
            ProtocolType::Iterm2,
            ProtocolType::Sixel,
        ] {
            let mut app = App::new(RgbaImage::new(12, 12), None);
            let mut picker = ratatui_image::picker::Picker::halfblocks();
            picker.set_protocol_type(protocol);
            app.graphics = Some(Graphics::new(picker));
            let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
            terminal
                .draw(|frame| app.draw(frame, &layout(frame.area())))
                .unwrap();
            let (old, _) = app.graphics.as_ref().unwrap().placement.unwrap();
            app.show_original = false;
            app.sync(&terminal_layout(&terminal));
            let completed = terminal
                .draw(|frame| app.draw(frame, &layout(frame.area())))
                .unwrap();
            for pos in completed.buffer.area.positions() {
                assert_eq!(
                    completed.buffer[pos].diff_option == CellDiffOption::AlwaysUpdate,
                    protocol != ProtocolType::Kitty && old.contains(pos),
                    "{protocol:?} at {pos:?}",
                );
            }
            assert!(app.graphics.as_ref().unwrap().placement.is_none());
        }
    }
}
