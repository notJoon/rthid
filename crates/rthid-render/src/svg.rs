use rthid_dither::mass::{MassOptions, MassStudy, PathCommand, RoundedRing};
use std::{fmt::Write as _, io};

/// Borrowed inputs for a self-contained mass-study analysis sheet.
pub struct MassSheet<'a> {
    /// Original preview pixels in row-major RGBA order.
    pub preview: &'a [u8],
    /// Preview width in pixels.
    pub preview_w: u32,
    /// Preview height in pixels.
    pub preview_h: u32,
    /// Mass study whose dimensions define the SVG coordinate system.
    pub study: &'a MassStudy,
    /// Rounded base, shadow and accent contours.
    pub rings: &'a [Vec<RoundedRing>; 3],
    /// Background, base, shadow and accent RGB tones.
    pub tones: [[u8; 3]; 4],
    /// Settings used to build the study.
    pub options: &'a MassOptions,
}

/// Writes a deterministic, self-contained SVG analysis sheet.
///
/// # Errors
///
/// Returns `InvalidInput` when preview dimensions do not match its RGBA bytes,
/// `InvalidData` for non-finite geometry or settings, and propagates encoder and writer errors.
pub fn write_mass_svg(mut writer: impl io::Write, sheet: &MassSheet<'_>) -> io::Result<()> {
    let expected = (sheet.preview_w as usize)
        .checked_mul(sheet.preview_h as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "preview size overflows"))?;
    if expected != sheet.preview.len() || sheet.preview_w == 0 || sheet.preview_h == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "preview RGBA does not match its dimensions",
        ));
    }

    let png = encode_png(sheet.preview, sheet.preview_w, sheet.preview_h)?;
    let encoded = base64(&png);
    let w = sheet.study.w() as f64;
    let h = sheet.study.h() as f64;
    let gap = 4.0;
    let right = w + gap;
    let font_size = w / 40.0;
    let line_height = font_size * 1.4;
    let settings_y = h + gap;
    let total_w = 2.0 * w + gap;
    let labels = [
        format!("sigma: {}", setting(sheet.options.sigma)?),
        format!(
            "thresholds: {} {} {}",
            setting(sheet.options.thresholds[0])?,
            setting(sheet.options.thresholds[1])?,
            setting(sheet.options.thresholds[2])?
        ),
        format!(
            "polarity: {}",
            if sheet.options.invert {
                "light"
            } else {
                "dark"
            }
        ),
        format!(
            "ellipses: {}",
            if sheet.options.ellipses { "on" } else { "off" }
        ),
        format!(
            "tones: {} {} {} {}",
            color(sheet.tones[0]),
            color(sheet.tones[1]),
            color(sheet.tones[2]),
            color(sheet.tones[3])
        ),
        format!("analysis: {} x {}", sheet.study.w(), sheet.study.h()),
    ];
    let total_h = settings_y + (labels.len() as f64 + 0.5) * line_height;
    let mut svg = String::new();
    writeln!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{}\">",
        escape(&format!("0 0 {} {}", number(total_w)?, number(total_h)?))
    )
    .unwrap();
    writeln!(
        svg,
        "<rect x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" fill=\"{}\"/>",
        escape(&number(total_w)?),
        escape(&number(total_h)?),
        escape(&color(sheet.tones[0]))
    )
    .unwrap();
    writeln!(
        svg,
        "<image x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\" href=\"{}\"/>",
        escape(&number(w)?),
        escape(&number(h)?),
        escape(&format!("data:image/png;base64,{encoded}"))
    )
    .unwrap();
    for (tier, rings) in sheet.rings.iter().enumerate() {
        let mut data = String::new();
        for ring in rings {
            for command in ring.commands() {
                match command {
                    PathCommand::Move(point) => path_point(&mut data, 'M', point, right)?,
                    PathCommand::Line(point) => path_point(&mut data, 'L', point, right)?,
                    PathCommand::Quadratic(control, end) => {
                        write!(
                            data,
                            "Q{} {} {} {}",
                            number(control[0] + right)?,
                            number(control[1])?,
                            number(end[0] + right)?,
                            number(end[1])?
                        )
                        .unwrap();
                    }
                    PathCommand::Close => data.push('Z'),
                }
            }
        }
        writeln!(
            svg,
            "<path fill=\"{}\" fill-rule=\"evenodd\" d=\"{}\"/>",
            escape(&color(sheet.tones[tier + 1])),
            escape(&data)
        )
        .unwrap();
    }
    for ellipse in sheet.study.ellipses() {
        let cx = ellipse.center[0] + right;
        let cy = ellipse.center[1];
        let degrees = ellipse.angle.to_degrees();
        writeln!(
            svg,
            "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"none\" stroke=\"{}\" transform=\"{}\"/>",
            escape(&number(cx)?),
            escape(&number(cy)?),
            escape(&number(ellipse.radii[0])?),
            escape(&number(ellipse.radii[1])?),
            escape(&color(sheet.tones[0])),
            escape(&format!(
                "rotate({} {} {})",
                number(degrees)?,
                number(cx)?,
                number(cy)?
            ))
        )
        .unwrap();
    }

    for (line, label) in labels.iter().enumerate() {
        writeln!(
            svg,
            "<text x=\"0\" y=\"{}\" font-family=\"monospace\" font-size=\"{}\" fill=\"{}\">{}</text>",
            escape(&number(settings_y + (line + 1) as f64 * line_height)?),
            escape(&number(font_size)?),
            escape(&color(sheet.tones[3])),
            escape(label)
        )
        .unwrap();
    }
    svg.push_str("</svg>\n");
    writer.write_all(svg.as_bytes())
}

