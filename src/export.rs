//! Companion PNG destination and latest write error.

use rthid_dither::dither::PaletteColor;
use rthid_io::{
    io::{Cause, IoError, Job, Op},
    png::Grid,
};
use std::{
    io,
    path::{Path, PathBuf},
};

/// Companion PNG destination and latest status.
pub struct Export {
    path: PathBuf,
    error: Option<String>,
}

impl Export {
    /// Retains the destination for future writes.
    pub fn new(path: PathBuf) -> Self {
        Self { path, error: None }
    }

    /// Destination protected from being opened as the source.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Copies the current grid into an owned export job.
    ///
    /// # Errors
    /// Returns an export error if the grid cannot be encoded at the preview scale.
    pub fn job(
        &self,
        indices: &[u8],
        w: u32,
        h: u32,
        palette: &[PaletteColor],
    ) -> Result<Job, IoError> {
        let grid =
            Grid::new(indices.to_vec(), w, h, palette.to_vec()).map_err(|error| IoError {
                op: Op::Encode,
                path: self.path.clone(),
                cause: Cause::Io(error),
            })?;
        Ok(Job::Export {
            path: self.path.clone(),
            grid,
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
        let mut export = Export::new("preview.png".into());
        let error = export.job(&[], 1, 1, &[]).err().unwrap();
        assert!(
            matches!(&error.cause, Cause::Io(error) if error.kind() == io::ErrorKind::InvalidInput)
        );
        export.complete(Err(&error));
        assert_eq!(
            export.status(),
            Some("PNG preview.png: PNG grid size mismatch")
        );
    }
}
