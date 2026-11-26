------------------------------- MODULE Saves -------------------------------
(***************************************************************************)
(* Config saves of rthid with several processes that share one config      *)
(* file. Written against commit b3bbdc6. The save state and its handlers   *)
(* have since moved from App into store::Library (src/store.rs), and the   *)
(* names below are the current ones.                                       *)
(*                                                                         *)
(*   - App and Library: App::commit and Library::commit, App::queue_save   *)
(*     and Library::save_job, App::complete forwarding Updated to          *)
(*     Library::complete, Library::saved and Library::save_failed, and the *)
(*     Library fields unsaved, dirty, saving, save_retry, save_error and   *)
(*     last_token.                                                         *)
(*   - IO (crates/rthid-io/src/io.rs): one worker thread, newest job per   *)
(*     kind (IO::enqueue, IO::poll), the stop flag checked before a job    *)
(*     runs (run), and IO::finish, which waits SHUTDOWN_WAIT, then reports *)
(*     TimedOut and detaches a worker still inside a job.                  *)
(*   - Store (store::save, store::merge) and atomic::update                *)
(*     (crates/rthid-io/src/atomic.rs): sibling lock file, re read, merge  *)
(*     of the unsaved changes, temp file and rename.                       *)
(*   - Quit: main after event_loop, and save_on_quit (src/main.rs).        *)
(*                                                                         *)
(* Abstracted: an edit is a value for a key (a preset or template name),   *)
(* and a removal is one more value. Every edit gets a per process sequence *)
(* number and the commit counter at the time it was made, used only by the *)
(* properties. Like Change, the app compares changes by value, not by      *)
(* sequence number. Lock waits and the retry deadline are not timed.       *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS Procs, Keys, Vals, MaxEdits, MaxFail,  \* processes, names, values and bounds
          AllowTimeout,   \* finish may time out with the worker still inside a job
          PermanentFail,  \* once a write failed it keeps failing (a merge rejection, a read only directory)
          QuitSameBatch,  \* 'q' may arrive in the same input batch as an edit
          MaxTok          \* state constraint bound for safety runs with PermanentFail

NoV == "none"   \* no value, also the absence of a change
\* no job: slot, running job or Library.saving is None
NoJob == [id |-> 0, tok |-> 0, ch |-> [k \in Keys |-> NoV], sq |-> [k \in Keys |-> 0],
          stage |-> "none", detached |-> FALSE]
\* no edit made yet
NoEdit == [v |-> NoV, seq |-> 0, at |-> 0]

VARIABLES
    file,        \* [Keys -> Vals \cup {NoV}] the shared config file, one entry per name
    fileSeq,     \* [Keys -> <<p, seq>>] which edit the file holds (bookkeeping)
    commits,     \* number of renames so far (bookkeeping)
    touchedAt,   \* [Keys -> Nat] commit index of the last rename that wrote the key (bookkeeping)
    touchedBy,   \* [Keys -> Procs \cup {0}] process of that rename (bookkeeping)
    lock,        \* holder of the sibling .lock file: <<>>, <<p, "w">> (IO worker) or <<p, "m">> (save_on_quit)
    st,          \* [Procs -> "run" | "finish" | "quitsave" | "exit_ok" | "exit_err" | "exited"] phase of main
    unsaved,     \* [Procs -> [Keys -> Vals \cup {NoV}]] Library.unsaved (App.unsaved at b3bbdc6)
    useq,        \* [Procs -> [Keys -> Nat]] seq of the edit held in unsaved (bookkeeping)
    lastEdit,    \* [Procs -> [Keys -> edit]] newest edit made (bookkeeping)
    nEdits,      \* [Procs -> Nat] edits made so far (bounded by MaxEdits)
    dirty,       \* Library.dirty
    saveRetry,   \* Library.save_retry is Some (the deadline itself is abstracted)
    saving,      \* [Procs -> job] Library.saving, the newest job and its changes (tok = 0 is None)
    lastTok,     \* Library.last_token, the tag of the newest save job
    slot,        \* [Procs -> job] queued Update job in the IO slot
    run,         \* [Procs -> job] job the IO worker is executing
    pend,        \* [Procs -> Nat] newest submitted Update id (IO pending)
    res,         \* [Procs -> result] newest result not yet polled
    nextId,      \* IO.next_id
    stop,        \* IO.stopping, set by IO::finish
    fails,       \* injected failures so far (bounded by MaxFail)
    broken,      \* with PermanentFail, writes keep failing from now on
    saveErr,     \* Library.save_error
    qs,          \* save_on_quit in progress: its changes and stage
    confirmed,   \* [Procs -> [Keys -> edit and commit]] newest edit confirmed by an Ok the app accepted (bookkeeping)
    done,        \* [Procs -> SUBSET Nat] edits saved, superseded locally, or reported failed (bookkeeping)
    exitErr,     \* main returned the error of save_on_quit
    silent,      \* NoSilentFail history flag: a failed save covered an edit that no report covered
    b4viol,      \* B4 history flag: a stale result changed unsaved
    b4ext        \* B4ext history flag: an accepted result removed a newer edit

\* no unpolled result
NoRes == [id |-> 0, tok |-> 0, ok |-> FALSE, err |-> "none", at |-> 0]

vars == <<file, fileSeq, commits, touchedAt, touchedBy, lock, st, unsaved, useq, lastEdit,
          nEdits, dirty, saveRetry, saving, lastTok, slot, run, pend, res, nextId, stop,
          fails, broken, saveErr, qs, confirmed, done, exitErr, silent, b4viol, b4ext>>

Init ==
    /\ file = [k \in Keys |-> NoV]
    /\ fileSeq = [k \in Keys |-> <<0, 0>>]
    /\ commits = 0
    /\ touchedAt = [k \in Keys |-> 0] /\ touchedBy = [k \in Keys |-> 0]
    /\ lock = <<>>
    /\ st = [p \in Procs |-> "run"]
    /\ unsaved = [p \in Procs |-> [k \in Keys |-> NoV]]
    /\ useq = [p \in Procs |-> [k \in Keys |-> 0]]
    /\ lastEdit = [p \in Procs |-> [k \in Keys |-> NoEdit]]
    /\ nEdits = [p \in Procs |-> 0]
    /\ dirty = [p \in Procs |-> FALSE] /\ saveRetry = [p \in Procs |-> FALSE]
    /\ saving = [p \in Procs |-> NoJob] /\ lastTok = [p \in Procs |-> 0]
    /\ slot = [p \in Procs |-> NoJob] /\ run = [p \in Procs |-> NoJob]
    /\ pend = [p \in Procs |-> 0] /\ res = [p \in Procs |-> NoRes]
    /\ nextId = [p \in Procs |-> 0] /\ stop = [p \in Procs |-> FALSE]
    /\ fails = [p \in Procs |-> 0] /\ broken = [p \in Procs |-> FALSE]
    /\ saveErr = [p \in Procs |-> "none"]
    /\ qs = [p \in Procs |-> NoJob]
    /\ confirmed = [p \in Procs |-> [k \in Keys |-> [v |-> NoV, seq |-> 0, at |-> 0]]]
    /\ done = [p \in Procs |-> {}]
    /\ exitErr = [p \in Procs |-> FALSE]
    /\ silent = FALSE
    /\ b4viol = FALSE /\ b4ext = FALSE

\* the change set ch is empty
Empty(ch) == \A k \in Keys : ch[k] = NoV
\* sequence numbers of the edits p still holds in unsaved
SeqsIn(p) == {useq[p][k] : k \in {kk \in Keys : unsaved[p][kk] # NoV}}
\* store::merge: only the named entries are replaced, all other entries are kept
Merge(f, ch) == [k \in Keys |-> IF ch[k] # NoV THEN ch[k] ELSE f[k]]
\* the process has not exited
Alive(p) == st[p] # "exited"
\* another failure may be injected
CanFail(p) == fails[p] < MaxFail

-----------------------------------------------------------------------------
(* Shared commit of a write by process p with change set ch and seqs sq *)
\* The rename of atomic::update: the merged text replaces the file
Commit(p, ch, sq) ==
    /\ file' = Merge(file, ch)
    /\ fileSeq' = [k \in Keys |-> IF ch[k] # NoV THEN <<p, sq[k]>> ELSE fileSeq[k]]
    /\ commits' = commits + 1
    /\ touchedAt' = [k \in Keys |-> IF ch[k] # NoV THEN commits + 1 ELSE touchedAt[k]]
    /\ touchedBy' = [k \in Keys |-> IF ch[k] # NoV THEN p ELSE touchedBy[k]]
    /\ done' = [done EXCEPT ![p] = @ \cup {sq[k] : k \in {kk \in Keys : ch[kk] # NoV}}]
\* no rename happens
NoCommit == UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy>>

-----------------------------------------------------------------------------
(* User *)

\* A preset or template save or removal in a modal: App::commit hands it to Library::commit,
\* which keeps the last change per key and sets dirty
Edit(p, k, v) ==
    /\ st[p] = "run" /\ nEdits[p] < MaxEdits
    /\ LET s == nEdits[p] + 1 IN
       /\ nEdits' = [nEdits EXCEPT ![p] = s]
       /\ unsaved' = [unsaved EXCEPT ![p][k] = v]
       /\ useq' = [useq EXCEPT ![p][k] = s]
       /\ lastEdit' = [lastEdit EXCEPT ![p][k] = [v |-> v, seq |-> s, at |-> commits]]
       \* the edit it replaces in unsaved no longer needs saving
       /\ done' = [done EXCEPT ![p] = IF unsaved[p][k] # NoV THEN @ \cup {useq[p][k]} ELSE @]
    /\ dirty' = [dirty EXCEPT ![p] = TRUE]
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, lock, st, saveRetry, saving,
                   lastTok, slot, run, pend, res, nextId, stop, fails, broken, saveErr, qs,
                   confirmed, b4viol, b4ext, exitErr, silent>>

\* 'q' in App::handle_key. It may arrive in the same input batch as edits, then
\* event_loop skips queue_save. IO::finish sets the stop flag.
Quit(p) ==
    /\ st[p] = "run" /\ (QuitSameBatch \/ ~dirty[p])
    /\ st' = [st EXCEPT ![p] = "finish"]
    /\ stop' = [stop EXCEPT ![p] = TRUE]
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, lock, unsaved, useq, lastEdit,
                   nEdits, dirty, saveRetry, saving, lastTok, slot, run, pend, res, nextId,
                   fails, broken, saveErr, qs, confirmed, done, b4viol, b4ext, exitErr, silent>>

-----------------------------------------------------------------------------
(* App::queue_save and Library::save_job, after an input batch or at the retry deadline *)

QueueSave(p) ==
    /\ st[p] = "run" /\ (dirty[p] \/ saveRetry[p])
    /\ dirty' = [dirty EXCEPT ![p] = FALSE]
    /\ saveRetry' = [saveRetry EXCEPT ![p] = FALSE]
    /\ IF Empty(unsaved[p])
       THEN UNCHANGED <<saving, lastTok, slot, pend, nextId>>
       ELSE LET job == [id |-> nextId[p] + 1, tok |-> lastTok[p] + 1, ch |-> unsaved[p],
                        sq |-> useq[p], stage |-> "start", detached |-> FALSE]
            IN /\ lastTok' = [lastTok EXCEPT ![p] = @ + 1]
               /\ nextId' = [nextId EXCEPT ![p] = @ + 1]
               /\ saving' = [saving EXCEPT ![p] = job]
               /\ slot' = [slot EXCEPT ![p] = job]        \* replaces a queued older job
               /\ pend' = [pend EXCEPT ![p] = job.id]
               \* an unpolled older result is now dropped by io.poll (id mismatch)
               /\ res' = [res EXCEPT ![p] = NoRes]
    /\ IF Empty(unsaved[p]) THEN UNCHANGED res ELSE TRUE
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, lock, st, unsaved, useq,
                   lastEdit, nEdits, run, stop, fails, broken, saveErr, qs, confirmed,
                   done, b4viol, b4ext, exitErr, silent>>

\* Library::complete for an Updated result r, reached through App::complete. Ok removes
\* the saved changes by value (unsaved.retain), Err sets save_retry and calls
\* Library::save_failed.
Complete(p, r) ==
    LET cur == saving[p].tok # 0 /\ r.tok = saving[p].tok
        sv  == saving[p]
        kept == [k \in Keys |-> IF unsaved[p][k] # NoV /\ unsaved[p][k] = sv.ch[k]
                                THEN NoV ELSE unsaved[p][k]]
        removedNewer == \E k \in Keys : unsaved[p][k] # NoV /\ kept[k] = NoV
                                        /\ useq[p][k] > sv.sq[k]
        report == r.err # saveErr[p]
    IN
    IF ~cur
    THEN \* stale tag: dropped by the saving.take_if guard
         /\ UNCHANGED <<unsaved, useq, saving, saveRetry, saveErr, confirmed, b4ext, silent>>
         /\ b4viol' = (b4viol \/ unsaved' # unsaved)
         /\ done' = done
    ELSE /\ saving' = [saving EXCEPT ![p] = NoJob]
         /\ b4viol' = b4viol
         /\ IF r.ok
            THEN /\ unsaved' = [unsaved EXCEPT ![p] = kept]
                 /\ useq' = [useq EXCEPT ![p] = [k \in Keys |-> IF kept[k] = NoV THEN 0 ELSE useq[p][k]]]
                 /\ confirmed' = [confirmed EXCEPT ![p] =
                        [k \in Keys |-> IF sv.ch[k] # NoV
                                        THEN [v |-> sv.ch[k], seq |-> sv.sq[k], at |-> r.at]
                                        ELSE confirmed[p][k]]]
                 /\ saveErr' = [saveErr EXCEPT ![p] = "none"]
                 /\ b4ext' = (b4ext \/ removedNewer)
                 \* a removed newer edit counts as saved by its equal value
                 /\ done' = [done EXCEPT ![p] = @ \cup {useq[p][k] : k \in {kk \in Keys :
                                    unsaved[p][kk] # NoV /\ kept[kk] = NoV}}]
                 /\ UNCHANGED <<saveRetry, silent>>
            ELSE /\ saveRetry' = [saveRetry EXCEPT ![p] = TRUE]
                 /\ saveErr' = [saveErr EXCEPT ![p] = r.err]
                 /\ done' = IF report THEN [done EXCEPT ![p] = @ \cup SeqsIn(p)] ELSE done
                 /\ silent' = (silent \/ (~report /\ SeqsIn(p) \ done[p] # {}))
                 /\ UNCHANGED <<unsaved, useq, confirmed, b4ext>>

\* IO::poll then App::complete at the top of event_loop. queue_save runs after every
\* input batch, so dirty is always clear when the loop polls again.
Poll(p) ==
    /\ st[p] = "run" /\ res[p].id # 0 /\ ~dirty[p]
    /\ res' = [res EXCEPT ![p] = NoRes] /\ pend' = [pend EXCEPT ![p] = 0]
    /\ Complete(p, res[p])
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, lock, st, lastEdit, nEdits,
                   dirty, lastTok, slot, run, nextId, stop, fails, broken, qs, exitErr>>

-----------------------------------------------------------------------------
(* IO worker of process p (io.rs run and execute) and atomic::update *)

\* Delivers the worker's result. IO::poll drops anything but the newest id.
Deliver(p, j, ok, err, at) ==
    /\ run' = [run EXCEPT ![p] = NoJob]
    /\ res' = IF ~j.detached /\ j.id = pend[p]
              THEN [res EXCEPT ![p] = [id |-> j.id, tok |-> j.tok, ok |-> ok, err |-> err, at |-> at]]
              ELSE res

\* The worker takes the queued job out of its slot (run)
WTake(p) ==
    /\ Alive(p) /\ run[p].id = 0 /\ slot[p].id # 0
    /\ run' = [run EXCEPT ![p] = slot[p]]
    /\ slot' = [slot EXCEPT ![p] = NoJob]
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, lock, st, unsaved, useq,
                   lastEdit, nEdits, dirty, saveRetry, saving, lastTok, pend, res, nextId, stop,
                   fails, broken, saveErr, qs, confirmed, done, b4viol, b4ext, exitErr, silent>>

\* After stop was set, a queued Update fails with Cause::Stopped instead of running
WStart(p) ==
    /\ Alive(p) /\ run[p].stage = "start"
    /\ IF stop[p]
       THEN Deliver(p, run[p], FALSE, "stopped", 0)           \* Cause::Stopped
       ELSE /\ run' = [run EXCEPT ![p].stage = "lock"] /\ UNCHANGED res
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, lock, st, unsaved, useq,
                   lastEdit, nEdits, dirty, saveRetry, saving, lastTok, slot, pend, nextId, stop,
                   fails, broken, saveErr, qs, confirmed, done, b4viol, b4ext, exitErr, silent>>

\* try_lock on the sibling .lock file succeeded
WLock(p) ==
    /\ Alive(p) /\ run[p].stage = "lock" /\ lock = <<>>
    /\ lock' = <<p, "w">>
    /\ run' = [run EXCEPT ![p].stage = "read"]
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, st, unsaved, useq,
                   lastEdit, nEdits, dirty, saveRetry, saving, lastTok, slot, pend, res, nextId,
                   stop, fails, broken, saveErr, qs, confirmed, done, b4viol, b4ext, exitErr, silent>>

\* SAVE_WAIT (store.rs) passed with the lock held elsewhere: "config is busy"
WBusy(p) ==
    /\ Alive(p) /\ run[p].stage = "lock" /\ lock # <<>> /\ CanFail(p)
    /\ fails' = [fails EXCEPT ![p] = @ + 1]
    /\ Deliver(p, run[p], FALSE, "busy", 0)
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, lock, st, unsaved, useq,
                   lastEdit, nEdits, dirty, saveRetry, saving, lastTok, slot, pend, nextId, stop,
                   broken, saveErr, qs, confirmed, done, b4viol, b4ext, exitErr, silent>>

\* read_regular, merge, write temp, sync, rename (atomic::update). Reading and
\* writing are one step: the lock excludes every other writer in between.
WWrite(p) ==
    /\ Alive(p) /\ run[p].stage = "read" /\ ~broken[p]
    /\ Commit(p, run[p].ch, run[p].sq)
    /\ lock' = <<>>
    /\ Deliver(p, run[p], TRUE, "none", commits + 1)
    /\ UNCHANGED <<st, unsaved, useq, lastEdit, nEdits, dirty, saveRetry, saving, lastTok,
                   slot, pend, nextId, stop, fails, broken, saveErr, qs, confirmed, b4viol, b4ext, exitErr, silent>>

\* temp write, sync or rename failed: nothing replaced
WFail(p) ==
    /\ Alive(p) /\ run[p].stage = "read" /\ (CanFail(p) \/ broken[p])
    /\ fails' = [fails EXCEPT ![p] = IF broken[p] THEN @ ELSE @ + 1]
    /\ broken' = [broken EXCEPT ![p] = PermanentFail]
    /\ lock' = <<>>
    /\ Deliver(p, run[p], FALSE, "io", 0)
    /\ NoCommit
    /\ UNCHANGED <<st, unsaved, useq, lastEdit, nEdits, dirty, saveRetry, saving, lastTok,
                   slot, pend, nextId, stop, saveErr, qs, confirmed, done, b4viol, b4ext, exitErr, silent>>

-----------------------------------------------------------------------------
(* Shutdown in main: IO::finish, App::complete for its results, save_on_quit *)

\* Worker idle and slots empty: the worker thread exits, the newest result is applied.
FinishDone(p) ==
    /\ st[p] = "finish" /\ run[p].id = 0 /\ slot[p].id = 0
    /\ st' = [st EXCEPT ![p] = "quitsave"]
    /\ res' = [res EXCEPT ![p] = NoRes] /\ pend' = [pend EXCEPT ![p] = 0]
    /\ IF res[p].id # 0 THEN Complete(p, res[p])
       ELSE UNCHANGED <<unsaved, useq, saving, saveRetry, saveErr, confirmed, done, b4viol, b4ext, silent>>
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, lock, lastEdit, nEdits, dirty,
                   lastTok, slot, run, nextId, stop, fails, broken, qs, exitErr>>

\* SHUTDOWN_WAIT passed with the worker inside a job: the pending Update is reported
\* TimedOut and the worker is detached, still running (IO::finish_timeout).
FinishTimeout(p) ==
    /\ AllowTimeout
    /\ st[p] = "finish" /\ run[p].id # 0 /\ res[p].id = 0
    /\ st' = [st EXCEPT ![p] = "quitsave"]
    /\ run' = [run EXCEPT ![p].detached = TRUE]
    /\ slot' = [slot EXCEPT ![p] = NoJob]
    /\ pend' = [pend EXCEPT ![p] = 0]
    /\ IF pend[p] # 0
       THEN Complete(p, [id |-> pend[p], tok |-> saving[p].tok, ok |-> FALSE,
                         err |-> "timedout", at |-> 0])
       ELSE UNCHANGED <<unsaved, useq, saving, saveRetry, saveErr, confirmed, done, b4viol, b4ext, silent>>
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, lock, lastEdit, nEdits, dirty,
                   lastTok, res, nextId, stop, fails, broken, qs, exitErr>>

\* save_on_quit starts, with nothing to do when App::unsaved is empty
QSStart(p) ==
    /\ st[p] = "quitsave" /\ qs[p].id = 0
    /\ IF Empty(unsaved[p])
       THEN /\ st' = [st EXCEPT ![p] = "exit_ok"] /\ UNCHANGED qs
       ELSE /\ qs' = [qs EXCEPT ![p] = [id |-> 1, tok |-> 0, ch |-> unsaved[p], sq |-> useq[p],
                                       stage |-> "lock", detached |-> FALSE]]
            /\ UNCHANGED st
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, lock, unsaved, useq, lastEdit,
                   nEdits, dirty, saveRetry, saving, lastTok, slot, run, pend, res, nextId, stop,
                   fails, broken, saveErr, confirmed, done, b4viol, b4ext, exitErr, silent>>

\* save_on_quit calls store::save, so atomic::update takes the same lock
QSLock(p) ==
    /\ st[p] = "quitsave" /\ qs[p].stage = "lock" /\ lock = <<>>
    /\ lock' = <<p, "m">> /\ qs' = [qs EXCEPT ![p].stage = "read"]
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, st, unsaved, useq, lastEdit,
                   nEdits, dirty, saveRetry, saving, lastTok, slot, run, pend, res, nextId, stop,
                   fails, broken, saveErr, confirmed, done, b4viol, b4ext, exitErr, silent>>

\* the one second wait for the lock expired, or the write failed
QSFail(p) ==
    /\ st[p] = "quitsave"
    /\ \/ qs[p].stage = "lock" /\ lock # <<>> /\ CanFail(p)
       \/ qs[p].stage = "read" /\ (CanFail(p) \/ broken[p])
    /\ fails' = [fails EXCEPT ![p] = IF broken[p] THEN @ ELSE @ + 1]
    /\ lock' = IF qs[p].stage = "read" THEN <<>> ELSE lock
    /\ st' = [st EXCEPT ![p] = "exit_err"]
    /\ qs' = [qs EXCEPT ![p] = NoJob]
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, unsaved, useq, lastEdit,
                   nEdits, dirty, saveRetry, saving, lastTok, slot, run, pend, res, nextId, stop,
                   broken, saveErr, confirmed, done, b4viol, b4ext, exitErr, silent>>

\* save_on_quit wrote and renamed: the process will exit without an error
QSWrite(p) ==
    /\ st[p] = "quitsave" /\ qs[p].stage = "read" /\ ~broken[p]
    /\ Commit(p, qs[p].ch, qs[p].sq)
    /\ lock' = <<>>
    /\ st' = [st EXCEPT ![p] = "exit_ok"]
    /\ qs' = [qs EXCEPT ![p] = NoJob]
    /\ confirmed' = [confirmed EXCEPT ![p] =
            [k \in Keys |-> IF qs[p].ch[k] # NoV
                            THEN [v |-> qs[p].ch[k], seq |-> qs[p].sq[k], at |-> commits + 1]
                            ELSE confirmed[p][k]]]
    /\ UNCHANGED <<unsaved, useq, lastEdit, nEdits, dirty, saveRetry, saving, lastTok, slot,
                   run, pend, res, nextId, stop, fails, broken, saveErr, b4viol, b4ext, exitErr, silent>>

