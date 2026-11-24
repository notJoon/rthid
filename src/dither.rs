#[derive(Debug, Clone)]
pub struct PaletteColor {
    pub rgb: [u8; 3],
    pub f: [f64; 3],
}

#[derive(Debug, Clone, Copy)]
pub struct Tone {
    pub brightness: f64,
    pub contrast: f64,
    pub gamma: f64,
    pub black: f64,
    pub white: f64,
    pub sharpen: f64,
}

impl Default for Tone {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 0.0,
            gamma: 1.0,
            black: 0.0,
            white: 1.0,
            sharpen: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub enum Mode {
    #[default]
    Tone,
    Color,
}

#[derive(Debug, Clone)]
pub struct Options {
    pub palette: Vec<PaletteColor>,
    pub mode: Mode,
    pub serpentine: bool,
    pub strength: f64,
    pub tone: Tone,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            palette: make_palette(&["#000", "#fff"]).unwrap(),
            mode: Mode::Tone,
            serpentine: true,
            strength: 1.0,
            tone: Tone::default(),
        }
    }
}

// Rec. 601 luma.
fn luma([r, g, b]: [f64; 3]) -> f64 {
    0.299 * r + 0.587 * g + 0.114 * b
}

fn hex_to_rgb(hex: &str) -> Result<[u8; 3], &'static str> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if !matches!(hex.len(), 3 | 6) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("expected a 3- or 6-digit hex color");
    }
    let s = hex.len() / 3;
    let scale = if s == 1 { 17 } else { 1 };
    Ok(std::array::from_fn(|i| {
        u8::from_str_radix(&hex[i * s..(i + 1) * s], 16).unwrap() * scale
    }))
}

/// Sorted darkest first, so the last color acts as paper.
pub fn make_palette(hexes: &[&str]) -> Result<Vec<PaletteColor>, &'static str> {
    if hexes.is_empty() || hexes.len() > 256 {
        return Err("palette must have 1 to 256 colors");
    }
    let mut palette = hexes
        .iter()
        .map(|hex| {
            let rgb = hex_to_rgb(hex)?;
            Ok(PaletteColor {
                rgb,
                f: rgb.map(|v| f64::from(v) / 255.0),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let lum = |p: &PaletteColor| luma(p.rgb.map(f64::from)) / 255.0;
    palette.sort_by(|a, b| lum(a).total_cmp(&lum(b)));
    Ok(palette)
}

fn to_channels(rgba: &[u8], w: usize, h: usize, tone: &Tone) -> [Vec<f32>; 3] {
    let n = w.checked_mul(h).expect("image size overflow");
    assert_eq!(Some(rgba.len()), n.checked_mul(4), "RGBA length mismatch");
    let mut channels = [vec![0.0; n], vec![0.0; n], vec![0.0; n]];
    let cf = 1.0 + tone.contrast;
    let span = (tone.white - tone.black).max(0.02);
    let g = 1.0 / tone.gamma;
    for (c, channel) in channels.iter_mut().enumerate() {
        for (i, value) in channel.iter_mut().enumerate() {
            *value = (f64::from(rgba[i * 4 + c]) / 255.0) as f32;
        }
        if tone.sharpen > 0.0 {
            let blurred = box_blur3(channel, w, h);
            for (value, blur) in channel.iter_mut().zip(blurred) {
                let v = f64::from(*value);
                *value = (v + tone.sharpen * (v - f64::from(blur))) as f32;
            }
        }
        for value in channel {
            let v = ((f64::from(*value) - 0.5) * cf + 0.5 + tone.brightness).clamp(0.0, 1.0);
            *value = ((v.powf(g) - tone.black) / span).clamp(0.0, 1.0) as f32;
        }
    }
    channels
}

fn box_blur3(channel: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut blurred = vec![0.0; channel.len()];
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0;
            let mut count = 0;
            for yy in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for xx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    sum += f64::from(channel[yy * w + xx]);
                    count += 1;
                }
            }
            blurred[y * w + x] = (sum / f64::from(count)) as f32;
        }
    }
    blurred
}

