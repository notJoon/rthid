# Configuration saves across processes

## Context

The model was written against commit `b3bbdc6`. Since then the save fields (`unsaved`, `dirty`, `saving`, `save_retry`, `save_error`) and the save logic moved from `App` into `store::Library` in `src/store.rs`. `App::queue_save` now delegates to `Library::save_job`, the `Updated` branch of `App::complete` delegates to `Library::complete`, and `App::saved` and `App::save_failed` became `Library::saved` and `Library::save_failed`. The names below are the current ones. The checked behavior did not change.

Palette presets and templates are saved to one TOML file that several rthid processes may share. The pipeline has four layers.

- **App and Library** (`src/app.rs`, `src/store.rs`). `Library::commit` keeps the last change per list and name in `unsaved` and sets `dirty`. `App::queue_save` runs after every input batch and when the retry deadline passes. It calls `Library::save_job`, which clears `dirty` and `save_retry` and returns an `Update` job with a new token and a copy of `unsaved` as `saving`. `Library::complete` ignores results whose tag is not `saving.token`. On success it removes the saved changes from `unsaved` by value (`unsaved.retain`). On failure it sets `save_retry` and calls `Library::save_failed`, which reports a text only when it differs from `save_error`.
- **IO** (`crates/rthid-io/src/io.rs`). One worker thread runs every job. Each kind keeps only its newest job. A new `Update` replaces a queued one, and `IO::poll` drops results whose id is not the newest. On shutdown `IO::finish` sets a stop flag, so queued updates fail with `Cause::Stopped`. It waits up to `SHUTDOWN_WAIT` (5 s), reports `TimedOut` for anything still pending, and detaches a worker that is still inside a job.
- **Store** (`src/store.rs`, `crates/rthid-io/src/atomic.rs`). `atomic::update` takes a lock on a sibling `.lock` file. It waits `SAVE_WAIT` (50 ms) in the background and one second at quit. It then reads the file again, `store::merge` applies only the named changes, and the text goes to a temp file, is synced and renamed over the config.
- **Quit** (`src/main.rs`). `event_loop` skips `App::queue_save` once quit is requested. `main` then runs `IO::finish`, applies the returned `Updated` results with `App::complete`, and calls `save_on_quit` with `App::unsaved`. Its error becomes the exit status.

The subtle parts are a background job that is still running when a newer one is queued, removal from `unsaved` by value, the once per text failure report, and the shutdown path where a timed out worker keeps running next to `save_on_quit`.

## Model

`Saves.tla` has a set of processes `Procs` sharing one file, a map from names (`Keys`) to values (`Vals`).

- An edit sets a key to a value in `unsaved` (last change per key), like `Library::commit`. Removal is not a separate case. It is one more value.
- Each process has the App and Library state, an IO slot, the running job, the newest pending id and the newest unpolled result. The worker steps are take, start (checks the stop flag), lock, write or fail. Write covers read, merge and rename under the lock in one step.
- Faults: lock busy and write failure, bounded by `MaxFail` per process. `PermanentFail` makes every write fail after the first failure, like a merge that rejects an edit or a read only directory. `AllowTimeout` lets `IO::finish` time out while the worker is inside a job. `QuitSameBatch` lets `q` arrive in the same input batch as an edit, so `App::queue_save` is skipped for that edit.
- Polls only happen with `dirty` clear, because `App::queue_save` always runs between an input batch and the next poll.
- Bookkeeping for the properties only: every edit gets a sequence number per process and the commit counter at the time it was made. The file remembers which edit each key holds, and `confirmed` records the newest edit an accepted `Ok` covered.
- Bounds: two processes, one key, two values, two edits and one injected failure per process. The merge check uses two keys and one value. Liveness uses one edit per process, or one process with two edits and two failures.

## Properties