\* main returns: the error (if any) is printed, the process exits, the OS drops
\* the detached worker and its lock.
Exit(p) ==
    /\ st[p] \in {"exit_ok", "exit_err"}
    /\ st' = [st EXCEPT ![p] = "exited"]
    /\ run' = [run EXCEPT ![p] = NoJob]
    /\ lock' = IF lock # <<>> /\ lock[1] = p THEN <<>> ELSE lock
    /\ done' = IF st[p] = "exit_err" THEN [done EXCEPT ![p] = @ \cup SeqsIn(p)] ELSE done
    /\ exitErr' = [exitErr EXCEPT ![p] = (st[p] = "exit_err")]
    /\ UNCHANGED <<file, fileSeq, commits, touchedAt, touchedBy, unsaved, useq, lastEdit,
                   nEdits, dirty, saveRetry, saving, lastTok, slot, pend, res, nextId, stop,
                   fails, broken, saveErr, qs, confirmed, b4viol, b4ext, silent>>

-----------------------------------------------------------------------------

\* User actions, system steps and injected faults of process p
User(p) == Quit(p) \/ \E k \in Keys, v \in Vals : Edit(p, k, v)
Sys(p) ==
    \/ QueueSave(p) \/ Poll(p) \/ WTake(p) \/ WStart(p) \/ WLock(p) \/ WWrite(p)
    \/ FinishDone(p) \/ QSStart(p) \/ QSLock(p) \/ QSWrite(p) \/ Exit(p)
