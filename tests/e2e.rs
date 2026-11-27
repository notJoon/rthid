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

struct Output {
    parser: vt100::Parser,
    raw: Vec<u8>,
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
                output.raw.extend_from_slice(&buffer[..count]);
                output.parser.process(&buffer[..count]);
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

    fn wait_for(&mut self, description: &str, mut predicate: impl FnMut(&Output) -> bool) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let timeout = {
                let output = self.output.lock().unwrap();
                if predicate(&output) {
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
            && contents.contains("o: open")
            && contents.contains("Fit (sextant)")
            && screen.alternate_screen()
            && screen.bracketed_paste()
            && screen.mouse_protocol_mode() != vt100::MouseProtocolMode::None
    });

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
