------------------------------ MODULE Notices ------------------------------
(***************************************************************************)
(* Status line notices of rthid: the notice slot of App (src/app.rs), the  *)
(* deferred notice, the Loading text and the IO completions that write     *)
(* them.                                                                   *)
(*                                                                         *)
(* Written against commit b3bbdc6. Notices have since become               *)
(* Notice { source, text } and config clears go through store::Library,    *)
(* but the slot and deferral behavior modelled here is unchanged.          *)
(*                                                                         *)
(* Abstracted: a message is a record [src, txt, id]. src names the         *)
(* reporter, txt stands for one of two texts per source, and id is fresh   *)
(* per report so the properties can tell a stale message from a current    *)
(* one. resolved[src] is the highest id whose cause is known to be         *)
(* resolved.                                                               *)
(*                                                                         *)
(* The event loop (event_loop in src/main.rs) alternates two phases:       *)
(*   - "io": any number of IO completions (the io.poll loop)               *)
(*   - EndIO: one draw, the only moment a message counts as seen           *)
(*   - "input": any number of user events (the event::read drain)          *)
(*   - EndInput: App::queue_save, then back to "io"                        *)
(* IO keeps only the newest job of each kind (IO::enqueue, IO::poll), so   *)
(* each kind has at most one pending result.                               *)
(***************************************************************************)
EXTENDS Naturals, Sequences

CONSTANTS MaxInput, MaxMsg,  \* bounds on user actions and reported messages
          BatchIO  \* TRUE: several completions may be applied before one draw

None    == [src |-> "none", txt |-> "none", id |-> 0]          \* empty slot
LoadMsg == [src |-> "load", txt |-> "Loading", id |-> 0]    \* text written by App::request_open
Srcs    == {"val", "exp", "cfgfail", "cfgwarn", "open", "list"}
CfgSrc  == {"cfgfail", "cfgwarn"}            \* Source::Config (a "config:" text prefix at b3bbdc6)
NoReq   == [tok |-> 0, dlg |-> 0]                             \* open_request is None

VARIABLES
    notice,     \* App.notice
    deferred,   \* App.deferred_notice, the one message held back while loading
    openReq,    \* App.open_request (tok = 0 means None)
    modal,      \* App.modal: "none", "open" (OpenFile dialog) or "edit" (preset or template editor)
    gen,        \* generation of the open dialog when modal = "open", echoed by Listed
    lastToken,  \* App.last_token, shared by dialog generations and Open tokens
    exportErr,  \* error text behind Export::status, "none" after a successful export
    saveError,  \* store::Library save_error (App.save_error at b3bbdc6), "none" when None
    saveRetry,  \* Library save_retry is Some
    dirty,      \* Library dirty: edits arrived since the last save job
    pOpen,      \* tag of the pending Open job in IO, 0 when none
    pList,      \* tag of the pending List job in IO, 0 when none
    pExport,    \* an Export job is pending in IO
    pUpdate,    \* an Update (config save) job is pending in IO
    resolved,   \* [Srcs -> Nat] causes resolved up to this message id (bookkeeping)
    msgId,      \* last message id handed out (bookkeeping)
    phase, inputs, \* event loop phase, and user actions so far (bounded by MaxInput)
    cfgUnseen,  \* a config message was reported and not yet drawn (A2 bookkeeping)
    duringLoad, \* newest message reported while loading (A4b bookkeeping)
    lostAtFinish, \* A4b history flag: such a message was lost when loading finished
    a3viol,     \* A3 history flag: a stale result changed a notice slot
    ioInFrame,  \* a completion was applied since the last draw
    step        \* label of the last action, shown in traces

vars == <<notice, deferred, openReq, modal, gen, lastToken, exportErr, saveError,
          saveRetry, dirty, pOpen, pList, pExport, pUpdate, resolved, msgId, phase,
          inputs, cfgUnseen, duringLoad, lostAtFinish, a3viol, ioInFrame, step>>

Loading   == openReq.tok # 0                          \* App::loading
DialogGen == IF modal = "open" THEN gen ELSE 0        \* App::dialog
\* m is a message whose cause was resolved after it was reported
Stale(m)  == m.src \in Srcs /\ m.id <= resolved[m.src]
\* a new message from source s with text t
NM(s, t)  == [src |-> s, txt |-> t, id |-> msgId + 1]
\* the clear behind the `!self.loading()` guard in App::handle_key, App::apply_step and App::set_listing
ClearIf(n) == IF Loading THEN n ELSE None