Faults(p) == WBusy(p) \/ WFail(p) \/ FinishTimeout(p) \/ QSFail(p)

\* Any step of any process
Next == \E p \in Procs : User(p) \/ Sys(p) \/ Faults(p)

\* Fairness for B2: every enabled system step eventually happens, a free lock is
\* eventually taken, and a permanently failing write eventually reports its error.
Fair == \A p \in Procs :
          /\ WF_vars(QueueSave(p)) /\ WF_vars(Poll(p)) /\ WF_vars(WTake(p))
          /\ WF_vars(WStart(p)) /\ SF_vars(WLock(p)) /\ WF_vars(WWrite(p))
          /\ WF_vars(FinishDone(p)) /\ WF_vars(QSStart(p)) /\ SF_vars(QSLock(p))
          /\ WF_vars(QSWrite(p)) /\ WF_vars(Exit(p))
          \* a permanently failing write still finishes with an error
          /\ WF_vars(WFail(p) /\ broken[p]) /\ WF_vars(QSFail(p) /\ broken[p])

\* Spec for safety runs, FairSpec for B2
Spec == Init /\ [][Next]_vars
FairSpec == Spec /\ Fair

-----------------------------------------------------------------------------
(* Properties *)

\* B1a: a confirmed edit stays in the file unless a later rename wrote that key. This is
\*      the cross process lost update check.
B1a == \A p \in Procs, k \in Keys :
         confirmed[p][k].at # 0 =>
           (file[k] = confirmed[p][k].v \/ touchedAt[k] > confirmed[p][k].at)

