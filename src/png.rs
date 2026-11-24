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

pub fn preview_scale(w: u32, h: u32) -> u32 {
    2048u32.checked_div(w.max(h)).unwrap_or(1).clamp(1, 4)
}

pub struct Job {
    pub indices: Vec<u8>,
    pub w: u32,
    pub h: u32,
    pub palette: Vec<PaletteColor>,
}

pub struct Worker {
    jobs: Option<Sender<(u64, Job)>>,
    results: Receiver<(u64, Option<io::Error>)>,
    latest: u64,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let (jobs, queued) = mpsc::channel::<(u64, Job)>();
        let (finished, results) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("preview-png".into())
            .spawn(move || {
                while let Ok(mut newest) = queued.recv() {
                    for job in queued.try_iter() {
                        newest = job;
                    }
                    let (id, job) = newest;
                    let error = write_atomic(
                        &path,
                        &job.indices,
                        job.w,
                        job.h,
                        &job.palette,
                        preview_scale(job.w, job.h),
                    )
                    .err();
                    if finished.send((id, error)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            results,
            latest: 0,
            thread: Some(thread),
        })
    }

    pub fn submit(&mut self, job: Job) -> io::Result<()> {
        self.latest += 1;
        self.jobs
            .as_ref()
            .unwrap()
            .send((self.latest, job))
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PNG worker stopped"))
    }

    // Ignore completions for grids that have already been replaced in the UI.
    pub fn poll(&self) -> Option<Option<io::Error>> {
        self.results
            .try_iter()
            .filter(|(id, _)| *id == self.latest)
            .last()
            .map(|(_, error)| error)
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

/// Writes an integer-scaled dot grid, replacing the destination only after PNG encoding succeeds.
pub fn write_atomic(
    path: &Path,
    indices: &[u8],
    w: u32,
    h: u32,
    palette: &[PaletteColor],
    scale: u32,
) -> io::Result<()> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "invalid PNG grid or scale");
    let ow = w
        .checked_mul(scale)
        .filter(|&v| v > 0)
        .ok_or_else(invalid)?;
    let oh = h
        .checked_mul(scale)
        .filter(|&v| v > 0)
        .ok_or_else(invalid)?;
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
        use std::time::{Duration, Instant};
        let dir = std::env::temp_dir().join(format!("rthid-worker-test-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
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
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = worker.poll() {
                assert!(result.is_none());
                break;
            }
            assert!(Instant::now() < deadline, "latest PNG job timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
        let image = image::open(&path).unwrap().to_rgba8();
        assert_eq!(image.dimensions(), (12, 8));
        assert!(image.pixels().all(|p| p.0 == [255; 4]));
        drop(worker);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn atomic_png_replaces_and_preserves_destination_on_failure() {
        let dir = std::env::temp_dir().join(format!("rthid-png-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
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
        std::fs::remove_dir_all(dir).unwrap();
    }
}
