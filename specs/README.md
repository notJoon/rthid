# Formal models

This folder holds design records for rthid, each backed by a TLA+ model and checked with TLC. A record covers a state machine whose behavior depends on the order of user input, background IO results and other rthid processes. Its README states the problem, what the model abstracts, the properties, the results, and the design the code follows because of them.

| Folder | Specs | Question |
| --- | --- | --- |
| [notices](notices/README.md) | `Notices.tla`, `NoticesFix.tla` | Does the status line show the right message while files load and IO results arrive? |
| [saves](saves/README.md) | `Saves.tla` | Do configuration edits reach the shared config file, or get reported, across processes, failures and quit? |

The models name the Rust functions they stand for, for example `App::request_open` in `src/app.rs`. They never cite line numbers.

## Running TLC

TLC needs a Java runtime and `tla2tools.jar` from the [TLA+ project releases](https://github.com/tlaplus/tlaplus/releases).

Each `.cfg` file checks one property or one group of properties with fixed bounds. Its first line states the expected result. Run TLC from the model folder and pass the config and the spec:

```sh
cd specs/notices
java -cp <path-to>/tla2tools.jar tlc2.TLC -workers auto -deadlock -config Notices_A1.cfg Notices.tla
```

`-deadlock` turns off deadlock reports. Every model is bounded and stops in terminal states on purpose. TLC explores states breadth first, so a reported counterexample is a shortest one.

## Conventions

- Tracked: `<Model>.tla`, one `.cfg` per check, and the model README. A fixed design is its own spec (`<Model>Fix.tla`).
- Every `.cfg` appears in its README's How to run table with the expected result.
- Not tracked: TLC output (`states/`, `*_TTrace_*`, logs). Results are summarized in the README instead.
- The `.tla` files are the source and are edited by hand. There are no generator scripts.
- Changing a spec updates Results in the same commit. A changed result means revisiting Decision.
- Code changes justified by a Decision mention the model folder in the commit message.
- Long checks keep their `.cfg` and are marked deferred in the README.