fn path_point(data: &mut String, command: char, point: [f64; 2], offset: f64) -> io::Result<()> {
    write!(
        data,
        "{command}{} {}",
        number(point[0] + offset)?,
        number(point[1])?
    )
    .unwrap();
    Ok(())
}

fn number(value: f64) -> io::Result<String> {
    if !value.is_finite() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "non-finite SVG number",
        ));
    }
    let mut text = format!("{value:.3}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    if text == "-0" {
        text = "0".into();
    }
    Ok(text)
}

// Settings keep round-trip precision so distinct thresholds stay distinct.
fn setting(value: f32) -> io::Result<String> {
    if !value.is_finite() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "non-finite SVG setting",
        ));
    }
    Ok(value.to_string())
}

fn color(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        escaped.push_str(match ch {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&apos;",
            _ => {
                escaped.push(ch);
                continue;
            }
        });
    }
    escaped
}

fn encode_png(rgba: &[u8], w: u32, h: u32) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    writer.write_image_data(rgba).map_err(io::Error::other)?;
    drop(writer);
    Ok(bytes)
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bits = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(TABLE[((bits >> 18) & 63) as usize] as char);
        out.push(TABLE[((bits >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((bits >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(bits & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rthid_dither::mass::{MONO, mass_study};

    fn document() -> (String, usize) {
        let (w, h) = (40, 40);
        let mut preview = vec![255; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let distance = ((x as f64 + 0.5 - 20.0) / 14.0).powi(2)
                    + ((y as f64 + 0.5 - 20.0) / 14.0).powi(2);
                let value = if distance <= (12.0_f64 / 14.0).powi(2) {
                    0
                } else if distance <= 1.0 {
                    51
                } else {
                    255
                };
                let pixel = (y * w + x) * 4;
                preview[pixel..pixel + 3].fill(value);
            }
        }
        let options = MassOptions {
            sigma: 0.0,
            ..MassOptions::default()
        };
        let study = mass_study(&preview, w, h, &options).unwrap();
        assert!(!study.ellipses().is_empty());
        let rings = study.rounded_rings();
        let mut bytes = Vec::new();
        write_mass_svg(
            &mut bytes,
            &MassSheet {
                preview: &preview,
                preview_w: w as u32,
                preview_h: h as u32,
                study: &study,
                rings: &rings,
                tones: MONO.map(|tone| tone.rgb),
                options: &options,
            },
        )
        .unwrap();
        (String::from_utf8(bytes).unwrap(), study.ellipses().len())
    }

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (plain, encoded) in [
            (b"".as_slice(), ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
        ] {
            assert_eq!(base64(plain), encoded);
        }
    }

    #[test]
    fn writes_a_safe_deterministic_document() {
        let (first, ellipses) = document();
        let (second, _) = document();
        assert_eq!(first, second);
        assert!(first.starts_with("<svg"));
        assert!(first.contains("xmlns=\"http://www.w3.org/2000/svg\""));
        assert_eq!(first.matches("fill-rule=\"evenodd\"").count(), 3);
        assert_eq!(first.matches("<ellipse").count(), ellipses);
        assert!(!first.contains("script"));
        assert_eq!(first.matches("http").count(), 1);
        assert_eq!(first.matches('<').count(), first.matches('>').count());
    }

    #[test]
    fn escapes_xml_and_formats_numbers() {
        assert_eq!(escape("&<>\"'"), "&amp;&lt;&gt;&quot;&apos;");
        assert_eq!(number(1.230).unwrap(), "1.23");
        assert_eq!(number(-0.0001).unwrap(), "0");
        assert_eq!(number(2.0).unwrap(), "2");
        assert_eq!(
            number(f64::NAN).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(setting(0.2501).unwrap(), "0.2501");
        assert_ne!(setting(0.2501).unwrap(), setting(0.2502).unwrap());
        assert!(setting(f32::INFINITY).is_err());
    }
}
