#![cfg(unix)]

use std::{
    io::{ErrorKind, Read, Write},
    num::NonZeroU16,
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

const DEADLINE: Duration = Duration::from_secs(10);
const BEGIN_FRAME: &[u8] = b"\x1b[?2026h";
const END_FRAME: &[u8] = b"\x1b[?2026l";

struct Output {
    parser: vt100::Parser,
    raw: Vec<u8>,
    /// Whether the last synchronized update is still open, so the screen holds a partial frame.
    in_frame: bool,
}

struct Run {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Arc<Mutex<Output>>,
    exiting: bool,
}

impl Run {
    fn spawn(home: &Path, image: &Path) -> Self {
        let pair = native_pty_system()
            .openpty(size(100, 30))
            .expect("open pseudo terminal");
        let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_rthid"));
        command.env_clear();
        command.env("TERM", "xterm-256color");
        command.env("HOME", home);
        command.env("PATH", std::env::var_os("PATH").expect("PATH is set"));
        command.args([
            "--graphics".as_ref(),
            "off".as_ref(),
            "--glyphs".as_ref(),
            "sextant".as_ref(),
            "--config".as_ref(),
            home.join("config.toml").as_os_str(),
            image.as_os_str(),
        ]);

        let mut reader = pair.master.try_clone_reader().expect("clone pty reader");
        let writer = pair.master.take_writer().expect("take pty writer");
        let child = pair.slave.spawn_command(command).expect("spawn rthid");
        drop(pair.slave);

        let output = Arc::new(Mutex::new(Output {
            parser: vt100::Parser::new(
                NonZeroU16::new(30).unwrap(),
                NonZeroU16::new(100).unwrap(),
                100,
            ),
            raw: Vec::new(),
            in_frame: false,
        }));
        let reader_output = Arc::clone(&output);
        thread::spawn(move || {
            let mut buffer = [0; 8192];
            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => count,
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                let mut output = reader_output.lock().unwrap();
                // Rescan the tail of earlier output too, in case a marker spans two reads.
                let start = output.raw.len().saturating_sub(BEGIN_FRAME.len() - 1);
                output.raw.extend_from_slice(&buffer[..count]);
                output.parser.process(&buffer[..count]);
                if let Some(marker) = output.raw[start..]
                    .windows(BEGIN_FRAME.len())
                    .rev()
                    .find(|window| *window == BEGIN_FRAME || *window == END_FRAME)
                {
                    output.in_frame = marker == BEGIN_FRAME;
                }
            }
        });

        Self {
            master: pair.master,
            child,
            writer,
            output,
            exiting: false,
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write pty input");
        self.writer.flush().expect("flush pty input");
    }

    /// Polls `predicate` against complete frames only, as a mode 2026 terminal would show them.
    fn wait_for(&mut self, description: &str, mut predicate: impl FnMut(&Output) -> bool) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let timeout = {
                let output = self.output.lock().unwrap();
                if !output.in_frame && predicate(&output) {
                    return;
                }
                (Instant::now() >= deadline).then(|| {
                    format!(
                        "timed out waiting for {description} (raw bytes: {})\n\n{}",
                        output.raw.len(),
                        output.parser.screen().contents(),
                    )
                })
            };
            if let Some(message) = timeout {
                panic!("{message}");
            }
            if !self.exiting
                && let Some(status) = self.child.try_wait().expect("poll rthid")
            {
                panic!("rthid exited early with {status:?}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_screen(&mut self, text: &str) {
        self.wait_for(text, |output| {
            output.parser.screen().contents().contains(text)
        });
    }

    fn resize(&self, columns: u16, rows: u16) {
        let mut output = self.output.lock().unwrap();
        output.parser.set_size(
            NonZeroU16::new(rows).unwrap(),
            NonZeroU16::new(columns).unwrap(),
        );
        self.master.resize(size(columns, rows)).expect("resize pty");
    }

    fn quit(&mut self) {
        self.exiting = true;
        self.send(b"q");
    }

    fn wait_for_exit(&mut self) -> portable_pty::ExitStatus {
        let deadline = Instant::now() + DEADLINE;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll rthid") {
                return status;
            }
            if Instant::now() >= deadline {
                let screen = self.output.lock().unwrap().parser.screen().contents();
                panic!("timed out waiting for rthid to exit\n\n{screen}");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn view_range(output: &Output) -> Option<(String, String)> {
    let contents = output.parser.screen().contents();
    let status = contents.lines().find(|line| line.contains("% x "))?;
    let (_, ranges) = status.split_once("% x ")?;
    let (x, y) = ranges.split_once(" y ")?;
    Some((x.to_owned(), y.trim().to_owned()))
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[test]
fn terminal_workflow_restores_the_terminal() {
    let home = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/rthid-dither/tests/fixtures/pratt.png");
    let second = home.path().join("tall.png");
    image::RgbaImage::from_pixel(8, 40, image::Rgba([20, 100, 180, 255]))
        .save(&second)
        .unwrap();

    let mut run = Run::spawn(home.path(), &fixture);
    run.wait_for("preview, help, and fitted status", |output| {
        let screen = output.parser.screen();
        let contents = screen.contents();
        contents.contains("Dots 569x1024 ")
            && contents.contains("Parameters")
            && contents.contains("?: keys")
            && contents.contains("Fit (sextant)")
            && screen.alternate_screen()
            && screen.bracketed_paste()
            && screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None
    });
    // Without frame markers every wait would also accept partial frames.
    assert!(
        run.output
            .lock()
            .unwrap()
            .raw
            .windows(END_FRAME.len())
            .any(|window| window == END_FRAME),
        "rthid frames are not wrapped in synchronized updates"
    );

    run.send(b"+");
    let mut zoomed = None;
    run.wait_for("zoomed status", |output| {
        zoomed = view_range(output);
        zoomed.is_some()
    });
    let zoomed = zoomed.unwrap();

    run.send(b"\x1b[<65;20;10M");
    let mut scrolled = None;
    run.wait_for("mouse wheel pan", |output| {
        scrolled = view_range(output).filter(|range| range.1 != zoomed.1);
        scrolled.is_some()
    });
    let scrolled = scrolled.unwrap();

    run.send(b"\x1b[<0;20;10M\x1b[<32;16;8M\x1b[<0;16;8m");
    run.wait_for("mouse drag pan", |output| {
        view_range(output).is_some_and(|range| range != scrolled)
    });
    run.send(b"-");
    run.wait_for_screen("Fit (sextant)");

    let paste = format!("\x1b[200~{}\x1b[201~", second.display());
    run.send(paste.as_bytes());
    run.wait_for_screen("Dots 8x40 ");

    run.resize(36, 6);
    run.wait_for("36x6 frame", |output| {
        let screen = output.parser.screen();
        screen.size() == (NonZeroU16::new(6).unwrap(), NonZeroU16::new(36).unwrap())
            && screen
                .rows(0, 36)
                .next()
                .is_some_and(|row| row.contains("rthid") && row.contains("Parameters"))
            && screen
                .rows(0, 36)
                .nth(5)
                .is_some_and(|row| row.starts_with("Card "))
    });
    run.resize(100, 30);
    run.wait_for_screen("Fit (sextant)");

    assert!(
        !run.output
            .lock()
            .unwrap()
            .raw
            .windows(b"\x1b[2J".len())
            .any(|window| window == b"\x1b[2J"),
        "rthid must erase individual rows instead of the whole display"
    );

    run.quit();
    let status = run.wait_for_exit();
    assert!(status.success(), "rthid exited with {status:?}");
    run.wait_for("terminal modes to be restored", |output| {
        let screen = output.parser.screen();
        !screen.alternate_screen()
            && screen.mouse_protocol_mode() == vt100::MouseProtocolMode::None
            && !screen.bracketed_paste()
    });
}

#[test]
fn sessions_save_restore_from_cli_and_flush_at_quit() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("photo.png");
    image::RgbaImage::from_pixel(8, 12, image::Rgba([40, 100, 180, 255]))
        .save(&source)
        .unwrap();
    let source_bytes = std::fs::read(&source).unwrap();
    let session = home.path().join("photo.rthid");
    let mut run = Run::spawn(home.path(), &source);
    run.wait_for_screen("Dots 8x12 ");
    run.send(b"n ");
    run.wait_for_screen("Dithered");
    run.send(b"\x13");
    run.wait_for_screen("saved photo.rthid");
    assert!(
        std::fs::read_to_string(session.join("manifest.toml"))
            .unwrap()
            .contains("name = \"Layer 1\"")
    );
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    run.quit();
    assert!(run.wait_for_exit().success());

    let mut run = Run::spawn(home.path(), &session);
    run.wait_for_screen("Dithered");
    run.send(b"f\x13\x03");
    run.exiting = true;
    assert!(run.wait_for_exit().success());
    let manifest = std::fs::read_to_string(session.join("manifest.toml")).unwrap();
    assert!(manifest.contains("flip = true"));
    assert!(!session.join("source.rthid").exists());
    let mut run = Run::spawn(home.path(), &session.join("source-1.png"));
    run.wait_for_screen("Dithered");
    run.quit();
    assert!(run.wait_for_exit().success());
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
}

#[test]
fn cli_rejects_exports_inside_sessions_before_starting_the_terminal() {
    let home = tempfile::tempdir().unwrap();
    let source = home.path().join("photo.png");
    image::RgbaImage::new(8, 12).save(&source).unwrap();
    let session = home.path().join("photo.rthid");
    let mut run = Run::spawn(home.path(), &source);
    run.wait_for_screen("Dots 8x12 ");
    run.send(b"n\x13");
    run.wait_for_screen("saved photo.rthid");
    run.quit();
    assert!(run.wait_for_exit().success());
    let source_before = std::fs::read(session.join("source-1.png")).unwrap();
    for option in ["--preview-png", "--mass-svg"] {
        for name in ["source-1.png", "exposure-1-0.png", "new.png"] {
            let output = std::process::Command::new(env!("CARGO_BIN_EXE_rthid"))
                .arg(&session)
                .arg(option)
                .arg(session.join(name))
                .output()
                .unwrap();
            assert!(!output.status.success());
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(error.contains("inside the session directory"), "{error}");
        }
    }
    assert_eq!(
        std::fs::read(session.join("source-1.png")).unwrap(),
        source_before
    );
}

#[test]
fn cli_reports_invalid_sessions_before_starting_the_terminal() {
    let home = tempfile::tempdir().unwrap();
    let session = home.path().join("broken.rthid");
    std::fs::create_dir(&session).unwrap();
    std::fs::write(session.join("manifest.toml"), "version = 2").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rthid"))
        .arg(&session)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("Could not load"));
    assert!(error.contains("manifest.toml"));
}