\* App::set_notice, which parks the message in deferred_notice while loading. Returns <<notice, deferred>>.
SN(ld, n, d, m) == IF ld THEN <<n, m>> ELSE <<m, d>>
\* duringLoad bookkeeping for a message reported while ld says whether loading
DL(ld, m) == IF ld THEN m ELSE duringLoad

Init ==
    /\ notice \in {None, [src |-> "cfgwarn", txt |-> "warn", id |-> 1]}  \* startup config warning, Loaded::notice given to App::new
    /\ deferred = None
    /\ openReq = NoReq
    /\ modal = "none" /\ gen = 0 /\ lastToken = 0
    /\ exportErr = "none" /\ saveError = "none"
    /\ saveRetry = FALSE /\ dirty = FALSE
    /\ pOpen = 0 /\ pList = 0 /\ pExport = FALSE /\ pUpdate = FALSE
    /\ resolved = [s \in Srcs |-> 0]
    /\ msgId = 1
    /\ phase = "io" /\ inputs = 0
    /\ cfgUnseen = (notice # None)
    /\ duringLoad = None /\ lostAtFinish = FALSE /\ a3viol = FALSE
    /\ ioInFrame = FALSE
    /\ step = "Init"

\* shorthand for the variables an action leaves alone
Unch(vs) == UNCHANGED vs

-----------------------------------------------------------------------------
(* User input, phase "input" *)

\* common guard of user actions in the "input" phase
Input(label) == phase = "input" /\ inputs < MaxInput /\ inputs' = inputs + 1 /\ step' = label
                /\ UNCHANGED ioInFrame

\* A global key that only clears the notice (zoom, pan, Tab) in App::handle_key
KeyOther ==
    /\ Input("KeyOther") /\ modal = "none"
    /\ notice' = ClearIf(notice)
    /\ Unch(<<deferred, openReq, modal, gen, lastToken, exportErr, saveError, saveRetry,
              dirty, pOpen, pList, pExport, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, a3viol>>)

\* h or l on a setting: App::handle_key then App::adjust, and App::refresh queues an Export job
SettingKey ==
    /\ Input("SettingKey") /\ modal = "none"
    /\ notice' = ClearIf(notice)
    /\ pExport' = TRUE
    /\ Unch(<<deferred, openReq, modal, gen, lastToken, exportErr, saveError, saveRetry,
              dirty, pOpen, pList, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, a3viol>>)

\* 'o' in App::handle_key: App::open_dialog then App::request_list
OpenDialog ==
    /\ Input("OpenDialog") /\ modal = "none"
    /\ notice' = ClearIf(notice)
    /\ lastToken' = lastToken + 1 /\ gen' = lastToken + 1 /\ modal' = "open"
    /\ pList' = lastToken + 1
    /\ Unch(<<deferred, openReq, exportErr, saveError, saveRetry, dirty, pOpen, pExport,
              pUpdate, resolved, msgId, phase, cfgUnseen, duringLoad, lostAtFinish, a3viol>>)

\* Esc in the open dialog: App::handle_key clears the notice and Step::close closes the dialog
DialogEsc ==
    /\ Input("DialogEsc") /\ modal = "open"
    /\ notice' = ClearIf(notice) /\ modal' = "none"
    /\ Unch(<<deferred, openReq, gen, lastToken, exportErr, saveError, saveRetry, dirty,
              pOpen, pList, pExport, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, a3viol>>)

\* Entering a directory in the dialog: Effect::ListDir in App::apply
DialogListDir ==
    /\ Input("DialogListDir") /\ modal = "open"
    /\ notice' = ClearIf(notice) /\ pList' = gen
    /\ Unch(<<deferred, openReq, modal, gen, lastToken, exportErr, saveError, saveRetry,
              dirty, pOpen, pExport, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, a3viol>>)

\* App::request_open with dialog generation d (0 means no dialog) writes Loading into the slot
RequestOpen(d) ==
    /\ lastToken' = lastToken + 1
    /\ openReq' = [tok |-> lastToken + 1, dlg |-> d]
    /\ notice' = LoadMsg
    /\ pOpen' = lastToken + 1

\* Picking a file in the dialog: Effect::Open through App::apply_step and App::apply
DialogPick ==
    /\ Input("DialogPick") /\ modal = "open"
    /\ RequestOpen(gen)
    /\ Unch(<<deferred, modal, gen, exportErr, saveError, saveRetry, dirty, pList, pExport,
              pUpdate, resolved, msgId, phase, cfgUnseen, duringLoad, lostAtFinish, a3viol>>)

\* A dropped or pasted path with no modal: App::handle_paste, which does not clear the notice
DropFile ==
    /\ Input("DropFile") /\ modal = "none"
    /\ RequestOpen(0)
    /\ Unch(<<deferred, modal, gen, exportErr, saveError, saveRetry, dirty, pList, pExport,
              pUpdate, resolved, msgId, phase, cfgUnseen, duringLoad, lostAtFinish, a3viol>>)

\* 'P' or 'T' in App::handle_key opens the preset or template editor
EditOpen ==
    /\ Input("EditOpen") /\ modal = "none"
    /\ notice' = ClearIf(notice) /\ modal' = "edit"
    /\ Unch(<<deferred, openReq, gen, lastToken, exportErr, saveError, saveRetry, dirty,
              pOpen, pList, pExport, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, a3viol>>)

\* An invalid name in the editor: Step::notice (src/modal.rs) reaches App::apply_step, which
\* clears the slot unless loading, then App::set_notice reports the validation error
EditInvalid ==
    /\ Input("EditInvalid") /\ modal = "edit"
    /\ LET m == NM("val", "invalid")
           r == SN(Loading, ClearIf(notice), deferred, m)
       IN /\ notice' = r[1] /\ deferred' = r[2]
          /\ msgId' = msgId + 1
          /\ duringLoad' = DL(Loading, m)
    /\ Unch(<<openReq, modal, gen, lastToken, exportErr, saveError, saveRetry, dirty,
              pOpen, pList, pExport, pUpdate, resolved, phase, cfgUnseen, lostAtFinish, a3viol>>)

\* A valid save: App::commit hands the edit to Library::commit, which sets dirty, and the
\* editor closes. This resolves the cause of any earlier validation error.
EditSave ==
    /\ Input("EditSave") /\ modal = "edit"
    /\ notice' = ClearIf(notice)
    /\ dirty' = TRUE /\ modal' = "none"
    /\ resolved' = [resolved EXCEPT !["val"] = msgId]
    /\ Unch(<<deferred, openReq, gen, lastToken, exportErr, saveError, saveRetry, pOpen,
              pList, pExport, pUpdate, msgId, phase, cfgUnseen, duringLoad, lostAtFinish, a3viol>>)

\* Esc closes the editor
EditEsc ==
    /\ Input("EditEsc") /\ modal = "edit"
    /\ notice' = ClearIf(notice) /\ modal' = "none"
    /\ Unch(<<deferred, openReq, gen, lastToken, exportErr, saveError, saveRetry, dirty,
              pOpen, pList, pExport, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, a3viol>>)

\* App::queue_save (through Library::save_job) after the input batch, at the end of an
\* event_loop iteration. `due` says whether the retry deadline has passed.
EndInput ==
    /\ phase = "input" /\ phase' = "io"
    /\ \E due \in BOOLEAN :
        IF dirty \/ (saveRetry /\ due)
        THEN /\ dirty' = FALSE /\ saveRetry' = FALSE /\ pUpdate' = TRUE
             /\ step' = "EndInput(queue_save)"
        ELSE /\ UNCHANGED <<dirty, saveRetry, pUpdate>> /\ step' = "EndInput"
    /\ inputs' = inputs /\ UNCHANGED ioInFrame
    /\ Unch(<<notice, deferred, openReq, modal, gen, lastToken, exportErr, saveError,
              pOpen, pList, pExport, resolved, msgId, cfgUnseen, duringLoad, lostAtFinish, a3viol>>)

\* The draw: ui::status shows the notice, else the export status
EndIO ==
    /\ phase = "io" /\ phase' = "input" /\ inputs' = inputs
    /\ cfgUnseen' = IF ~Loading /\ notice.src \in CfgSrc THEN FALSE ELSE cfgUnseen
    /\ step' = "Draw" /\ ioInFrame' = FALSE
    /\ Unch(<<notice, deferred, openReq, modal, gen, lastToken, exportErr, saveError,
              saveRetry, dirty, pOpen, pList, pExport, pUpdate, resolved, msgId,
              duringLoad, lostAtFinish, a3viol>>)

-----------------------------------------------------------------------------
(* IO completions, phase "io" (App::complete) *)

\* Common guard of IO completions. Without BatchIO each completion is drawn before the next.
IOStep(label) == /\ phase = "io" /\ step' = label /\ inputs' = inputs
                 /\ (BatchIO \/ ~ioInFrame) /\ ioInFrame' = TRUE

\* Opened(Ok(Image)) or Opened(Err) in App::complete
OpenDone(ok) ==
    /\ IOStep(IF ok THEN "Opened(Ok)" ELSE "Opened(Err)")
    /\ pOpen # 0 /\ pOpen' = 0
    /\ IF pOpen # openReq.tok
       THEN \* stale token: dropped by the open_request.take_if guard
            Unch(<<notice, deferred, openReq, modal, gen, lastToken, exportErr, saveError,
                   saveRetry, dirty, pList, pExport, pUpdate, resolved, msgId, phase,
                   cfgUnseen, duringLoad, lostAtFinish, a3viol>>)
       ELSE
         LET req == openReq
             m   == NM("open", "decode")
             n2  == IF ok THEN None ELSE m                 \* notice = None, then set_notice (no longer loading)
             n3  == IF n2 = None THEN deferred ELSE n2     \* deferred_notice only fills an empty slot
             res == IF ok THEN [resolved EXCEPT !["open"] = msgId] ELSE resolved
             dl  == duringLoad
         IN /\ openReq' = NoReq
            /\ notice' = n3 /\ deferred' = None
            /\ msgId' = IF ok THEN msgId ELSE msgId + 1
            /\ modal' = IF ok /\ req.dlg # 0 /\ req.dlg = DialogGen THEN "none" ELSE modal
            /\ pExport' = IF ok THEN TRUE ELSE pExport     \* App::open_image calls refresh
            /\ resolved' = res
            /\ duringLoad' = None
            /\ lostAtFinish' = (lostAtFinish \/
                   (dl # None /\ ~(dl.id <= res[dl.src]) /\ n3.id # dl.id
                    \* an export error stays visible through App::export_status
                    /\ ~(dl.src = "exp" /\ exportErr = dl.txt)))
            /\ Unch(<<gen, lastToken, exportErr, saveError, saveRetry, dirty, pList, pUpdate,
                      phase, cfgUnseen, a3viol>>)

