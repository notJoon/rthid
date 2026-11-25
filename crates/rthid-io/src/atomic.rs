//! Shared file replacement for PNG exports and configuration.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};

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
    let result = write(&mut file);
    drop(file);
    let result = result.and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

/// Updates fresh text under an advisory sibling lock and syncs the replacement file.
/// Lock errors are save errors. The persistent lock file survives config replacement.
pub fn update<T>(
    path: &Path,
    transform: impl FnOnce(Option<String>) -> io::Result<(String, T)>,
) -> io::Result<T> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    for attempt in 0..=20 {
        match lock.try_lock() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) if attempt < 20 => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(fs::TryLockError::WouldBlock) => {
                return Err(io::Error::new(io::ErrorKind::WouldBlock, "config is busy"));
            }
            Err(fs::TryLockError::Error(error)) => return Err(error),
        }
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let (text, result) = transform(text)?;
    replace(path, |file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })?;
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
        assert_eq!(fs::read(&path).unwrap(), b"original");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        let destination = dir.path().join("directory");
        fs::create_dir(&destination).unwrap();
        assert!(replace(&destination, |file| file.write_all(b"data")).is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}