- **B1a** An edit confirmed by a successful save stays in the file unless a later rename wrote the same key. This checks that reading again and merging under the lock prevents lost updates across processes.
- **B1b** The file never goes back to an older edit of a process after a newer edit of that process for the same key was confirmed.
- **B2** (liveness, `FairSpec`) Every edit is eventually saved, replaced by a newer local edit, or covered by a failure report. Fairness is weak on every system step and strong on taking a free lock. Failures are bounded.
- **NoSilentFail** (safety side of B2) A failed save never covers an edit that no failure report covers.
- **B3** `saving` and `save_retry` are never set together.
- **B4** A result with an old tag never changes `unsaved`.
- **B4ext** No accepted result removes an edit made after its job was queued.
- **B5** A process that exits without an error leaves each of its newest edits in the file, unless another process renamed a value for that key after the edit was made.

## Results

| Property | Result |
| --- | --- |
| B1a | PASS in every configuration, including all faults and two keys |
| B1b | PASS without shutdown timeouts. FAIL with `AllowTimeout`. |
| B2 | PASS with bounded failures (2 processes with 1 edit, 1 process with 2 edits). The wider run is deferred. |
| NoSilentFail | FAIL, with transient and with permanent failures |
| B3 | PASS in every configuration |
| B4 | PASS in every configuration |
| B4ext | FAIL. Harmless alone, its effect is the B5 failure. |
| B5 | PASS unless `q` arrives in the same input batch as an edit. FAIL with `QuitSameBatch`. |

Counterexamples, as rthid scenarios. All of them reproduce code paths that exist at `b3bbdc6` and on the current branch.

- **B1b, detached worker after a shutdown timeout** (`Saves_B1_timeout.cfg`). The user saves preset X with value b. `Library::save_job` creates job 1 and the worker starts it, then stalls before taking the lock. The stall is inside `atomic::update`, for example resolving the path or opening the `.lock` file on a hung network mount. The user changes X to a, `App::queue_save` queues job 2, and the user quits. `IO::finish` sets the stop flag, waits 5 s, reports job 2 as `TimedOut` and detaches the worker, which is still in job 1. `Library::complete` keeps X=a unsaved, and `save_on_quit` writes it and succeeds. Before the process exits, the worker resumes and takes the lock that `save_on_quit` just released. It merges its old change and renames X=b over X=a. rthid exits with status 0 and the file holds the older edit. This needs a filesystem stall of more than 5 s and a race in the short window between `save_on_quit` and process exit, so it is rare.
- **B5, equal value edit dropped at shutdown** (`Saves_B5_samebatch.cfg`). Process B saves template T with content b. Job 1 renames it, but its `Ok` has not been polled yet. Process A quits and its `save_on_quit` writes T=a. In B the user saves T again with the same content b and presses `q` in the same input batch, so `event_loop` skips `App::queue_save`. In `main`, `IO::finish` returns the `Ok` of job 1. `Library::complete` removes the new edit from `unsaved`, because `unsaved.retain` compares `Change` values and the new edit equals the saved one. `save_on_quit` has nothing to write and B exits with status 0. The file keeps T=a from A although the save in B came later. This needs the second save and `q` in one drain of `event::read` and an identical value, so the impact is small.
- **NoSilentFail, transient** (`Saves_silent.cfg`). A save fails, for example with "config is busy" because another rthid holds the lock longer than `SAVE_WAIT`. `Library::save_failed` returns `config: config is busy` and records `save_error`. The user presses a key, which clears the notice. The user saves another preset, and that save fails with the same text, so `Library::save_failed` returns `SaveNotice::Unchanged`. The new edit is unsaved with nothing on screen until a retry succeeds. B2 holds, so a retry eventually does.
- **NoSilentFail, permanent** (`Saves_silent_permanent.cfg`). Same, with a failure that repeats, like a merge that rejects an edit or a read only config directory. Every later edit stays unsaved without any report until quit. There `save_on_quit` fails and `main` returns the error.

Why the passing properties hold:

