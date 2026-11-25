//! Home directory expansion for entered paths.

use std::path::{Path, PathBuf};

/// Expands only `~` and `~/` when a home directory is supplied.
pub fn expand_tilde(text: &str, home: Option<&Path>) -> PathBuf {
    if let Some(home) = home {
        if text == "~" {
            return home.to_path_buf();
        }
        if let Some(rest) = text.strip_prefix("~/") {
            return home.join(rest);
        }
    }
    PathBuf::from(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_expansion_only_uses_a_home_prefix() {
        for (text, home, expected) in [
            ("~", Some(Path::new("/home/me")), "/home/me"),
            (
                "~/photo.png",
                Some(Path::new("/home/me")),
                "/home/me/photo.png",
            ),
            (
                "~other/photo.png",
                Some(Path::new("/home/me")),
                "~other/photo.png",
            ),
            ("photo~.png", Some(Path::new("/home/me")), "photo~.png"),
            ("~/photo.png", None, "~/photo.png"),
            ("~", None, "~"),
        ] {
            assert_eq!(expand_tilde(text, home), PathBuf::from(expected));
        }
    }
}