\* B1b: the file never goes back to an edit of p older than one p already confirmed.
B1b == \A p \in Procs, k \in Keys :
         ~(fileSeq[k][1] = p /\ fileSeq[k][2] < confirmed[p][k].seq
           /\ file[k] # confirmed[p][k].v)

\* B1: both parts together
B1 == B1a /\ B1b

\* B2: every edit is eventually saved, superseded, or reported failed.
B2 == \A p \in Procs : \A s \in 1..MaxEdits :
        [](s <= nEdits[p] => <>(s \in done[p]))

\* B3: Library.saving and Library.save_retry are never set together.
B3 == \A p \in Procs : ~(saving[p].tok # 0 /\ saveRetry[p])

\* B4: a stale result never clears unsaved edits. B4ext: no accepted result removes
\*     an edit made after its job was queued.
B4 == ~b4viol
B4ext == ~b4ext

\* B5: a process that exited without an error left each of its newest edits in the
\*     file, unless another process renamed a value for that key after the edit.
B5 == \A p \in Procs :
        st[p] = "exited" /\ ~exitErr[p] =>
          \A k \in Keys : lastEdit[p][k].seq # 0 =>
            (file[k] = lastEdit[p][k].v
             \/ (touchedAt[k] > lastEdit[p][k].at /\ touchedBy[k] # p))

\* NoSilentFail (safety side of B2): no failed save covers an edit that no failure report covers.
NoSilentFail == ~silent

\* State bound on save tokens for runs with PermanentFail
TokBound == \A p \in Procs : lastTok[p] <= MaxTok



\* All safety properties together
Safety == B1a /\ B1b /\ B3 /\ B4 /\ B4ext /\ B5
=============================================================================
