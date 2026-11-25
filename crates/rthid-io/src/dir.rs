//! Visible directories and supported image files.

use std::{
    io,
    path::{Path, PathBuf},
};

/// Visible directory entry supplied by the event loop.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    /// File name relative to the listed directory.
    pub name: String,
    /// Whether selecting this entry lists a directory.
    pub dir: bool,
}

/// Lists visible directories first, then readable image formats, sorted by name.
pub fn list_dir(dir: &Path) -> io::Result<(PathBuf, Vec<Entry>)> {
    let dir = dir.canonicalize()?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(entry.path()) else {
            continue;
        };
        let is_dir = metadata.is_dir();
        if is_dir
            || (metadata.is_file()
                && image::ImageFormat::from_path(entry.path())
                    .is_ok_and(|format| format.reading_enabled()))
        {
            entries.push(Entry { name, dir: is_dir });
        }
    }
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    if dir.parent().is_some() {
        entries.insert(
            0,
            Entry {
                name: "..".into(),
                dir: true,
            },
        );
    }
    Ok((dir, entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_listing_filters_and_sorts() {
        let temp = tempfile::tempdir().unwrap();
        for name in [".hidden.png", "z.png", "b.png", "notes.txt"] {
            std::fs::write(temp.path().join(name), b"").unwrap();
        }
        for name in ["c", "a"] {
            std::fs::create_dir(temp.path().join(name)).unwrap();
        }
        let (dir, entries) = list_dir(temp.path()).unwrap();
        assert_eq!(dir, temp.path().canonicalize().unwrap());
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.name.as_str(), entry.dir))
                .collect::<Vec<_>>(),
            [
                ("..", true),
                ("a", true),
                ("c", true),
                ("b.png", false),
                ("z.png", false)
            ]
        );
        assert!(list_dir(&temp.path().join("missing")).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(temp.path().join("a"), temp.path().join("linked")).unwrap();
            symlink(temp.path().join("b.png"), temp.path().join("linked.png")).unwrap();
            symlink(temp.path().join("missing"), temp.path().join("broken.png")).unwrap();
            let (_, entries) = list_dir(temp.path()).unwrap();
            assert_eq!(entries.len(), 7);
            assert!(entries.contains(&Entry {
                name: "linked".into(),
                dir: true
            }));
            assert!(entries.contains(&Entry {
                name: "linked.png".into(),
                dir: false
            }));
        }
    }
}
