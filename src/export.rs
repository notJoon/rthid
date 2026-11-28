//! Companion PNG and mass-study SVG destinations and latest write errors.

use crate::settings::Settings;
use rthid_dither::mass::{MassOptions, MassStudy};
use rthid_io::io::{Cause, IoError, Job, Op};
use rthid_render::{Grid, MassSheet, check_output_size, preview_size};
use std::{
    io,
    path::{Path, PathBuf},
};

/// Companion PNG destination and latest status.
pub struct Export {
    state: ExportState,
    dpi: Option<u32>,
    /// PNG size of the last submitted job.
    png_size: Option<(u32, u32)>,
}

/// Destination, display label and latest error shared by both exporters.
pub struct ExportState {
    path: PathBuf,
    label: &'static str,
    error: Option<String>,
}

impl ExportState {
    fn new(path: PathBuf, label: &'static str) -> Self {
        Self {
            path,
            label,
            error: None,
        }
    }

    /// Destination path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Updates the retained error and reports whether the status changed.
    /// Export errors are always `Op::Encode` and use the same reason-before-path shape.
    pub fn complete(&mut self, result: Result<(), &IoError>) -> bool {
        let error = result
            .err()
            .map(|error| self.message(&error.cause.to_string()));
        self.set_error(error)
    }

    fn message(&self, cause: &str) -> String {
        format!("{}: {cause}: {}", self.label, self.path.display())
    }

    fn set_error(&mut self, error: Option<String>) -> bool {
        let changed = self.error != error;
        self.error = error;
        changed
    }

    /// Latest error formatted for display.
    pub fn status(&self) -> Option<&str> {
        self.error.as_deref()
    }

    fn result(&self) -> io::Result<()> {
        match &self.error {
            Some(error) => Err(io::Error::other(error.clone())),
            None => Ok(()),
        }
    }
}

impl Export {
    /// Retains the destination and optional physical resolution for future writes.
    pub fn new(path: PathBuf, dpi: Option<u32>) -> Self {
        Self {
            state: ExportState::new(path, "PNG"),
            dpi,
            png_size: None,
        }
    }

    /// Physical PNG resolution, `None` for the integer preview scale.
    pub fn dpi(&self) -> Option<u32> {
        self.dpi
    }

    /// Destination protected from being opened as the source.
    pub fn path(&self) -> &Path {
        &self.state.path
    }

    /// Resolves and validates the companion PNG size before submission.
    fn size(
        &self,
        w: u32,
        h: u32,
        settings: &Settings,
        source: (u32, u32),
    ) -> Result<(u32, u32), IoError> {
        let size = match self.dpi {
            Some(dpi) => crate::print::png_size(settings, source, dpi),
            None => Ok(preview_size(w, h)),
        };
        size.and_then(|size| check_output_size(size).map(|()| size))
            .map_err(|error| self.invalid(error))
    }

    fn invalid(&self, error: io::Error) -> IoError {
        IoError {
            op: Op::Encode,
            path: self.state.path.clone(),
            cause: Cause::Io(error),
        }
    }

    /// Copies the current grid into an owned export job and retains its PNG size.
    ///
    /// # Errors
    /// Returns an export error if the grid or palette indices are invalid.
    fn job(
        &mut self,
        indices: &[u8],
        w: u32,
        h: u32,
        settings: &Settings,
        size: (u32, u32),
    ) -> Result<Job, IoError> {
        let grid = Grid::new(indices.to_vec(), w, h, settings.palette.colors().to_vec())
            .map_err(|error| self.invalid(error))?;
        self.png_size = Some(size);
        let dpi = self.dpi;
        Ok(Job::Export {
            path: self.state.path.clone(),
            write: Box::new(move |file| rthid_render::write_png(file, &grid, size, dpi)),
        })
    }

    /// Returns a new export job when its pixels or output size changed.
    pub fn sync(
        &mut self,
        indices: &[u8],
        w: u32,
        h: u32,
        settings: &Settings,
        source: (u32, u32),
        changed: bool,
    ) -> Option<Job> {
        let result = self.size(w, h, settings, source).and_then(|size| {
            if !changed && self.png_size == Some(size) {
                return Ok(None);
            }
            self.job(indices, w, h, settings, size).map(Some)
        });
        result.unwrap_or_else(|error| {
            self.complete(Err(&error));
            None
        })
    }

    /// Updates the retained error and reports whether the status changed.
    pub fn complete(&mut self, result: Result<(), &IoError>) -> bool {
        self.state.complete(result)
    }

    /// Destination and status updated by write results.
    pub fn state_mut(&mut self) -> &mut ExportState {
        &mut self.state
    }

    /// Returns the latest PNG error formatted for display.
    pub fn status(&self) -> Option<&str> {
        self.state.status()
    }

    /// Returns the retained write error, if any.
    pub fn result(&self) -> io::Result<()> {
        self.state.result()
    }
}

/// Mass-study SVG destination and latest status.
pub struct MassExport {
    state: ExportState,
    version: Option<u64>,
}

impl MassExport {
    /// Retains the destination for future mass-study writes.
    pub fn new(path: PathBuf) -> Self {
        Self {
            state: ExportState::new(path, "SVG"),
            version: None,
        }
    }

    /// Destination protected from being opened as the source.
    pub fn path(&self) -> &Path {
        &self.state.path
    }

    /// Builds an owned SVG export job when the mass build changed.
    pub fn sync(
        &mut self,
        version: u64,
        preview: &image::RgbaImage,
        study: &MassStudy,
        tones: [[u8; 3]; 4],
        options: &MassOptions,
    ) -> Option<Job> {
        if self.version == Some(version) {
            return None;
        }
        self.version = Some(version);
        let path = self.state.path.clone();
        let preview = preview.clone();
        let study = study.clone();
        let options = options.clone();
        Some(Job::Export {
            path,
            write: Box::new(move |file| {
                let rings = study.rounded_rings();
                rthid_render::write_mass_svg(
                    file,
                    &MassSheet {
                        preview: preview.as_raw(),
                        preview_w: preview.width(),
                        preview_h: preview.height(),
                        study: &study,
                        rings: &rings,
                        tones,
                        options: &options,
                    },
                )
            }),
        })
    }

    /// Records a failed mass build once for its cache version.
    pub fn fail(&mut self, version: u64, error: &str) {
        if self.version != Some(version) {
            self.version = Some(version);
            self.state.set_error(Some(self.state.message(error)));
        }
    }

    /// Destination and status updated by write results.
    pub fn state_mut(&mut self) -> &mut ExportState {
        &mut self.state
    }

    /// Returns the latest SVG error formatted for display.
    pub fn status(&self) -> Option<&str> {
        self.state.status()
    }

    /// Returns the retained write error, if any.
    pub fn result(&self) -> io::Result<()> {
        self.state.result()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_grid_fails_before_submission() {
        let mut export = Export::new("preview.png".into(), None);
        let error = export
            .job(&[], 1, 1, &Settings::default(), (4, 4))
            .err()
            .unwrap();
        assert!(
            matches!(&error.cause, Cause::Io(error) if error.kind() == io::ErrorKind::InvalidInput)
        );
        export.complete(Err(&error));
        assert_eq!(
            export.status(),
            Some("PNG: grid has 0 indices, expected 1x1 = 1: preview.png")
        );
    }
}