- B1a: `atomic::update` holds the sibling lock across read, merge and rename, and `store::merge` touches only the named entries. The rename does not replace the lock file, so all writers contend on the same lock.
- B3: `Library::save_job` clears `save_retry` before it sets `saving`. The error path of `Library::complete` takes `saving` before it sets `save_retry`.
- B4: `IO::poll` returns only the newest id, and `Library::complete` drops tags other than `saving.token`.
- B2: every failure sets `save_retry`, `event_loop` wakes at the deadline, and `App::queue_save` resubmits all unsaved edits. With bounded failures a save eventually succeeds, and quit reports what is left.

## Decision

- Keep the save design: last change per key, newest job wins, read again and merge under a lock file next to the config, rename to commit.
- A worker must not commit an `Update` after shutdown started. Check the stop flag again after the lock is taken in `atomic::update`, or in `execute` for `Job::Update` in `crates/rthid-io/src/io.rs`, and fail with `Cause::Stopped`. Then a detached worker cannot rename after `save_on_quit`.
- Removal from `unsaved` after a successful save in `Library::complete` should match edit identity, not value. Tag each change with an edit sequence number and remove only changes whose number the job covers.
- A save failure must stay visible while edits remain unsaved. Render `save_error` as a persistent status (see the notices decision, rule 6) instead of relying on a once per text notice.

## Consequences and limits

- Not modelled: `save_blocked`, export jobs that share the worker and delay updates (only covered through the timeout), symlink resolution, the temp file sweep, and the directory sync after the rename. When configuration loading blocked saving, `Library::unsaved` is empty. The edits of that session are dropped at quit and only the startup notice reports it.
- The lock is assumed to exclude other open file descriptions in the same process too. This is how `File::try_lock` (flock) behaves on Unix.
- The retry deadline and lock wait times are abstract. Any failure may happen whenever the lock is held elsewhere, within the failure bound.
- Values are compared by key and value only, like `Change` equality. Richer content (colors, modes) cannot make two changes equal that the model keeps apart. It can make the B5 scenario more or less likely.
- Bounds are small. The counterexamples need at most 18 steps.

## How to run

TLC needs a Java runtime and `tla2tools.jar` from the TLA+ project releases. Run from `specs/saves`:

```sh
java -cp <path-to>/tla2tools.jar tlc2.TLC -workers auto -deadlock -config <config> Saves.tla
```

Violations show up within seconds. Passing runs take seconds to a few minutes.

| Config | Expected | Distinct states (for reference) |
| --- | --- | --- |
| `Saves_safety.cfg` | no error (B1a, B1b, B3, B4, B5) | 5.4 M |
| `Saves_B3B4_faults.cfg` | no error (B1a, B3, B4 with every fault) | 23.6 M |
| `Saves_merge_2keys.cfg` | no error (B1a, B3, B4 with two keys) | 20.8 M |
| `Saves_B1_timeout.cfg` | B1b violated | |
| `Saves_B5_samebatch.cfg` | B5 violated | |
| `Saves_B4ext.cfg` | B4ext violated | |
| `Saves_silent.cfg` | NoSilentFail violated | |
| `Saves_silent_permanent.cfg` | NoSilentFail violated | |
| `Saves_B2.cfg` | no error (B2, 2 processes with 1 edit) | 37 k |
| `Saves_B2_1proc.cfg` | no error (B2, 1 process with 2 edits and 2 failures) | 2.5 k |
| `Saves_B2_wide.cfg` | deferred, not run yet | about 6.7 M |

Deferred: the wide B2 liveness run has not been run. It covers two processes with two edits each and one failure per process. Its config is `Saves_B2.cfg` with `MaxEdits = 2`, saved as `Saves_B2_wide.cfg`. Run it with:

```sh
java -cp <path-to>/tla2tools.jar tlc2.TLC -workers auto -deadlock -config Saves_B2_wide.cfg Saves.tla
```

The liveness check over its state space takes a long time. The smaller bounds (`Saves_B2.cfg` and `Saves_B2_1proc.cfg`) pass.

Traces show the action name of each state. The `confirmed`, `fileSeq`, `touchedAt` and `lastEdit` variables are bookkeeping for the properties and have no counterpart in rthid.
