# Status line notices while loading

## Context

The model was written against commit `b3bbdc6`. Since then the notice became `Notice { source, text }` with `Source::Config` or `Source::Other`, and save bookkeeping moved from `App` into `store::Library` in `src/store.rs`. The names below are the current ones. The checked behavior did not change, because every config notice text starts with `config:`, so tagging by source matches the old prefix test.

The status line shows one message at a time. It lives in `App.notice` in `src/app.rs`, and many independent sources write to it.

- `App::request_open` writes the `Loading <name>` text into the same slot.
- While an open request is pending (`App::loading`), `App::report_notice` parks every other message in `App.deferred_notice`, which holds one message. `App::complete` for `Opened` clears the slot and moves the deferred message back only if the slot is still empty.
- User input clears the slot in `App::handle_key` and `App::apply_step`. A current `Listed(Ok)` clears it in `App::set_listing`. All three skip the clear while loading, so that the Loading text survives.
- A config success clears a config notice in `App::save_notice` (`SaveNotice::ClearConfig`). It looks at `deferred_notice` while loading and at `notice` otherwise. `Exported(Ok)` in `App::complete` clears a notice whose text equals `Export::status` (`src/export.rs`).
- `Library::save_failed` reports a save error only when its text differs from `save_error`. A repeated error returns `SaveNotice::Unchanged`.
- Results carry tags. `Opened` is matched against `open_request.token`. `Listed` carries the dialog generation, but `Listed(Err)` sets the notice without checking it.

Each rule is reasonable alone. Together they decide what the user reads after a load finishes, and that depends on how user input, IO completions and draws interleave. A TODO on `App.deferred_notice` proposes deriving Loading from `open_request`. The models check the current code and that proposal.

## Model

`Notices.tla` models the current code. `NoticesFix.tla` models the proposal, with switches for its variants.

- Messages are records `[src, txt, id]`. Sources are validation (`val`), export (`exp`), config save failure (`cfgfail`), config load warning (`cfgwarn`), open error (`open`) and listing error (`list`). Each report gets a fresh `id`. `resolved[src]` records which ids have a resolved cause, so the model can tell a stale message from a current one.
- The event loop has two phases, like `event_loop` in `src/main.rs`. In the IO phase any number of completions apply, then one draw happens (`EndIO`). In the input phase any number of user events apply, then `App::queue_save` runs (`EndInput`). A draw is the only moment a message counts as seen. `BatchIO = FALSE` forces a draw after every completion. This removes interleavings that need two results to land in the same poll.
- IO keeps only the newest job of each kind (`IO::enqueue`, `IO::poll` in `crates/rthid-io/src/io.rs`). Each kind has at most one pending result, and updates are always current.
- User actions: a plain key, a setting change (queues an export), `o`, Esc in the dialog, entering a directory, picking a file, dropping a file, opening the preset editor, an invalid name, a valid save, Esc in the editor.
- IO results: `Opened` Ok or Err, `Listed` Ok or Err, `Exported` Ok or one of two error texts, `Updated` Ok, Ok with a load warning, or one of two error texts.
- Bounds: `MaxInput = 6` user actions and `MaxMsg = 5` reported messages.

## Properties

- **A1** After loading finishes, the notice is never a message whose cause has been resolved since. Examples are a validation error followed by a successful save, an export error followed by an export success, a listing error followed by a good listing of the same dialog, and a config error followed by a config success. `A1val`, `A1exp`, `A1list`, `A1cfg` and `A1open` split A1 by source.
- **A2** A reported config warning or save failure is drawn at least once, or still waits in a slot, unless a config success cleared it.
- **A3** A `Listed` result for a dialog generation that is no longer current, or an `Opened` result for a request that is no longer current, never changes the notice or the deferred notice.
- **A4a** While loading, the status text is Loading.
- **A4b** When loading finishes, the newest unresolved message reported during the load is shown. Export errors count as shown, since `App::export_status` keeps them on the status line.
- **A4p** (proposal only) A4b restricted to config messages.

## Results

Current code (`Notices.tla`):

| Property | Result |
| --- | --- |
| A1 | FAIL for validation, export and listing messages. PASS for config and open messages. |
| A2 | FAIL |
| A3 | FAIL for `Listed(Err)`. PASS for `Opened`, since IO coalescing and the token check drop stale results. |
| A4a | PASS |
| A4b | FAIL |

Counterexamples, as rthid scenarios:

