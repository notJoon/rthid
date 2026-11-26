# rthid-io

The filesystem side of rthid, used by the binary to keep App and dialogs free of
I/O. Use synchronous functions for individual operations, or `io::IO` to keep
only the latest requested work of each kind in the background.

## Quick start

This crate is not published to crates.io. Add it to your app's `Cargo.toml`,
adjusting the path to your checkout. The examples use `tempfile` for scratch files:

```toml
[dependencies]
rthid-io = { path = "../rthid/crates/rthid-io" }
tempfile = "3.27.0"
```

Put this in `src/main.rs` and run `cargo run`. It updates text under an advisory
lock and replaces the file only after the write succeeds:

```rust
use std::{fs, io, time::Duration};
use rthid_io::atomic;

fn main() -> io::Result<()> {
    let scratch = tempfile::tempdir()?;
    let path = scratch.path().join("config/settings.txt");
    let lines = atomic::update(&path, Duration::from_secs(1), |current| {
        let mut text = current.unwrap_or_default();
        text.push_str("theme=dark\n");
        let lines = text.lines().count();
        Ok((text, lines))
    })?;
    assert_eq!(lines, 1);
    assert_eq!(fs::read_to_string(path)?, "theme=dark\n");
    Ok(())
}
```

`atomic::update` creates missing parent directories and reads the latest text
while holding a sibling `.lock` file. A failed transform or write preserves the
previous file. For binary output, `atomic::replace` accepts a file-writing closure;
its parent directory must already exist.

## Modules

| Module | Purpose |
| --- | --- |
| `atomic` | File replacement through a temporary file beside the destination |
| `dir` | Visible directories and supported image files for the open dialog |
| `image` | Preview decoding, shrinking and protection from overwriting the source |
| `io` | Latest-only background jobs, typed results, and bounded shutdown |
| `paste` | Paths carried by terminal paste events |
| `path` | Home directory expansion for entered paths |

## Background export

Submit an owned encoder, then inspect its result when shutting down:

```rust
use std::io::Write;
use rthid_io::io::{IO, Job, Outcome};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scratch = tempfile::tempdir()?;
    let path = scratch.path().join("preview.png");
    let bytes = b"exported bytes".to_vec();
    let mut worker = IO::new(None)?;
    worker.submit(Job::Export {
        path: path.clone(),
        write: Box::new(move |file| file.write_all(&bytes)),
    }, 7);
    let results = worker.finish();
    assert_eq!(results.len(), 1);
    let done = results.into_iter().next().unwrap();
    assert_eq!(done.tag, 7);
    match done.outcome {
        Outcome::Exported(result) => assert_eq!(result?, path),
        _ => unreachable!("only an export was submitted"),
    }
    assert!(path.is_file());
    Ok(())
}
```

During an app's event loop, use `poll` to retrieve available results without
waiting. `busy` means a latest result has not yet been polled, rather than that
the worker is necessarily still executing it. Results retain the caller's `tag`,
operation, path, elapsed time, and typed errors.

## Latest-only jobs and shutdown

`IO` is not a queue that guarantees every submitted write will run. It has one
queued slot for each of Open, List, Export and Update. A newer submission replaces
queued work of the same kind **even when the destination paths differ**. Already
running work may still change the filesystem, but superseded results are discarded.
Different kinds do not replace each other, although they share one worker thread.

This suits one current source, companion PNG and configuration file. Use the
synchronous functions when every operation must run, such as saving independent
files. An Update edit must contain all changes not yet confirmed saved; submitting
individual incremental edits can lose earlier ones that were replaced in the queue.

Collect reads with `poll` before calling `finish`. Stopped jobs are reported as
skipped because the IO worker shut down, rather than as file failures.

`finish` skips queued reads and updates and waits up to five seconds for running
work and the latest queued export. It returns results, including stopped or timed
out jobs, and detaches a worker still running at the deadline. It does not cancel
an operation already running. Dropping `IO` uses the same bounded wait and discards
the results.

Keep unsaved configuration changes until the matching Update succeeds. On exit,
call `finish`, apply its matching successful Update results, then synchronously
save any remaining changes with `atomic::update`. Merge those changes into the
fresh text passed to the transform, as during normal saves. An `Edit` that has
already started running may still finish; a shutdown timeout is not cancellation.

## File and preview policies

- `atomic::replace` and Export jobs overwrite the resolved destination while
  keeping symlinks intact. Saving splits hard links and may replace read-only
  files when the parent allows it. The `.rthid-*.tmp` namespace is reserved: on
  first use of a resolved parent per process, stale matching regular files older
  than one hour are removed when possible. Do not store other files under those names.
- Atomic replacement does not by itself guarantee crash durability. `replace`
  does not sync files or directories. `update` syncs the replacement file and,
  on Unix, attempts to sync the parent directory after committing it.
- `image::load_image` returns preview pixels: images larger than 1024 pixels on
  their longest side are resized with the Triangle filter, preserving aspect
  ratio. Decoding rejects sides above 16384 pixels and requests a 256 MiB decoder
  allocation limit. Use an image decoder directly to retain full resolution.
- `dir::list_dir` is an image-picker listing: it hides dotfiles and unsupported
  file extensions, sorts directories first, and adds `..` when there is a parent.

See the crate docs (`cargo doc -p rthid-io --open`) for the API.

For PNG output, build a grid with [rthid-render](../rthid-render) and pass an
encoder closure to `Job::Export`. IO replaces the destination only on success.
`atomic::read_regular` and `atomic::update` report paths with generic file wording.
Lock timeouts retain `WouldBlock`, and file type failures retain `InvalidInput`.

Open jobs report filesystem failures as `Cause::Io`, including errors from the
image decoder, so callers can use `kind()` just as for export failures. Real
decode errors remain `Cause::Decode`. `Cause::IsSource(path)` carries the protected
source path and has no CLI flag wording. `WORKER_NAME` and `panic_message` are
intended for process panic hooks that need to recognize and report IO worker panics.
