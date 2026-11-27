//! Companion PNG destination and latest write error.

use crate::settings::Settings;
use rthid_io::io::{Cause, IoError, Job, Op};
use rthid_render::{Grid, check_output_size, preview_size};
use std::{
    io,
    path::{Path, PathBuf},
};

/// Companion PNG destination and latest status.
pub struct Export {
    path: PathBuf,
    dpi: Option<u32>,
    error: Option<String>,
}

impl Export {
    /// Retains the destination and optional physical resolution for future writes.
    pub fn new(path: PathBuf, dpi: Option<u32>) -> Self {
        Self {
            path,
            dpi,
            error: None,
        }
    }

    /// Physical PNG resolution, `None` for the integer preview scale.
    pub fn dpi(&self) -> Option<u32> {
        self.dpi
    }

    /// Destination protected from being opened as the source.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Copies the current grid into an owned export job.
    ///
    /// # Errors
    /// Returns an export error if the grid, palette indices or output size are invalid.
    pub fn job(
        &self,
        indices: &[u8],
        w: u32,
        h: u32,
        settings: &Settings,
        source: (u32, u32),
    ) -> Result<Job, IoError> {
        let invalid = |error| IoError {
            op: Op::Encode,
            path: self.path.clone(),
            cause: Cause::Io(error),
        };
        let size = match self.dpi {
            Some(dpi) => crate::print::png_size(settings, source, dpi),
            None => Ok(preview_size(w, h)),
        }
        .and_then(|size| check_output_size(size).map(|()| size))
        .map_err(invalid)?;
        let grid = Grid::new(indices.to_vec(), w, h, settings.palette.clone()).map_err(invalid)?;
        let dpi = self.dpi;
        Ok(Job::Export {
            path: self.path.clone(),
            write: Box::new(move |file| rthid_render::write_png(file, &grid, size, dpi)),
        })
    }

    /// Updates the retained error and reports whether the status changed.
    pub fn complete(&mut self, result: Result<(), &IoError>) -> bool {
        let error = result.err().map(ToString::to_string);
        let changed = self.error != error;
        self.error = error;
        changed
    }

    /// Returns the latest PNG error formatted for display.
    pub fn status(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Returns the retained write error, if any.
    pub fn result(&self) -> io::Result<()> {
        match &self.error {
            Some(error) => Err(io::Error::other(error.clone())),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_grid_fails_before_submission() {
        let mut export = Export::new("preview.png".into(), None);
        let error = export
            .job(&[], 1, 1, &Settings::default(), (1, 1))
            .err()
            .unwrap();
        assert!(
            matches!(&error.cause, Cause::Io(error) if error.kind() == io::ErrorKind::InvalidInput)
        );
        export.complete(Err(&error));
        assert_eq!(
            export.status(),
            Some("PNG preview.png: grid has 0 indices, expected 1x1 = 1")
        );
    }
}