\* Listed(Ok) or Listed(Err) in App::complete (Ok goes through App::set_listing)
ListDone(ok) ==
    /\ IOStep(IF ok THEN "Listed(Ok)" ELSE "Listed(Err)")
    /\ pList # 0 /\ pList' = 0
    /\ LET stale == pList # DialogGen
           m     == NM("list", "denied")
           r     == SN(Loading, notice, deferred, m)
           n1    == IF ok THEN (IF stale THEN notice ELSE ClearIf(notice)) ELSE r[1]
           d1    == IF ok THEN deferred ELSE r[2]
       IN /\ notice' = n1 /\ deferred' = d1
          /\ msgId' = IF ok THEN msgId ELSE msgId + 1
          /\ resolved' = IF ok /\ ~stale THEN [resolved EXCEPT !["list"] = msgId] ELSE resolved
          /\ duringLoad' = IF ok THEN duringLoad ELSE DL(Loading, m)
          /\ a3viol' = (a3viol \/ (stale /\ (n1 # notice \/ d1 # deferred)))
    /\ Unch(<<openReq, modal, gen, lastToken, exportErr, saveError, saveRetry, dirty, pOpen,
              pExport, pUpdate, phase, cfgUnseen, lostAtFinish>>)

\* Exported in App::complete. Success clears the notice only if its text equals export.status().
ExportDone(res) ==
    /\ IOStep(IF res = "ok" THEN "Exported(Ok)" ELSE "Exported(Err " \o res \o ")")
    /\ pExport /\ pExport' = FALSE
    /\ IF res = "ok"
       THEN /\ notice' = IF notice.src = "exp" /\ notice.txt = exportErr THEN None ELSE notice
            /\ exportErr' = "none"
            /\ resolved' = [resolved EXCEPT !["exp"] = msgId]
            /\ Unch(<<deferred, msgId, duringLoad>>)
       ELSE LET m == NM("exp", res)
                r == SN(Loading, notice, deferred, m)
            IN /\ exportErr' = res
               /\ notice' = r[1] /\ deferred' = r[2]
               /\ msgId' = msgId + 1
               /\ duringLoad' = DL(Loading, m)
               /\ UNCHANGED resolved
    /\ Unch(<<openReq, modal, gen, lastToken, saveError, saveRetry, dirty, pOpen, pList,
              pUpdate, phase, cfgUnseen, lostAtFinish, a3viol>>)

