//! Background PNG writer for the dithered grid. Each dot becomes a solid square
//! and the file is replaced only after a complete encode.

use crate::dither::{self, PaletteColor};
use image::ImageEncoder;
use std::{
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
};

/// Integer dot size from 1 to 4 that keeps the long side within 2048 px,
/// unless the grid is already larger.
pub fn preview_scale(w: u32, h: u32) -> u32 {
    2048u32.checked_div(w.max(h)).unwrap_or(1).clamp(1, 4)
}

/// Owned grid submitted for background PNG encoding.
pub struct Job {
    /// One palette index per dot, row major.
    pub indices: Vec<u8>,
    /// Grid width in pixels.
    pub w: u32,
    /// Grid height in pixels.
    pub h: u32,
    /// Colors referenced by the indices.
    pub palette: Vec<PaletteColor>,
}

/// Background PNG writer. When several grids are queued only the newest is written.
pub struct Worker {
    jobs: Option<Sender<(u64, Job)>>,
    results: Receiver<(u64, Option<io::Error>)>,
    latest: u64,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl Worker {
    /// Spawns the writer thread for the destination path.
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let (jobs, queued) = mpsc::channel::<(u64, Job)>();
        let (finished, results) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("preview-png".into())
            .spawn(move || {
                let mut result = Ok(());
                while let Ok(first) = queued.recv() {
                    let (id, job) = queued.try_iter().last().unwrap_or(first);
                    result = write_atomic(
                        &path,
                        &job.indices,
                        job.w,
                        job.h,
                        &job.palette,
                        preview_scale(job.w, job.h),
                    );
                    let error = result
                        .as_ref()
                        .err()
                        .map(|error| io::Error::new(error.kind(), error.to_string()));
                    if finished.send((id, error)).is_err() {
                        break;
                    }
                }
                result
            })?;
        Ok(Self {
            jobs: Some(jobs),
            results,
            latest: 0,
            thread: Some(thread),
        })
    }

    /// Queues a grid without blocking and makes it the latest requested result.
    pub fn submit(&mut self, job: Job) -> io::Result<()> {
        self.latest += 1;
        self.jobs
            .as_ref()
            .unwrap()
            .send((self.latest, job))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PNG worker stopped"))
    }

    /// Returns the result for the latest submitted grid once written, ignoring replaced grids.
    pub fn poll(&self) -> Option<io::Result<()>> {
        self.results
            .try_iter()
            .filter(|(id, _)| *id == self.latest)
            .last()
            .map(|(_, error)| error.map_or(Ok(()), Err))
    }

