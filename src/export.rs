use rthid::{
    dither::PaletteColor,
    png::{Job, Worker},
};
use std::{
    io,
    path::{Path, PathBuf},
};

/// Companion PNG destination, background writer and latest status.
pub struct Export {
    path: PathBuf,
    worker: Option<Worker>,
    error: Option<String>,
}

impl Export {
    /// Starts the writer and retains startup errors for display.
    pub fn new(path: PathBuf) -> Self {
        let (worker, error) = match Worker::new(path.clone()) {
            Ok(worker) => (Some(worker), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            worker,
            error: message(&path, error),
            path,
        }
    }

    /// Whether a writer needs dithered pixels even when the preview shows RGB.
    pub fn needs_dither(&self) -> bool {
        self.worker.is_some()
    }

    /// Submits the refreshed dithered pixels.
    pub fn sync(&mut self, indices: &[u8], w: u32, h: u32, palette: &[PaletteColor]) {
        if let Some(worker) = &mut self.worker
            && let Err(error) = worker.submit(Job {
                indices: indices.to_vec(),
                w,
                h,
                palette: palette.to_vec(),
            })
        {
            self.set_error(Some(error));
        }
    }

    /// Collects the latest write result and reports a status change.
    pub fn poll(&mut self) -> bool {
        if let Some(worker) = &self.worker
            && let Some(result) = worker.poll()
        {
            return self.set_error(result.err());
        }
        false
    }

    /// Waits for pending writes and reports the final status.
    pub fn finish(&mut self) -> io::Result<()> {
        if let Some(worker) = self.worker.take() {
            self.set_error(worker.finish().err());
        }
        match &self.error {
            Some(error) => Err(io::Error::other(error.clone())),
            None => Ok(()),
        }
    }

    /// Returns the latest PNG error formatted for display.
    pub fn status(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn set_error(&mut self, error: Option<io::Error>) -> bool {
        let error = message(&self.path, error);
        let changed = self.error != error;
        self.error = error;
        changed
    }
}

fn message(path: &Path, error: Option<io::Error>) -> Option<String> {
    error.map(|error| format!("PNG {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rthid::dither::make_palette;

    fn sync(export: &mut Export, w: u32, h: u32) {
        export.sync(
            &vec![0; (w * h) as usize],
            w,
            h,
            &make_palette(&["#000", "#fff"]).unwrap(),
        )
    }

    fn wait_for(export: &mut Export, ready: impl Fn(&Export) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            export.poll();
            if ready(export) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "PNG completion timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn sync_submits_latest_pixels() {
        let path =
            std::env::temp_dir().join(format!("rthid-version-test-{}.png", std::process::id()));
        let mut export = Export::new(path.clone());
        sync(&mut export, 4, 2);
        sync(&mut export, 2, 4);
        export.finish().unwrap();
        assert_eq!(image::open(&path).unwrap().to_rgba8().dimensions(), (8, 16));
        assert!(export.status().is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn poll_requests_redraw_only_when_png_status_changes() {
        let path = std::env::temp_dir().join(format!("rthid-status-test-{}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let mut export = Export::new(path.clone());
        sync(&mut export, 4, 2);
        let wait_for_change = |export: &mut Export| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !export.poll() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "PNG status change timed out"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        wait_for_change(&mut export);
        assert!(
            export
                .status()
                .unwrap()
                .starts_with(&format!("PNG {}: ", path.display()))
        );
        assert!(!export.poll());
        std::fs::remove_dir(&path).unwrap();
        sync(&mut export, 4, 2);
        wait_for_change(&mut export);
        assert!(export.status().is_none());
        assert!(!export.poll());
        export.finish().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn sync_keeps_write_error_until_background_success() {
        let dir = std::env::temp_dir().join(format!("rthid-error-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("preview.png");
        std::fs::create_dir(&path).unwrap();
        let mut export = Export::new(path.clone());
        sync(&mut export, 4, 2);
        wait_for(&mut export, |export| export.status().is_some());
        std::fs::remove_dir(&path).unwrap();
        let error = export.status().unwrap().to_owned();
        sync(&mut export, 4, 2);
        assert_eq!(export.status(), Some(error.as_str()));
        export.finish().unwrap();
        assert!(export.status().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sync_updates_companion_and_reports_write_errors() {
        let dir = std::env::temp_dir().join(format!("rthid-refresh-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("preview.png");
        let mut export = Export::new(path.clone());
        sync(&mut export, 4, 2);
        wait_for(&mut export, |_| {
            image::open(&path).is_ok_and(|image| image.width() == 16)
        });
        sync(&mut export, 2, 4);
        wait_for(&mut export, |_| {
            image::open(&path).is_ok_and(|image| image.width() == 8)
        });
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        sync(&mut export, 2, 4);
        wait_for(&mut export, |export| export.status().is_some());
        export.poll();
        assert!(export.status().is_some());
        std::fs::remove_dir(&path).unwrap();
        sync(&mut export, 2, 4);
        wait_for(&mut export, |export| export.status().is_none());
        export.finish().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
