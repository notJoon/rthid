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
    match fs::metadata(path) {
        Ok(metadata) => file.set_permissions(metadata.permissions())?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
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
    File::open(path.parent().unwrap())?.sync_all()?;
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
