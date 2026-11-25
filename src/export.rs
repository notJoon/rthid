//! Companion PNG wrapper. Submits each new dithered grid to the background
//! writer and keeps the latest error for the status line.

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

    /// Whether a writer needs dithered pixels even when the preview shows Original.
    /// False when the writer failed to start.
    pub fn needs_dither(&self) -> bool {
        self.worker.is_some()
    }

    /// Hands the dithered grid to the writer without blocking. Older pending grids are skipped.
    /// Records an error if the writer has stopped.
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

    /// Collects the result of the latest submitted grid and returns whether the status changed.
    pub fn poll(&mut self) -> bool {
        if let Some(worker) = &self.worker
            && let Some(result) = worker.poll()
        {
            return self.set_error(result.err());
        }
        false
    }

    /// Waits for the last write and returns the final error, if any. Later syncs are ignored.
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

    fn poll_until(export: &mut Export, ready: impl Fn(&Export, bool) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let changed = export.poll();
            if ready(export, changed) {
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
    fn poll_requests_redraw_only_when_png_status_changes() {
        let dir = crate::temp_dir::TempDir::new();
        let path = dir.join("preview.png");
        let mut export = Export::new(path.clone());
        for (w, h, width) in [(4, 2, 16), (2, 4, 8)] {
            sync(&mut export, w, h);
            poll_until(&mut export, |_, changed| {
                assert!(!changed);
                image::open(&path).is_ok_and(|image| image.width() == width)
            });
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        sync(&mut export, 4, 2);
        poll_until(&mut export, |_, changed| changed);
        assert!(
            export
                .status()
                .unwrap()
                .starts_with(&format!("PNG {}: ", path.display()))
        );
        assert!(!export.poll());
        std::fs::remove_dir(&path).unwrap();
        sync(&mut export, 4, 2);
        poll_until(&mut export, |_, changed| changed);
        assert!(export.status().is_none());
        assert!(!export.poll());
        export.finish().unwrap();
    }

    #[test]
    fn sync_keeps_write_error_until_background_success() {
        let dir = crate::temp_dir::TempDir::new();
        let path = dir.join("preview.png");
        std::fs::create_dir(&path).unwrap();
        let mut export = Export::new(path.clone());
        sync(&mut export, 4, 2);
        poll_until(&mut export, |export, _| export.status().is_some());
        std::fs::remove_dir(&path).unwrap();
        let error = export.status().unwrap().to_owned();
        sync(&mut export, 4, 2);
        assert_eq!(export.status(), Some(error.as_str()));
        export.finish().unwrap();
        assert!(export.status().is_none());
    }
}
