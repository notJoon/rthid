//! Kitty, iTerm2 and Sixel output for the Original Overview.
//!
//! This is the only path that draws real device pixels. Every other view is drawn
//! with cell glyphs.

use std::{
    io::{self, Write},
    time::Duration,
};

use crate::{
    memo::Memo,
    view::{PreviewGeometry, fit_aspect},
};
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

/// Terminal image output for the Original Overview. Keeps one encoded image and
/// the cell rectangle it last covered.
pub struct Graphics {
    picker: Picker,
    kitty: bool,
    name: &'static str,
    // One fixed Kitty id means a second cached image would overwrite the first,
    // so the cache must hold a single entry.
    cache: Memo<ImageKey, (Rect, StatefulProtocol)>,
    error: Option<String>,
    // Cells covered by the last drawn image, None when no image is shown.
    placement: Option<Rect>,
}

/// Fixed Kitty image id ("rth" in ASCII), so each upload replaces the previous
/// image and exit can delete it.
const KITTY_IMAGE_ID: u32 = 0x727468;

/// Queries the terminal for an image protocol, blocking for up to 200 ms.
///
/// Returns None when graphics are off, the terminal is Apple Terminal, the query
/// fails, or only half blocks are available.
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

/// Deletes the pinned Kitty image so it does not linger after exit.
impl Drop for Graphics {
    fn drop(&mut self) {
        if !self.kitty {
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
    /// Wraps a picker that already selected Kitty, iTerm2 or Sixel.
    ///
    /// Panics for half blocks, which use cell rendering instead.
    pub(crate) fn new(picker: Picker) -> Self {
        let protocol = picker.protocol_type();
        let name = match protocol {
            ProtocolType::Kitty => "kitty",
            ProtocolType::Iterm2 => "iTerm2",
            ProtocolType::Sixel => "sixel",
            ProtocolType::Halfblocks => unreachable!("glyph rendering does not use Graphics"),
        };
        Self {
            kitty: protocol == ProtocolType::Kitty,
            name,
            picker,
            cache: Memo::default(),
            error: None,
            placement: None,
        }
    }

    /// Returns the protocol label used in the status line.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the last encoding error, formatted for the status line. Cleared when
    /// the image is rebuilt or encodes successfully.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    // Forces a redraw of the cells under the previous iTerm2 or Sixel image when it
    // moves or hides, since cell diffing cannot see image pixels. Kitty is skipped
    // because its image is drawn through Unicode placeholder cells that a normal
    // cell diff already replaces.
    fn repaint_previous(&mut self, placement: Option<Rect>, buffer: &mut Buffer) {
        if !self.kitty
            && self.placement != placement
            && let Some(rect) = self.placement
        {
            for pos in rect.intersection(buffer.area).positions() {
                buffer[pos].set_diff_option(CellDiffOption::AlwaysUpdate);
            }
        }
        self.placement = placement;
    }

    /// Draws the Original Overview image centered in the preview area, or clears the
    /// previous placement when `request` is None.
    ///
    /// Encodes again only when the grid version or preview area changes, and a rebuild
    /// resets the stored error.
    pub fn present(
        &mut self,
        frame: &mut Frame,
        request: Option<(&RgbaImage, u64, &PreviewGeometry)>,
    ) {
        let Some((original, grid_version, geometry)) = request else {
            self.repaint_previous(None, frame.buffer_mut());
            return;
        };
        let key = ImageKey {
            grid: grid_version,
            area: geometry.area,
        };
        let picker = &self.picker;
        let rebuilt = self.cache.update(key, || {
            let font = picker.font_size();
            let (rect, image) = original_image(original, geometry, (font.width, font.height));
            let protocol = if self.kitty {
                // Pin the image id so each upload replaces the previous image and
                // exit cleanup can delete it.
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

// Returns the centered cell rectangle and an opaque image sized to its device
// pixels. Upscaling uses nearest sampling to keep dots crisp, downscaling uses a
// triangle filter, and padding left by rounding stays transparent.
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
    let (cw, ch) = fit_aspect(w, h, area, f64::from(fh) / f64::from(fw));
    let rect = area.centered(Constraint::Length(cw), Constraint::Length(ch));
    let (pw, ph) = (u32::from(cw) * fw, u32::from(ch) * fh);
    let filter = if pw < w || ph < h {
        FilterType::Triangle
    } else {
        FilterType::Nearest
    };
    let scale = (f64::from(pw) / f64::from(w)).min(f64::from(ph) / f64::from(h));
    let rw = (f64::from(w) * scale).round().max(1.0) as u32;
    let rh = (f64::from(h) * scale).round().max(1.0) as u32;
    let mut resized = imageops::resize(grid, rw, rh, filter);
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

    use crate::{app::App, ui::layout};
    use ratatui::{Terminal, backend::TestBackend};

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
        let mut graphics = Graphics::new(picker);
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        let mut draw = |app: &mut App, terminal: &mut Terminal<TestBackend>| {
            app.sync(layout(terminal.size().unwrap()).preview);
            terminal
                .draw(|frame| {
                    let geometry = app.geometry(layout(frame.area()).preview);
                    graphics.present(
                        frame,
                        (app.show_original && !app.view.exact && !geometry.area.is_empty())
                            .then(|| (app.pipeline.grid(), app.pipeline.grid_version(), &geometry)),
                    );
                })
                .unwrap();
            graphics.cache.version()
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
        app.params.rotation = crate::app::Rotation::R180;
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
    fn present_repaints_only_previous_rect_for_iterm2_and_sixel() {
        let area = Rect::new(0, 0, 10, 8);
        let old = Rect::new(2, 1, 6, 3);
        let image = RgbaImage::new(6, 6);
        for protocol in [
            ProtocolType::Kitty,
            ProtocolType::Iterm2,
            ProtocolType::Sixel,
        ] {
            let mut picker = Picker::halfblocks();
            picker.set_protocol_type(protocol);
            let mut graphics = Graphics::new(picker);
            for placement in [Some(Rect::new(0, 6, 2, 1)), Some(old), None] {
                graphics.placement = Some(old);
                let mut terminal =
                    Terminal::new(TestBackend::new(area.width, area.height)).unwrap();
                let completed = terminal
                    .draw(|frame| {
                        let geometry = crate::view::geometry(
                            &View::default(),
                            image.dimensions(),
                            placement.unwrap_or_default(),
                        );
                        graphics.present(frame, placement.map(|_| (&image, 1, &geometry)));
                        assert_eq!(graphics.placement, placement);
                    })
                    .unwrap();
                for pos in area.positions() {
                    assert_eq!(
                        completed.buffer[pos].diff_option == CellDiffOption::AlwaysUpdate,
                        protocol != ProtocolType::Kitty
                            && placement != Some(old)
                            && old.contains(pos)
                    );
                }
                let completed = terminal
                    .draw(|frame| {
                        let geometry = crate::view::geometry(
                            &View::default(),
                            image.dimensions(),
                            placement.unwrap_or_default(),
                        );
                        graphics.present(frame, placement.map(|_| (&image, 1, &geometry)));
                    })
                    .unwrap();
                assert!(
                    completed
                        .buffer
                        .content
                        .iter()
                        .all(|cell| cell.diff_option != CellDiffOption::AlwaysUpdate)
                );
            }
        }
    }
}