- **A1, validation error** (`Notices_A1.cfg`). The user drops `photo.png` on the terminal. `App::handle_paste` calls `App::request_open`, which shows Loading. While it loads, the user presses `P` and submits an empty name. The editor returns `Step::notice` (`src/modal.rs`), and `App::apply_step` passes the error to `App::set_notice`, which parks it in `deferred_notice`. The user fixes the name and saves. `App::commit` stores the change, and the loading guard in `App::apply_step` skips the clear. `Opened(Ok)` arrives and `App::complete` restores the deferred validation error. The status line complains about a preset that was saved.
- **A1, export error** (`Notices_A1exp.cfg`, needs `--preview-png`). A setting change queues an export. The user drops a file. `Exported(Err)` is deferred. Another setting change queues an export and `Exported(Ok)` arrives. The notice is the Loading text, so the comparison with `Export::status` in `App::complete` fails and the deferred copy survives. `Opened(Ok)` restores the export error although the last export succeeded.
- **A1, listing error** (`Notices_A1list.cfg`). The user presses `o` and opens a typed path before the first listing arrives. That listing fails while the file loads and is deferred. The user enters a directory and `Listed(Ok)` fills the dialog, but `App::set_listing` skips the clear while loading. `Opened(Ok)` closes the dialog and restores the listing error, which no longer applies to anything.
- **A2, overwritten while loading** (`Notices_A2.cfg`). The user saves a preset, then drops a file and opens the editor again. The background save fails. `Library::complete` calls `Library::save_failed`, which records `save_error` and returns `config: ...`, and `App::save_notice` parks it in `deferred_notice`. The user then submits an invalid name, which replaces the deferred config error. The config error is never drawn. Later retries fail with the same text, which `Library::save_failed` suppresses. The failure stays invisible until a retry succeeds or quit reports it.
- **A2, replaced before a draw** (`Notices_A2_batch.cfg`). A save succeeds with a load warning, which `Library::saved` returns for `App::save_notice` to show. An `Exported(Err)` lands in the same `IO::poll` loop of `event_loop`, so the warning is replaced before any draw.
- **A3** (`Notices_A3.cfg`). The user presses `o`, which opens dialog generation 1 and queues its listing job, then Esc. `Listed(Err)` for generation 1 arrives and `App::complete` sets the notice for a dialog that is gone.
- **A4b** (`Notices_A4b.cfg`). The user presses `o` and picks a file before the listing arrives. `Listed(Err)` is deferred during the load. `Opened(Err)` writes the open error into the empty slot, and `App::complete` drops the deferred message because it only fills an empty slot. A deferred config failure is lost the same way (see A4p below).

Proposal (`NoticesFix.tla`). The notice is `Option<Notice { source, text }>`. Loading is derived from `open_request` and `deferred_notice` is gone. `App::request_open` clears the notice first. Among listing results, only a current `Listed(Ok)` clears the notice. User input clears it even while loading.

| Property | As proposed | With the rules in Decision |
| --- | --- | --- |
| A1 | PASS | PASS |
| A2 | FAIL | PASS |
| A3 | FAIL | PASS |
| A4a | PASS | PASS |
| A4b | FAIL | FAIL (accepted, see Consequences and limits) |
| A4p | FAIL | PASS |

- **A2 as proposed** (`NoticesFix_A2.cfg`). Same as the batch case above. One slot means a newer notice replaces an undrawn config warning. With `BatchIO = FALSE` (`NoticesFix_A2_frame.cfg`), a config failure reported while Loading is shown is cleared before the user sees it. Any key press clears it, and so does a second drop through `App::request_open`.
- **A3 as proposed** (`NoticesFix_A3.cfg`). Unchanged from the current code. `Listed(Err)` of a closed dialog still sets the notice. `NoticesFix_A3_gencheck.cfg` drops a `Listed(Err)` whose tag is not the current dialog generation, and passes.
- **A4b as proposed** (`NoticesFix_A4b.cfg`). `Opened(Err)` replaces a listing error reported during the load. Since user input clears while loading, Esc during the load also clears a message the user never saw.
- **Keeping the notice while loading does not help** (`NoticesFix_A1_keepwhileloading.cfg`). If input stops clearing the notice while loading, A1 fails again. The validation error survives the successful save that happened during the load. A1 needs input to clear while loading. So the loss of transient messages during a load has to be accepted, and persistent conditions must not depend on the transient slot.

## Decision

The status line code follows these rules. `NoticesFix_final.cfg` and `NoticesFix_final_frame.cfg` check them together (A1, A2, A3, A4a, A4p).