\* Updated: App::complete forwards it to Library::complete, which calls Library::saved or
\* Library::save_failed (App::saved and App::save_failed at b3bbdc6)
UpdateDone(res) ==
    /\ IOStep("Updated(" \o res \o ")")
    /\ pUpdate /\ pUpdate' = FALSE
    /\ CASE res = "ok" ->
              \* a save without warnings returns SaveNotice::ClearConfig, which clears a config notice
              LET clr(x) == IF x.src \in CfgSrc THEN None ELSE x
              IN /\ notice' = IF Loading THEN notice ELSE clr(notice)
                 /\ deferred' = IF Loading THEN clr(deferred) ELSE deferred
                 /\ saveError' = "none"
                 /\ resolved' = [resolved EXCEPT !["cfgfail"] = msgId, !["cfgwarn"] = msgId]
                 /\ cfgUnseen' = FALSE
                 /\ Unch(<<msgId, saveRetry, duringLoad>>)
         [] res = "okwarn" ->
              \* the re read file has skipped entries, so Loaded::notice is Some
              LET m == NM("cfgwarn", "warn")
                  r == SN(Loading, notice, deferred, m)
              IN /\ notice' = r[1] /\ deferred' = r[2]
                 /\ msgId' = msgId + 1
                 /\ saveError' = "none"
                 /\ resolved' = [resolved EXCEPT !["cfgfail"] = msgId]
                 /\ cfgUnseen' = TRUE
                 /\ duringLoad' = DL(Loading, m)
                 /\ UNCHANGED saveRetry
         [] OTHER ->
              \* a failure is reported only when its text differs from save_error
              /\ saveRetry' = TRUE
              /\ IF saveError # res
                 THEN LET m == NM("cfgfail", res)
                          r == SN(Loading, notice, deferred, m)
                      IN /\ notice' = r[1] /\ deferred' = r[2]
                         /\ msgId' = msgId + 1
                         /\ saveError' = res
                         /\ cfgUnseen' = TRUE
                         /\ duringLoad' = DL(Loading, m)
                 ELSE Unch(<<notice, deferred, msgId, saveError, cfgUnseen, duringLoad>>)
              /\ UNCHANGED resolved
    /\ Unch(<<openReq, modal, gen, lastToken, exportErr, dirty, pOpen, pList, pExport,
              phase, lostAtFinish, a3viol>>)

