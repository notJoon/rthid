//! Shared file replacement for PNG exports and configuration.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

struct Temp(Option<PathBuf>);

impl Drop for Temp {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = fs::remove_file(path);
        }
    }
}

/// Creates missing parent directories and replaces the destination after a successful write.
/// The caller syncs the file inside the write closure when durability is needed.
pub fn replace(path: &Path, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    if let Ok(entries) = fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".rthid-")
                && name.ends_with(".tmp")
                && let Ok(metadata) = entry.metadata()
                && metadata.is_file()
                && let Ok(modified) = metadata.modified()
                && modified
                    .elapsed()
                    .is_ok_and(|age| age > Duration::from_secs(3600))
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let (temp, mut file) = loop {
        let temp = parent.join(format!(
            ".rthid-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let mut temp = Temp(Some(temp));
    if let Ok(metadata) = fs::metadata(path) {
        let _ = file.set_permissions(metadata.permissions());
    }
    write(&mut file)?;
    drop(file);
    fs::rename(temp.0.as_ref().unwrap(), path)?;
    temp.0 = None;
    Ok(())
}

/// Updates fresh text under an advisory sibling lock and syncs the replacement file.
/// Resolves the destination once so symlink aliases share the lock and keep the symlink.
/// Lock errors, including unsupported filesystem locking, never permit unlocked writes.
/// The persistent lock file survives config replacement.
pub fn update<T>(
    path: &Path,
    transform: impl FnOnce(Option<String>) -> io::Result<(String, T)>,
) -> io::Result<T> {
    let mut path = path.to_path_buf();
    let mut hops = 0;
    while let Ok(target) = fs::read_link(&path) {
        if hops == 40 {
            return Err(io::Error::other("config symlink loop or too many links"));
        }
        hops += 1;
        path = if target.is_absolute() {
            target
        } else {
            path.parent().unwrap_or(Path::new(".")).join(target)
        };
    }
    let path = match path.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            fs::create_dir_all(parent)?;
            parent.canonicalize()?.join(
                path.file_name()
                    .ok_or_else(|| io::Error::other("config has no file name"))?,
            )
        }
        Err(error) => return Err(error),
    };
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut delay = Duration::from_millis(5);
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(io::Error::new(io::ErrorKind::WouldBlock, "config is busy"));
                }
                thread::sleep(delay.min(remaining));
                delay = (delay * 2).min(Duration::from_millis(50));
            }
            Err(fs::TryLockError::Error(error)) => return Err(error),
        }
    }
    let text = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let (text, result) = transform(text)?;
    replace(&path, |file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })?;
    #[cfg(unix)]
    {
        // Rename committed the edit, reporting failure could retry it over another instance's edits.
        let _ = File::open(path.parent().unwrap()).and_then(|dir| dir.sync_all());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_updates_keep_every_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config");
        thread::scope(|scope| {
            for writer in 0..2 {
                let path = &path;
                scope.spawn(move || {
                    for line in 0..50 {
                        update(path, |text| {
                            let mut text = text.unwrap_or_default();
                            text.push_str(&format!("{writer}:{line}\n"));
                            Ok((text, ()))
                        })
                        .unwrap();
                        thread::sleep(Duration::from_millis(1));
                    }
                });
            }
        });
        let text = fs::read_to_string(&path).unwrap();
        let lines: std::collections::HashSet<_> = text.lines().collect();
        assert_eq!(text.lines().count(), 100);
        assert_eq!(lines.len(), 100);
        for writer in 0..2 {
            for line in 0..50 {
                assert!(lines.contains(format!("{writer}:{line}").as_str()));
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn updates_through_symlinks_keep_the_link_and_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        let alias = dir.path().join("alias");
        fs::write(&path, "original").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        symlink(&path, &alias).unwrap();
        update(&alias, |_| Ok(("updated".into(), ()))).unwrap();
        assert!(fs::symlink_metadata(&alias).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(&path).unwrap(), "updated");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        assert!(path.with_extension("lock").exists());
        assert!(!alias.with_extension("lock").exists());
    }

    #[test]
    #[cfg(unix)]
    fn dangling_links_share_the_target_lock_and_reject_excessive_hops() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config");
        let alias = dir.path().join("alias");
        symlink("nested/config", &alias).unwrap();
        update(&alias, |_| Ok(("created".into(), ()))).unwrap();
        assert!(fs::symlink_metadata(&alias).unwrap().is_symlink());
        assert_eq!(fs::read_to_string(&path).unwrap(), "created");
        assert!(path.with_extension("lock").exists());
        assert!(!alias.with_extension("lock").exists());
        for hop in (0..40).rev() {
            let target = if hop == 39 {
                alias.clone()
            } else {
                dir.path().join(format!("link{}", hop + 1))
            };
            symlink(target, dir.path().join(format!("link{hop}"))).unwrap();
        }
        update(&dir.path().join("link1"), |text| Ok((text.unwrap(), ()))).unwrap();
        assert!(update(&dir.path().join("link0"), |_| Ok((String::new(), ()))).is_err());
        let cycle = dir.path().join("cycle");
        symlink("cycle", &cycle).unwrap();
        assert!(update(&cycle, |_| Ok((String::new(), ()))).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "created");
    }

    #[test]
    fn replacement_sweeps_only_old_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join(".rthid-old.tmp");
        let recent = dir.path().join(".rthid-new.tmp");
        let unrelated = dir.path().join("unrelated.tmp");
        for path in [&old, &recent, &unrelated] {
            File::create(path).unwrap();
        }
        let earlier = std::time::SystemTime::now() - Duration::from_secs(3601);
        for path in [&old, &unrelated] {
            File::open(path).unwrap().set_modified(earlier).unwrap();
        }
        replace(&dir.path().join("config"), |file| file.write_all(b"saved")).unwrap();
        assert!(!old.exists());
        assert!(recent.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn failed_transform_leaves_destination_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config");
        fs::write(&path, "original").unwrap();
        let result: io::Result<()> = update(&path, |_| Err(io::Error::other("failed")));
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
        update(&path, |text| Ok((text.unwrap(), ()))).unwrap();
    }

    #[test]
    fn failed_writes_leave_destination_and_remove_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/file");
        replace(&path, |file| file.write_all(b"original")).unwrap();
        assert!(
            replace(&path, |file| {
                file.write_all(b"partial")?;
                Err(io::Error::other("failed"))
            })
            .is_err()
        );
        assert!(std::panic::catch_unwind(|| replace(&path, |_| panic!("write panic"))).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        let destination = dir.path().join("directory");
        fs::create_dir(&destination).unwrap();
        assert!(replace(&destination, |file| file.write_all(b"data")).is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
