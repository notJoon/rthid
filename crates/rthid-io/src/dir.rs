//! Visible directories and supported image files.

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
};

/// Visible directory entry supplied by the event loop.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    /// Original file name for joining with the listed directory.
    pub file_name: OsString,
    /// File name with invalid UTF-8 replaced for display.
    pub name: String,
    /// Whether selecting this entry lists a directory.
    pub dir: bool,
}

/// Canonical directory and its visible entries.
#[derive(Debug)]
pub struct Listing {
    /// Canonical directory path.
    pub dir: PathBuf,
    /// Sorted visible entries.
    pub entries: Vec<Entry>,
}

/// Lists visible directories first, then readable image formats, sorted by name.
///
/// # Errors
/// Returns an error if the directory cannot be resolved or opened.
/// Entries that cannot be read or inspected are skipped.
pub fn list_dir(dir: &Path) -> io::Result<Listing> {
    let dir = dir.canonicalize()?;
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let Ok(entry) = entry else { continue };
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(mut file_type) = entry.file_type() else {
            continue;
        };
        // Only symlinks need a stat to learn what they point at.
        if file_type.is_symlink() {
            let Ok(metadata) = std::fs::metadata(entry.path()) else {
                continue;
            };
            file_type = metadata.file_type();
        }
        let is_dir = file_type.is_dir();
        if is_dir
            || (file_type.is_file()
                && image::ImageFormat::from_path(entry.path())
                    .is_ok_and(|format| format.reading_enabled()))
        {
            entries.push(Entry {
                file_name,
                name,
                dir: is_dir,
            });
        }
    }
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    if dir.parent().is_some() {
        entries.insert(
            0,
            Entry {
                file_name: "..".into(),
                name: "..".into(),
                dir: true,
            },
        );
    }
    Ok(Listing { dir, entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    // APFS refuses invalid UTF-8 file names.
    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn invalid_utf8_file_names_are_listed_and_decoded() {
        use std::os::unix::ffi::OsStringExt;
        let dir = tempfile::tempdir().unwrap();
        let raw = std::ffi::OsString::from_vec(b"photo-\xff.png".to_vec());
        let raw_path = dir.path().join(&raw);
        image::RgbaImage::new(1, 1).save(&raw_path).unwrap();
        let Listing { dir: base, entries } = list_dir(dir.path()).unwrap();
        let entry = entries.iter().find(|entry| entry.file_name == raw).unwrap();
        assert_eq!(
            crate::image::load_image(&base.join(&entry.file_name))
                .unwrap()
                .dimensions(),
            (1, 1)
        );
    }

    #[test]
    fn directory_listing_filters_and_sorts() {
        let temp = tempfile::tempdir().unwrap();
        for name in [".hidden.png", "z.png", "b.png", "notes.txt"] {
            std::fs::write(temp.path().join(name), b"").unwrap();
        }
        for name in ["c", "a"] {
            std::fs::create_dir(temp.path().join(name)).unwrap();
        }
        let Listing { dir, entries } = list_dir(temp.path()).unwrap();
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
            let entries = list_dir(temp.path()).unwrap().entries;
            assert_eq!(entries.len(), 7);
            assert!(entries.contains(&Entry {
                file_name: "linked".into(),
                name: "linked".into(),
                dir: true
            }));
            assert!(entries.contains(&Entry {
                file_name: "linked.png".into(),
                name: "linked.png".into(),
                dir: false
            }));
        }
    }
}