pub fn dither(rgba: &[u8], w: usize, h: usize, options: &Options) -> Vec<u8> {
    let palette = &options.palette;
    assert!(
        !palette.is_empty() && palette.len() <= 256,
        "palette must have 1 to 256 colors"
    );
    // One color needs no work, and tone mode would divide by zero levels.
    if palette.len() == 1 {
        return vec![0; w * h];
    }
    let mut channels = to_channels(rgba, w, h, &options.tone);
    let mut out = vec![0; w * h];
    let levels = (palette.len() - 1) as f64;
    // Tone mode diffuses error through luminance alone.
    let work = match options.mode {
        Mode::Tone => {
            // Luminance replaces the red buffer, rounded to f32 like every stored value.
            let [red, green, blue] = &mut channels;
            for ((r, g), b) in red.iter_mut().zip(green).zip(blue) {
                *r = luma([*r, *g, *b].map(f64::from)) as f32;
            }
            &mut channels[..1]
        }
        Mode::Color => &mut channels[..],
    };
    const KERNEL: [(isize, usize); 6] = [(1, 0), (2, 0), (-1, 1), (0, 1), (1, 1), (0, 2)];
    for y in 0..h {
        let reverse = options.serpentine && y % 2 == 1;
        for k in 0..w {
            let x = if reverse { w - 1 - k } else { k };
            let i = y * w + x;
            let (q, target) = match options.mode {
                Mode::Tone => {
                    let v = f64::from(work[0][i]);
                    let q = (v * levels + 0.5).floor().clamp(0.0, levels) as usize;
                    (q, [q as f64 / levels; 3])
                }
                Mode::Color => {
                    let px: [f64; 3] = std::array::from_fn(|c| f64::from(work[c][i]));
                    let mut nearest = 0;
                    let mut best_distance = f64::INFINITY;
                    for (j, color) in palette.iter().enumerate() {
                        let [dr, dg, db] = std::array::from_fn(|c| px[c] - color.f[c]);
                        let distance = 2.0 * dr * dr + 4.0 * dg * dg + 3.0 * db * db;
                        if distance < best_distance {
                            best_distance = distance;
                            nearest = j;
                        }
                    }
                    (nearest, palette[nearest].f)
                }
            };
            out[i] = q as u8;
            for (channel, t) in work.iter_mut().zip(target) {
                // Every Atkinson weight is 1, so each neighbor gets e/8.
                let e = (f64::from(channel[i]) - t) * options.strength / 8.0;
                for (dx, dy) in KERNEL {
                    let xx = x.checked_add_signed(if reverse { -dx } else { dx });
                    let yy = y + dy;
                    if let Some(xx) = xx.filter(|&xx| xx < w)
                        && yy < h
                    {
                        let j = yy * w + xx;
                        // Compute in f64, convert to f32 only on store.
                        channel[j] = (f64::from(channel[j]) + e) as f32;
                    }
                }
            }
        }
    }
    out
}