\* All interleavings of user input, draws and IO completions
Next ==
    \/ KeyOther \/ SettingKey \/ OpenDialog \/ DialogEsc \/ DialogListDir \/ DialogPick
    \/ DropFile \/ EditOpen \/ EditInvalid \/ EditSave \/ EditEsc \/ EndInput \/ EndIO
    \/ \E ok \in BOOLEAN : OpenDone(ok) \/ ListDone(ok)
    \/ \E r \in {"ok", "e1", "e2"} : ExportDone(r)
    \/ \E r \in {"ok", "okwarn", "busy", "denied"} : UpdateDone(r)

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Properties *)

\* A1: once loading is over, the status line never shows a message whose cause was resolved since.
A1 == ~Loading => ~Stale(notice)
\* A1 split by the source of the stale message, for one trace per cause
A1src(s) == ~Loading => ~(Stale(notice) /\ notice.src = s)
A1val == A1src("val")
A1exp == A1src("exp")
A1cfg == A1src("cfgfail") /\ A1src("cfgwarn")
A1list == A1src("list")
A1open == A1src("open")

\* A2: a reported config warning or save failure is drawn at least once, or still waits
\*     in a slot, unless a config success cleared it.
A2 == cfgUnseen => (notice.src \in CfgSrc \/ deferred.src \in CfgSrc)

\* A3: a Listed or Opened result for a dialog generation or token that is no longer
\*     current never changes the notice or the deferred notice.
A3 == ~a3viol

\* A4a: while loading, the status line reads Loading.
A4a == Loading => notice = LoadMsg
\* A4b: the newest unresolved message reported while loading is shown once loading finishes.
A4b == ~lostAtFinish

\* State bound on reported messages
Constr == msgId <= MaxMsg

\* Basic type check
TypeOK == phase \in {"io", "input"} /\ modal \in {"none", "open", "edit"}
=============================================================================
