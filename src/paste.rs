//! Extracts the first file path from terminal drop text without file access.

use std::path::PathBuf;

/// Reads the first quoted or escaped path, including local file URIs.
/// Plain words without a directory or extension are ignored.
pub fn dropped_path(text: &str) -> Option<PathBuf> {
    let mut token = String::new();
    let mut quote = None;
    let mut chars = text.trim().chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' if quote != Some('\'') => token.push(chars.next()?),
            '\'' | '"' if quote == Some(c) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(c),
            c if c.is_whitespace() && quote.is_none() => break,
            c => token.push(c),
        }
    }
    if quote.is_some() || token.is_empty() {
        return None;
    }
    if let Some(uri) = token.strip_prefix("file://") {
        let path = uri.strip_prefix("localhost").unwrap_or(uri);
        if !path.starts_with('/') {
            return None;
        }
        let mut decoded = Vec::new();
        let mut bytes = path.bytes();
        while let Some(byte) = bytes.next() {
            decoded.push(if byte == b'%' {
                let high = char::from(bytes.next()?).to_digit(16)?;
                let low = char::from(bytes.next()?).to_digit(16)?;
                (high * 16 + low) as u8
            } else {
                byte
            });
        }
        token = String::from_utf8(decoded).ok()?;
    } else if token.contains("://") {
        return None;
    }
    if token.chars().any(char::is_control) {
        return None;
    }
    let path = PathBuf::from(&token);
    (token.contains('/') || path.extension().is_some()).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_drop_formats() {
        for (text, expected) in [
            ("", None),
            (" \r\n\t ", None),
            ("hello world", None),
            (" \t/tmp/photo.jpg\r\n", Some("/tmp/photo.jpg")),
            ("photo.jpg", Some("photo.jpg")),
            ("'My Photo.jpg'", Some("My Photo.jpg")),
            ("\"My Photo.jpg\"", Some("My Photo.jpg")),
            (r"My\ Photo.jpg", Some("My Photo.jpg")),
            (r"/tmp/Photo\ \(1\).jpg", Some("/tmp/Photo (1).jpg")),
            (r#""/tmp/My \"Photo\".jpg""#, Some("/tmp/My \"Photo\".jpg")),
            ("\"/tmp/it's.jpg\"", Some("/tmp/it's.jpg")),
            ("file:///tmp/My%20Photo.jpg", Some("/tmp/My Photo.jpg")),
            ("file://localhost/tmp/a%28b%29.jpg", Some("/tmp/a(b).jpg")),
            ("file:///tmp/%ED%95%9C.jpg", Some("/tmp/한.jpg")),
            ("'file:///tmp/My%20Photo.jpg'", Some("/tmp/My Photo.jpg")),
            ("/tmp/first.jpg\n/tmp/second.jpg", Some("/tmp/first.jpg")),
            ("/tmp/first.jpg /tmp/second.jpg", Some("/tmp/first.jpg")),
            ("'My Photo.jpg' \"Other Photo.jpg\"", Some("My Photo.jpg")),
            ("file://remote/tmp/photo.jpg", None),
            ("https://example.com/photo.jpg", None),
            ("file:///tmp/bad%2", None),
            ("file:///tmp/bad%XX.jpg", None),
            ("file:///tmp/%00.jpg", None),
            ("file:///tmp/%FF.jpg", None),
            ("'unfinished.jpg", None),
            ("photo.jpg\\", None),
        ] {
            assert_eq!(dropped_path(text), expected.map(PathBuf::from), "{text:?}");
        }
    }
}