pub fn rasterize(
    idx: &[u8],
    gw: usize,
    gh: usize,
    ow: usize,
    oh: usize,
    color_of: impl Fn(u8) -> [u8; 3],
) -> Vec<u8> {
    assert!(gw > 0 && gh > 0, "source dimensions must be positive");
    assert_eq!(Some(idx.len()), gw.checked_mul(gh), "index length mismatch");
    let size = ow
        .checked_mul(oh)
        .and_then(|n| n.checked_mul(4))
        .expect("output size overflow");
    let mut rgba = vec![0; size];
    for y in 0..oh {
        let row = y * gh / oh * gw;
        for x in 0..ow {
            let color = color_of(idx[row + x * gw / ow]);
            let offset = (y * ow + x) * 4;
            rgba[offset..offset + 3].copy_from_slice(&color);
            rgba[offset + 3] = 255;
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        for hex in ["#aBc", "aBc", "#aAbBcC", "aAbBcC"] {
            assert_eq!(hex_to_rgb(hex), Ok([170, 187, 204]));
        }
        for hex in ["", "#abcd", "#ggg", "#가"] {
            assert!(hex_to_rgb(hex).is_err());
        }
    }

    #[test]
    fn sorts_palette_by_luminance() {
        let palette = make_palette(&["#fff", "#0f0", "#00f", "#000", "#f00"]).unwrap();
        assert_eq!(
            palette.iter().map(|p| p.rgb).collect::<Vec<_>>(),
            [[0, 0, 0], [0, 0, 255], [255, 0, 0], [0, 255, 0], [255; 3]]
        );
        assert_eq!(palette[4].f, [1.0; 3]);
    }

    #[test]
    fn solid_black_and_white_use_palette_endpoints() {
        let mut options = Options {
            palette: make_palette(&["#000", "#555", "#aaa", "#fff"]).unwrap(),
            ..Options::default()
        };
        for mode in [Mode::Tone, Mode::Color] {
            options.mode = mode;
            for (value, index) in [(0, 0), (255, 3)] {
                let rgba = [value, value, value, 255].repeat(32);
                assert_eq!(dither(&rgba, 8, 4, &options), vec![index; 32]);
            }
        }
    }

    fn gradient() -> Vec<u8> {
        (0..128)
            .flat_map(|i| {
                let (x, y) = (i % 16, i / 16);
                [
                    ((x * 16 + y * 7) % 256) as u8,
                    ((x * 11 + y * 29) % 256) as u8,
                    ((x * 23 + y * 13) % 256) as u8,
                    255,
                ]
            })
            .collect()
    }

    #[test]
    fn tone_matches_golden() {
        let options = Options {
            palette: make_palette(&["#fff", "#222", "#aaa", "#555"]).unwrap(),
            ..Options::default()
        };
        assert_eq!(dither(&gradient(), 16, 8, &options), TONE_GOLDEN);
    }

    #[test]
    fn color_matches_golden() {
        let options = Options {
            palette: make_palette(&["#f3e8c8", "#172438", "#c45238"]).unwrap(),
            mode: Mode::Color,
            serpentine: false,
            strength: 0.8,
            tone: Tone {
                contrast: 0.15,
                sharpen: 0.6,
                black: 0.04,
                white: 0.95,
                ..Tone::default()
            },
        };
        assert_eq!(dither(&gradient(), 16, 8, &options), COLOR_GOLDEN);
    }

    #[test]
    fn tone_adjustments_match_expected_channels() {
        let tone = Tone {
            brightness: 0.05,
            contrast: 0.5,
            gamma: 2.0,
            black: 0.1,
            white: 0.9,
            ..Tone::default()
        };
        assert_eq!(
            to_channels(&[0, 51, 255, 0], 1, 1, &tone),
            [vec![0.0], vec![0.270_284_7], vec![1.0]]
        );
    }

    #[test]
    fn rasterize_scales_nearest_neighbor() {
        let result = rasterize(&[0, 1], 2, 1, 4, 2, |i| {
            if i == 0 {
                [10, 20, 30]
            } else {
                [200, 210, 220]
            }
        });
        let row = [
            10, 20, 30, 255, 10, 20, 30, 255, 200, 210, 220, 255, 200, 210, 220, 255,
        ];
        assert_eq!(result, row.repeat(2));
    }
    const TONE_GOLDEN: [u8; 128] = [
        0, 0, 0, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 0, 0, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2,
        2, 2, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2,
        2, 3, 3, 2, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 3, 1, 1, 0, 1, 1, 1, 2, 2, 2, 2, 2, 3, 2,
        3, 1, 1, 1, 0, 1, 2, 2, 2, 2, 2, 2, 2, 3, 1, 1, 1, 1, 1, 2, 1, 1, 2, 2, 2, 2, 2, 1, 1, 1,
        1, 1, 1, 1, 2, 1, 1, 1,
    ];
    const COLOR_GOLDEN: [u8; 128] = [
        0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 2, 2, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 1, 2, 2, 2, 2, 1, 1, 1,
        2, 2, 0, 0, 0, 0, 0, 1, 2, 2, 0, 2, 1, 1, 2, 2, 2, 2, 0, 0, 0, 0, 2, 2, 0, 2, 2, 2, 1, 1,
        2, 2, 2, 2, 0, 0, 1, 2, 2, 2, 2, 2, 2, 1, 2, 2, 2, 1, 1, 0, 0, 0, 2, 2, 0, 2, 2, 2, 2, 2,
        2, 1, 1, 1, 0, 0, 0, 2, 2, 2, 2, 2, 2, 2, 0, 1, 1, 1, 1, 1, 0, 0, 2, 2, 0, 2, 2, 0, 0, 0,
        1, 1, 1, 1, 1, 0, 0, 2,
    ];
}
