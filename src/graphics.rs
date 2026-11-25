use std::{
    io::{self, Write},
    time::Duration,
};

use crate::{memo::Memo, view::PreviewGeometry};
use image::{
    RgbaImage,
    imageops::{self, FilterType},
};
use ratatui::{
    Frame,
    buffer::{Buffer, CellDiffOption},
    layout::{Constraint, Rect},
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

/// Grid version and available cells for the original overview.
#[derive(PartialEq)]
struct ImageKey {
    grid: u64,
    area: Rect,
}

/// Terminal image protocol, cached image and placement state.
pub struct Graphics {
    picker: Picker,
    // The pinned Kitty id requires a single cached protocol.
    cache: Memo<ImageKey, (Rect, StatefulProtocol)>,
    error: Option<String>,
    placement: Option<Rect>,
}

const KITTY_IMAGE_ID: u32 = 0x727468;

/// Queries supported terminal graphics unless disabled or running in Apple Terminal.
pub fn detect(mode: Mode, term_program: Option<&str>) -> Option<Graphics> {
    if mode == Mode::Off || term_program == Some("Apple_Terminal") {
        return None;
    }
    let picker = Picker::from_query_stdio_with_options(QueryStdioOptions {
        timeout: Duration::from_millis(200),
        ..QueryStdioOptions::default()
    })
    .ok()
    .filter(|picker| picker.protocol_type() != ProtocolType::Halfblocks)?;
    Some(Graphics::new(picker))
}

impl Drop for Graphics {
    fn drop(&mut self) {
        if self.picker.protocol_type() != ProtocolType::Kitty {
            return;
        }
        let (start, escape, end) = Parser::tmux_start_escape_end(self.picker.tmux_detected());
        let mut stdout = io::stdout().lock();
        let _ = write!(
            stdout,
            "{start}{escape}_Ga=d,d=I,i={KITTY_IMAGE_ID}{escape}\\{end}"
        )
        .and_then(|()| stdout.flush());
    }
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
        self.repaint_previous(None, buffer);
    }

    fn repaint_previous(&mut self, placement: Option<Rect>, buffer: &mut Buffer) {
        if self.picker.protocol_type() != ProtocolType::Kitty
            && self.placement != placement
            && let Some(rect) = self.placement
        {
            for pos in rect.intersection(buffer.area).positions() {
                buffer[pos].set_diff_option(CellDiffOption::AlwaysUpdate);
            }
        }
        self.placement = placement;
    }

    /// Draws the original overview, retaining any encoding error.
    pub fn draw_image(
        &mut self,
        frame: &mut Frame,
        original: &RgbaImage,
        grid_version: u64,
        geometry: &PreviewGeometry,
    ) {
        let key = ImageKey {
            grid: grid_version,
            area: geometry.area,
        };
        let picker = &self.picker;
        let rebuilt = self.cache.update(key, || {
            let font = picker.font_size();
            let (rect, image) = original_image(original, geometry, (font.width, font.height));
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
        self.repaint_previous(Some(rect), frame.buffer_mut());
        let (rect, protocol) = self.cache.get_mut().unwrap();
        frame.render_stateful_widget(
            StatefulImage::new().resize(Resize::Fit(None)),
            *rect,
            protocol,
        );
        if let Some(result) = protocol.last_encoding_result() {
            self.error = result.err().map(|error| format!("Graphics: {error}"));
        }
    }
}

fn original_image(
    grid: &RgbaImage,
    geometry: &PreviewGeometry,
    font: (u16, u16),
) -> (Rect, RgbaImage) {
    let (w, h) = geometry.grid;
    let area = geometry.area;
    let (fw, fh) = (u32::from(font.0), u32::from(font.1));
    if area.is_empty() || w == 0 || h == 0 || fw == 0 || fh == 0 {
        return (Rect::new(area.x, area.y, 0, 0), RgbaImage::new(0, 0));
    }
    let (cw, ch) = {
        let aspect = f64::from(fh) / f64::from(fw);
        let scale = (f64::from(area.width) / f64::from(w))
            .min(f64::from(area.height) * aspect / f64::from(h));
        (
            (f64::from(w) * scale).floor().max(1.0) as u16,
            (f64::from(h) * scale / aspect).floor().max(1.0) as u16,
        )
    };
    let rect = area.centered(Constraint::Length(cw), Constraint::Length(ch));
    let (pw, ph) = (u32::from(cw) * fw, u32::from(ch) * fh);
    let filter = if pw < w || ph < h {
        FilterType::Triangle
    } else {
        FilterType::Nearest
    };
    let mut resized = image::DynamicImage::ImageRgba8(grid.clone())
        .resize(pw, ph, filter)
        .into_rgba8();
    for pixel in resized.pixels_mut() {
        pixel[3] = 255;
    }
    if resized.dimensions() == (pw, ph) {
        return (rect, resized);
    }
    let mut image = RgbaImage::new(pw, ph);
    imageops::overlay(
        &mut image,
        &resized,
        i64::from((pw - resized.width()) / 2),
        i64::from((ph - resized.height()) / 2),
    );
    (rect, image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::View;
    use image::Rgba;
    fn checkerboard(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            if (x + y) % 2 == 0 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        })
    }

    #[test]
    fn graphics_overview_uses_font_aspect_and_preserves_crisp_upscaling() {
        let pixels = checkerboard(4, 4);
        let (rect, image) = original_image(
            &pixels,
            &crate::view::geometry(
                &View::default(),
                pixels.dimensions(),
                Rect::new(2, 3, 20, 10),
            ),
            (8, 16),
        );
        assert_eq!(rect, Rect::new(2, 3, 20, 10));
        assert_eq!(image.dimensions(), (160, 160));
        assert!(
            image
                .pixels()
                .all(|p| p.0 == [255, 0, 0, 255] || p.0 == [0, 0, 255, 255])
        );
        let (rect, image) = original_image(
            &pixels,
            &crate::view::geometry(
                &View::default(),
                pixels.dimensions(),
                Rect::new(2, 3, 20, 10),
            ),
            (8, 8),
        );
        assert_eq!(rect, Rect::new(7, 3, 10, 10));
        assert_eq!(image.dimensions(), (80, 80));
    }

    #[test]
    fn graphics_ignores_source_alpha() {
        let grid = RgbaImage::from_raw(1, 2, vec![25, 26, 128, 0, 231, 47, 93, 128]).unwrap();
        for exact in [false, true] {
            let (_, image) = original_image(
                &grid,
                &crate::view::geometry(
                    &View {
                        exact,
                        ..View::default()
                    },
                    grid.dimensions(),
                    Rect::new(0, 0, 1, 1),
                ),
                (1, 2),
            );
            assert_eq!(image.as_raw(), &[25, 26, 128, 255, 231, 47, 93, 255]);
        }
    }

    #[test]
    fn graphics_overview_pads_rounding_gaps_and_downscales() {
        let pixels = RgbaImage::from_pixel(300, 100, Rgba([10, 20, 30, 255]));
        let (rect, image) = original_image(
            &pixels,
            &crate::view::geometry(
                &View::default(),
                pixels.dimensions(),
                Rect::new(2, 3, 11, 10),
            ),
            (7, 15),
        );
        assert_eq!(rect, Rect::new(2, 8, 11, 1));
        assert_eq!(image.dimensions(), (77, 15));
        let opaque: Vec<_> = image
            .enumerate_pixels()
            .filter(|(_, _, p)| p[3] == 255)
            .collect();
        let width = opaque.iter().map(|(x, _, _)| x).max().unwrap()
            - opaque.iter().map(|(x, _, _)| x).min().unwrap()
            + 1;
        assert_eq!(width, 45);
        assert!(opaque.iter().all(|(_, _, p)| p.0 == [10, 20, 30, 255]));
        assert_eq!(image.get_pixel(0, 0)[3], 0);
        for area in [Rect::new(0, 0, 0, 3), Rect::new(0, 0, 3, 0)] {
            let (rect, image) = original_image(
                &pixels,
                &crate::view::geometry(&View::default(), pixels.dimensions(), area),
                (7, 15),
            );
            assert!(rect.is_empty());
            assert_eq!(image.dimensions(), (0, 0));
        }
    }

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
    fn graphics_cache_tracks_overview_grid_and_area() {
        let mut app = App::new(RgbaImage::new(120, 120), None);
        let mut picker = ratatui_image::picker::Picker::halfblocks();
        picker.set_protocol_type(ProtocolType::Kitty);
        let mut renderer = crate::ui::Renderer::new(Some(Graphics::new(picker)));
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut draw = |app: &mut App, terminal: &mut Terminal<TestBackend>| {
            app.sync(terminal_layout(terminal).preview);
            terminal
                .draw(|frame| crate::ui::draw(frame, app, &layout(frame.area()), &mut renderer))
                .unwrap();
            renderer.graphics().unwrap().cache.version()
        };
        assert_eq!(draw(&mut app, &mut terminal), 1);
        app.params.options.strength = 0.5;
        assert_eq!(draw(&mut app, &mut terminal), 1);
        app.view.x = 1;
        app.view.y = 1;
        assert_eq!(draw(&mut app, &mut terminal), 1);
        app.view.exact = true;
        assert_eq!(draw(&mut app, &mut terminal), 1);
        app.view.x = 2;
        assert_eq!(draw(&mut app, &mut terminal), 1);
        app.view.hide_minimap = true;
        assert_eq!(draw(&mut app, &mut terminal), 1);
        app.view.exact = false;
        assert_eq!(draw(&mut app, &mut terminal), 1);
        app.view.exact = true;
        app.params.rotation = 2;
        assert_eq!(draw(&mut app, &mut terminal), 1);
        app.view.exact = false;
        assert_eq!(draw(&mut app, &mut terminal), 2);
        assert_eq!(draw(&mut app, &mut terminal), 2);
        app.params.scale = 50;
        assert_eq!(draw(&mut app, &mut terminal), 3);
        terminal.backend_mut().resize(90, 35);
        assert_eq!(draw(&mut app, &mut terminal), 4);
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
            for placement in [Some(Rect::new(3, 2, 2, 2)), Some(old), None] {
                graphics.placement = Some(old);
                let mut buffer = Buffer::empty(area);
                let baseline = buffer.clone();
                graphics.repaint_previous(placement, &mut buffer);
                let emitted = baseline.diff(&buffer);
                let expected = if protocol == ProtocolType::Kitty || placement == Some(old) {
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
                graphics.repaint_previous(placement, &mut unchanged);
                assert!(baseline.diff(&unchanged).is_empty());
            }
        }
    }

    #[test]
    fn leaving_graphics_repaints_the_previous_image_in_one_draw() {
        for exact in [false, true] {
            for protocol in [
                ProtocolType::Kitty,
                ProtocolType::Iterm2,
                ProtocolType::Sixel,
            ] {
                let mut app = App::new(RgbaImage::new(12, 12), None);
                let mut picker = ratatui_image::picker::Picker::halfblocks();
                picker.set_protocol_type(protocol);
                let mut renderer = crate::ui::Renderer::new(Some(Graphics::new(picker)));
                let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
                terminal
                    .draw(|frame| {
                        crate::ui::draw(frame, &app, &layout(frame.area()), &mut renderer)
                    })
                    .unwrap();
                let old = renderer.graphics().unwrap().placement.unwrap();
                app.show_original = exact;
                app.view.exact = exact;
                app.sync(terminal_layout(&terminal).preview);
                let completed = terminal
                    .draw(|frame| {
                        crate::ui::draw(frame, &app, &layout(frame.area()), &mut renderer)
                    })
                    .unwrap();
                for pos in completed.buffer.area.positions() {
                    assert_eq!(
                        completed.buffer[pos].diff_option == CellDiffOption::AlwaysUpdate,
                        protocol != ProtocolType::Kitty && old.contains(pos),
                        "{protocol:?} at {pos:?}",
                    );
                }
                assert!(renderer.graphics().unwrap().placement.is_none());
                let mut glyphs = crate::ui::Renderer::new(None);
                let mut baseline = Terminal::new(TestBackend::new(100, 40)).unwrap();
                baseline
                    .draw(|frame| crate::ui::draw(frame, &app, &layout(frame.area()), &mut glyphs))
                    .unwrap();
                for pos in completed.buffer.area.positions() {
                    let mut actual = completed.buffer[pos].clone();
                    actual.set_diff_option(CellDiffOption::None);
                    assert_eq!(actual, baseline.backend().buffer()[pos]);
                }
                let text: String = completed
                    .buffer
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(!text.contains(&format!(
                    "Original ({})",
                    renderer.graphics().unwrap().name()
                )));
            }
        }
    }
}