1. Loading is derived from `open_request` and drawn ahead of the notice. It is never stored in the notice slot, and `deferred_notice` goes away. A4a then holds by construction.
2. The notice is `Option<Notice { source, text }>`. A source clears only its own notice. `Exported(Ok)` clears an export notice and a config success clears a config notice. There is no text comparison and no `config:` prefix test. The current code already tags config notices (`Source::Config`) and clears them by source in `App::save_notice`. Export notices are still matched by text.
3. `App::request_open` clears the notice. `Opened(Ok)` leaves the notice alone, since anything in it was reported after the request. `Opened(Err)` sets the open error.
4. User input clears the notice in `App::handle_key` and `App::apply_step`, even while loading.
5. Listing results are routed by dialog generation. A current `Listed(Ok)` clears the notice. A `Listed(Err)` whose tag is not the current dialog generation is dropped.
6. Persistent conditions are drawn from their own state when the notice is empty, not only through the transient notice. These are the export error from `Export::status` (as today), the config save error from `save_error` in `store::Library`, and the warnings and save blocking of the last config load. They stay until their own success clears them. Replacing or clearing the transient notice cannot hide them, and the once per text rule of `Library::save_failed` no longer hides repeated failures.

## Consequences and limits

- A transient message (validation, listing or open error) can still be replaced by a newer one, or dismissed by a key press, while Loading is on screen. So A4b keeps failing. This is accepted. The newest message wins, and a key press is a dismissal. Rule 6 protects persistent conditions instead.
- The model abstracts texts to two values per source. It cannot find bugs that depend on two different sources producing equal text.
- Not modelled: `Opened(Ok(Dir))` and the dialog it opens without a request, graphics errors, the startup `saving disabled` notice beyond one initial warning, timers (a save retry may happen at any input batch), and the contents of listings.
- Draws are taken at the boundary between the IO phase and the input phase, which matches `event_loop`. Real draws are skipped when nothing changed. That cannot make a message visible that the model hides.
- The bounds are small (6 user actions, 5 messages). Every counterexample found has at most 12 steps, well below the bounds. Longer scenarios are not covered.

## How to run

TLC needs a Java runtime and `tla2tools.jar` from the TLA+ project releases. Run from `specs/notices`:

```sh
java -cp <path-to>/tla2tools.jar tlc2.TLC -workers auto -deadlock -config <config> <spec>
```

For example, `-config Notices_A1.cfg Notices.tla`. Violations show up within seconds. Passing runs take a minute or a few.

| Config | Spec | Expected | Distinct states (for reference) |
| --- | --- | --- | --- |
| `Notices_A1.cfg` | `Notices.tla` | A1 violated | |
| `Notices_A1exp.cfg` | `Notices.tla` | A1exp violated | |
| `Notices_A1list.cfg` | `Notices.tla` | A1list violated | |
| `Notices_A1cfg.cfg` | `Notices.tla` | no error | 9.8 M |
| `Notices_A1open.cfg` | `Notices.tla` | no error | 9.8 M |
| `Notices_A2.cfg` | `Notices.tla` | A2 violated | |
| `Notices_A2_batch.cfg` | `Notices.tla` | A2 violated | |
| `Notices_A3.cfg` | `Notices.tla` | A3 violated | |
| `Notices_A4a.cfg` | `Notices.tla` | no error | 9.8 M |
| `Notices_A4b.cfg` | `Notices.tla` | A4b violated | |
| `NoticesFix_A1.cfg` | `NoticesFix.tla` | no error | 19.8 M |
| `NoticesFix_A1_keepwhileloading.cfg` | `NoticesFix.tla` | A1 violated | |
| `NoticesFix_A2.cfg` | `NoticesFix.tla` | A2 violated | |
| `NoticesFix_A2_frame.cfg` | `NoticesFix.tla` | A2 violated | |
| `NoticesFix_A2_fallback.cfg` | `NoticesFix.tla` | no error | 19.3 M |
| `NoticesFix_A2_frame_fallback.cfg` | `NoticesFix.tla` | no error | 14.2 M |
| `NoticesFix_A3.cfg` | `NoticesFix.tla` | A3 violated | |
| `NoticesFix_A3_gencheck.cfg` | `NoticesFix.tla` | no error | 16.6 M |
| `NoticesFix_A4a.cfg` | `NoticesFix.tla` | no error | 19.8 M |
| `NoticesFix_A4b.cfg` | `NoticesFix.tla` | A4b violated | |
| `NoticesFix_A4p.cfg` | `NoticesFix.tla` | A4p violated | |
| `NoticesFix_final.cfg` | `NoticesFix.tla` | no error | 16.0 M |
| `NoticesFix_final_frame.cfg` | `NoticesFix.tla` | no error | 14.2 M |

Traces print the `step` variable, which names the user action or IO result of each state.