    /// Waits for the final job and reports its result, even if it was already polled.
    pub fn finish(mut self) -> io::Result<()> {
        drop(self.jobs.take());
        self.thread
            .take()
            .unwrap()
            .join()
            .map_err(|_| io::Error::other("PNG worker panicked"))?
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Writes the grid with each dot as a `scale` px solid square. Encodes to a temporary
/// file beside `path` and renames it over the destination only after encoding succeeds.
pub fn write_atomic(
    path: &Path,
    indices: &[u8],
    w: u32,
    h: u32,
    palette: &[PaletteColor],
    scale: u32,
) -> io::Result<()> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "invalid PNG grid or scale");
    let scaled = |n: u32| n.checked_mul(scale).filter(|&v| v > 0).ok_or_else(invalid);
    let ow = scaled(w)?;
    let oh = scaled(h)?;
    (ow as usize)
        .checked_mul(oh as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|&n| n <= isize::MAX as usize)
        .ok_or_else(invalid)?;
    if (w as usize).checked_mul(h as usize) != Some(indices.len())
        || indices.iter().any(|&i| usize::from(i) >= palette.len())
    {
        return Err(invalid());
    }
    let pixels = dither::rasterize(
        indices,
        w as usize,
        h as usize,
        ow as usize,
        oh as usize,
        |i| palette[usize::from(i)].rgb,
    );
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let (temp, mut file) = loop {
        let temp = parent.join(format!(
            ".rthid-preview-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => break (temp, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let encoded = image::codecs::png::PngEncoder::new(&mut file)
        .write_image(&pixels, ow, oh, image::ExtendedColorType::Rgba8)
        .map_err(io::Error::other);
    drop(file);
    let result = encoded.and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dither::make_palette;

    #[test]
    fn preview_scale_bounds_the_long_side_with_exact_integer_dots() {
        for (w, h, expected) in [
            (1, 1, 4),
            (100, 20, 4),
            (20, 1024, 2),
            (1200, 400, 1),
            (2048, 2048, 1),
            (4096, 1, 1),
            (0, 0, 1),
        ] {
            assert_eq!(preview_scale(w, h), expected, "{w}x{h}");
        }
    }

    #[test]
    fn worker_eventually_writes_latest_job_and_reports_its_result() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        let path = dir.join("latest.png");
        let mut worker = Worker::new(path.clone()).unwrap();
        let palette = make_palette(&["#000", "#fff"]).unwrap();
        for n in 0..30 {
            worker
                .submit(Job {
                    indices: vec![if n == 0 { 2 } else { u8::from(n == 29) }; 6],
                    w: 3,
                    h: 2,
                    palette: palette.clone(),
                })
                .unwrap();
        }
        assert!(wait_for_result(&worker).is_ok());
        let image = image::open(&path).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (12, 8));
        assert!(image.pixels().all(|p| p.0 == [255; 4]));
        drop(worker);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    fn wait_for_result(worker: &Worker) -> io::Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(result) = worker.poll() {
                return result;
            }
            assert!(std::time::Instant::now() < deadline, "PNG job timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn worker_finish_reports_unpolled_and_polled_write_failures() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        for polled in [false, true] {
            let mut worker = Worker::new(dir.clone()).unwrap();
            worker
                .submit(Job {
                    indices: vec![0],
                    w: 1,
                    h: 1,
                    palette: make_palette(&["#000"]).unwrap(),
                })
                .unwrap();
            if polled {
                assert!(wait_for_result(&worker).is_err());
            }
            assert!(worker.finish().is_err());
        }
    }

    #[test]
    fn worker_finish_uses_latest_success_after_write_failure() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        let path = dir.join("preview.png");
        let mut worker = Worker::new(path.clone()).unwrap();
        for index in [1, 0] {
            worker
                .submit(Job {
                    indices: vec![index],
                    w: 1,
                    h: 1,
                    palette: make_palette(&["#000"]).unwrap(),
                })
                .unwrap();
            if index == 1 {
                assert!(wait_for_result(&worker).is_err());
            }
        }
        worker.finish().unwrap();
        assert!(image::open(path).is_ok());
    }

    #[test]
    fn worker_finish_handles_no_jobs_and_panics() {
        let dir = tempfile::tempdir().unwrap();
        Worker::new(dir.path().join("rthid-unused.png"))
            .unwrap()
            .finish()
            .unwrap();
        let (jobs, _) = mpsc::channel();
        let (_, results) = mpsc::channel();
        let worker = Worker {
            jobs: Some(jobs),
            results,
            latest: 0,
            thread: Some(thread::spawn(|| panic!("worker failed"))),
        };
        assert!(worker.finish().is_err());
    }

    #[test]
    fn atomic_png_replaces_and_preserves_destination_on_failure() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        let path = dir.join("preview.png");
        let palette = make_palette(&["#000", "#fff"]).unwrap();
        std::fs::write(&path, b"old").unwrap();
        write_atomic(&path, &[0, 1], 2, 1, &palette, 4).unwrap();
        let image = image::open(&path).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (8, 4));
        assert_eq!(image.get_pixel(3, 3).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(4, 3).0, [255; 4]);
        let before = std::fs::read(&path).unwrap();
        for (indices, w, h, scale) in [
            (&[0, 2][..], 2, 1, 4),
            (&[0][..], 0, 1, 4),
            (&[0][..], 1, 1, 0),
            (&[0][..], 1, 1, u32::MAX),
        ] {
            assert!(write_atomic(&path, indices, w, h, &palette, scale).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        assert!(write_atomic(&dir, &[0], 1, 1, &palette, 1).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        write_atomic(&path, &[1], 1, 1, &palette, 1).unwrap();
        assert_eq!(image::open(&path).unwrap().width(), 1);
    }
}
