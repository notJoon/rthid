---------------------------- MODULE NoticesFix ----------------------------
(***************************************************************************)
(* Proposed notice handling for the status line of rthid, written against  *)
(* commit b3bbdc6 (src/app.rs). The proposal:                              *)
(*   - the notice is Option<Notice { source, text }> (the type has since   *)
(*     landed in src/app.rs)                                               *)
(*   - Loading is derived from App.open_request and never stored in the    *)
(*     slot                                                                *)
(*   - deferred_notice is removed, so set_notice always writes the slot    *)
(*   - App::request_open clears the notice first                           *)
(*   - among listings only Listed(Ok) of the current dialog clears the     *)
(*     notice                                                              *)
(*   - user input clears the notice even while loading, and the status     *)
(*     still reads Loading because it is derived                           *)
(*   - Exported(Ok) and a config success clear only a notice of their own  *)
(*     source                                                              *)
(* Abstractions and event loop phases are the same as in Notices.tla.      *)
(*                                                                         *)
(* Switches for variants of the proposal:                                  *)
(*   - InputClearsWhileLoading: TRUE as proposed                           *)
(*   - ListErrGenCheck: FALSE as proposed, so Listed(Err) of a stale       *)
(*     generation still sets the notice                                    *)
(*   - OpenErrKeepsNotice: FALSE as proposed, so Opened(Err) overwrites    *)
(*     the notice                                                          *)
(*   - CfgFallback: FALSE as proposed. TRUE adds a status fallback drawn   *)
(*     from save_error and the warnings of the last config load, like the  *)
(*     export status.                                                      *)
(***************************************************************************)
EXTENDS Naturals, Sequences

\* Bounds and BatchIO as in Notices.tla, then the variant switches
CONSTANTS MaxInput, MaxMsg, BatchIO,
          InputClearsWhileLoading, ListErrGenCheck, OpenErrKeepsNotice, CfgFallback

None    == [src |-> "none", txt |-> "none", id |-> 0]
LoadMsg == [src |-> "load", txt |-> "Loading", id |-> 0]
Srcs    == {"val", "exp", "cfgfail", "cfgwarn", "open", "list"}
CfgSrc  == {"cfgfail", "cfgwarn"}
NoReq   == [tok |-> 0, dlg |-> 0]

VARIABLES
    notice,     \* proposed App.notice: Option<Notice { source, text }>, never Loading
    openReq,    \* App.open_request (tok = 0 means None), from which the Loading text is derived
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
    phase,      \* event loop phase, "io" or "input"
    inputs,     \* user actions so far (bounded by MaxInput)
    cfgUnseen,  \* a config message was reported and not yet drawn (A2 bookkeeping)
    duringLoad, \* newest message reported while loading (A4b and A4p bookkeeping)
    lostAtFinish, \* A4b history flag: such a message was lost when loading finished
    lostPersist,  \* A4p history flag: a config message reported while loading was lost
    a3viol,     \* A3 history flag: a stale result changed the notice
    ioInFrame,  \* a completion was applied since the last draw
    cfgWarn,    \* the last config load had warnings (Loaded::notice is Some)
    step        \* label of the last action, shown in traces

vars == <<notice, openReq, modal, gen, lastToken, exportErr, saveError,
          saveRetry, dirty, pOpen, pList, pExport, pUpdate, resolved, msgId, phase,
          inputs, cfgUnseen, duringLoad, lostAtFinish, lostPersist, a3viol, ioInFrame, cfgWarn, step>>

Loading   == openReq.tok # 0                         \* App::loading
DialogGen == IF modal = "open" THEN gen ELSE 0       \* App::dialog
\* a persistent config condition, drawn when the notice slot is empty (CfgFallback only)
CfgHeld   == CfgFallback /\ (saveError # "none" \/ cfgWarn)
CfgShown  == [src |-> "cfgfail", txt |-> "held", id |-> 0]
Visible   == IF Loading THEN LoadMsg                  \* derived status text
             ELSE IF notice = None /\ CfgHeld THEN CfgShown ELSE notice
\* m is a message whose cause was resolved after it was reported
Stale(m)  == m.src \in Srcs /\ m.id <= resolved[m.src]
\* a new message from source s with text t
NM(s, t)  == [src |-> s, txt |-> t, id |-> msgId + 1]
\* the clear done by user input, skipped while loading unless InputClearsWhileLoading
ClearIn(n) == IF InputClearsWhileLoading \/ ~Loading THEN None ELSE n
\* duringLoad bookkeeping for a newly reported message
DL(m)     == IF Loading THEN m ELSE duringLoad

Init ==
    /\ notice \in {None, [src |-> "cfgwarn", txt |-> "warn", id |-> 1]}  \* startup config warning
    /\ openReq = NoReq
    /\ modal = "none" /\ gen = 0 /\ lastToken = 0
    /\ exportErr = "none" /\ saveError = "none"
    /\ saveRetry = FALSE /\ dirty = FALSE
    /\ pOpen = 0 /\ pList = 0 /\ pExport = FALSE /\ pUpdate = FALSE
    /\ resolved = [s \in Srcs |-> 0]
    /\ msgId = 1
    /\ phase = "io" /\ inputs = 0
    /\ cfgUnseen = (notice # None)
    /\ cfgWarn = (notice # None)
    /\ duringLoad = None /\ lostAtFinish = FALSE /\ lostPersist = FALSE /\ a3viol = FALSE
    /\ ioInFrame = FALSE
    /\ step = "Init"

\* shorthand for the variables an action leaves alone
Unch(vs) == UNCHANGED vs

\* common guard of user actions in the "input" phase
Input(label) == phase = "input" /\ inputs < MaxInput /\ inputs' = inputs + 1 /\ step' = label
                /\ UNCHANGED ioInFrame

\* Any global key (zoom, pan, Tab) in App::handle_key clears the notice, also while loading
KeyOther ==
    /\ Input("KeyOther") /\ modal = "none"
    /\ notice' = ClearIn(notice)
    /\ Unch(<<openReq, modal, gen, lastToken, exportErr, saveError, saveRetry,
              dirty, pOpen, pList, pExport, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* h or l on a setting: App::adjust, and App::refresh queues an Export job
SettingKey ==
    /\ Input("SettingKey") /\ modal = "none"
    /\ notice' = ClearIn(notice)
    /\ pExport' = TRUE
    /\ Unch(<<openReq, modal, gen, lastToken, exportErr, saveError, saveRetry,
              dirty, pOpen, pList, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* 'o': App::open_dialog then App::request_list tagged with the new generation
OpenDialog ==
    /\ Input("OpenDialog") /\ modal = "none"
    /\ notice' = ClearIn(notice)
    /\ lastToken' = lastToken + 1 /\ gen' = lastToken + 1 /\ modal' = "open"
    /\ pList' = lastToken + 1
    /\ Unch(<<openReq, exportErr, saveError, saveRetry, dirty, pOpen, pExport,
              pUpdate, resolved, msgId, phase, cfgUnseen, duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* Esc in the open dialog closes it
DialogEsc ==
    /\ Input("DialogEsc") /\ modal = "open"
    /\ notice' = ClearIn(notice) /\ modal' = "none"
    /\ Unch(<<openReq, gen, lastToken, exportErr, saveError, saveRetry, dirty,
              pOpen, pList, pExport, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* Entering a directory in the dialog: Effect::ListDir
DialogListDir ==
    /\ Input("DialogListDir") /\ modal = "open"
    /\ notice' = ClearIn(notice) /\ pList' = gen
    /\ Unch(<<openReq, modal, gen, lastToken, exportErr, saveError, saveRetry,
              dirty, pOpen, pExport, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* App::request_open with dialog generation d (0 means no dialog), clearing the notice first
RequestOpen(d) ==
    /\ lastToken' = lastToken + 1
    /\ openReq' = [tok |-> lastToken + 1, dlg |-> d]
    /\ notice' = None
    /\ pOpen' = lastToken + 1

\* Picking a file in the dialog: Effect::Open with the dialog generation
DialogPick ==
    /\ Input("DialogPick") /\ modal = "open"
    /\ RequestOpen(gen)
    /\ Unch(<<modal, gen, exportErr, saveError, saveRetry, dirty, pList, pExport,
              pUpdate, resolved, msgId, phase, cfgUnseen, duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* A dropped or pasted path with no modal: App::handle_paste
DropFile ==
    /\ Input("DropFile") /\ modal = "none"
    /\ RequestOpen(0)
    /\ Unch(<<modal, gen, exportErr, saveError, saveRetry, dirty, pList, pExport,
              pUpdate, resolved, msgId, phase, cfgUnseen, duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* 'P' or 'T' opens the preset or template editor
EditOpen ==
    /\ Input("EditOpen") /\ modal = "none"
    /\ notice' = ClearIn(notice) /\ modal' = "edit"
    /\ Unch(<<openReq, gen, lastToken, exportErr, saveError, saveRetry, dirty,
              pOpen, pList, pExport, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* An invalid name in the editor: Step::notice reports a validation error
EditInvalid ==
    /\ Input("EditInvalid") /\ modal = "edit"
    /\ LET m == NM("val", "invalid")
       IN /\ notice' = m
          /\ msgId' = msgId + 1
          /\ duringLoad' = DL(m)
    /\ Unch(<<openReq, modal, gen, lastToken, exportErr, saveError, saveRetry, dirty,
              pOpen, pList, pExport, pUpdate, resolved, phase, cfgUnseen, lostAtFinish, lostPersist, a3viol>>)

\* A valid save: App::commit hands the edit to Library::commit, which sets dirty. This
\* resolves the cause of earlier validation errors.
EditSave ==
    /\ Input("EditSave") /\ modal = "edit"
    /\ notice' = ClearIn(notice)
    /\ dirty' = TRUE /\ modal' = "none"
    /\ resolved' = [resolved EXCEPT !["val"] = msgId]
    /\ Unch(<<openReq, gen, lastToken, exportErr, saveError, saveRetry, pOpen,
              pList, pExport, pUpdate, msgId, phase, cfgUnseen, duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* Esc closes the editor
EditEsc ==
    /\ Input("EditEsc") /\ modal = "edit"
    /\ notice' = ClearIn(notice) /\ modal' = "none"
    /\ Unch(<<openReq, gen, lastToken, exportErr, saveError, saveRetry, dirty,
              pOpen, pList, pExport, pUpdate, resolved, msgId, phase, cfgUnseen,
              duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* App::queue_save (through Library::save_job) after the input batch. `due` says whether
\* the retry deadline has passed.
EndInput ==
    /\ phase = "input" /\ phase' = "io"
    /\ \E due \in BOOLEAN :
        IF dirty \/ (saveRetry /\ due)
        THEN /\ dirty' = FALSE /\ saveRetry' = FALSE /\ pUpdate' = TRUE
             /\ step' = "EndInput(queue_save)"
        ELSE /\ UNCHANGED <<dirty, saveRetry, pUpdate>> /\ step' = "EndInput"
    /\ inputs' = inputs /\ UNCHANGED ioInFrame
    /\ Unch(<<notice, openReq, modal, gen, lastToken, exportErr, saveError,
              pOpen, pList, pExport, resolved, msgId, cfgUnseen, duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* The draw: the status shows Loading while loading, else the notice or the fallback
EndIO ==
    /\ phase = "io" /\ phase' = "input" /\ inputs' = inputs
    /\ cfgUnseen' = IF Visible.src \in CfgSrc THEN FALSE ELSE cfgUnseen
    /\ step' = "Draw" /\ ioInFrame' = FALSE
    /\ Unch(<<notice, openReq, modal, gen, lastToken, exportErr, saveError,
              saveRetry, dirty, pOpen, pList, pExport, pUpdate, resolved, msgId,
              duringLoad, lostAtFinish, lostPersist, a3viol>>)

\* Common guard of IO completions. Without BatchIO each completion is drawn before the next.
IOStep(label) == /\ phase = "io" /\ step' = label /\ inputs' = inputs
                 /\ (BatchIO \/ ~ioInFrame) /\ ioInFrame' = TRUE

\* Opened(Ok(Image)) or Opened(Err). Success leaves the notice alone, it was cleared by request_open.
OpenDone(ok) ==
    /\ IOStep(IF ok THEN "Opened(Ok)" ELSE "Opened(Err)")
    /\ pOpen # 0 /\ pOpen' = 0
    /\ IF pOpen # openReq.tok
       THEN Unch(<<notice, openReq, modal, gen, lastToken, exportErr, saveError,
                   saveRetry, dirty, pList, pExport, pUpdate, resolved, msgId, phase,
                   cfgUnseen, duringLoad, lostAtFinish, lostPersist, a3viol>>)
       ELSE
         LET req == openReq
             m   == NM("open", "decode")
             n3  == IF ok \/ (OpenErrKeepsNotice /\ notice # None) THEN notice ELSE m
             res == IF ok THEN [resolved EXCEPT !["open"] = msgId] ELSE resolved
             dl  == duringLoad
         IN /\ openReq' = NoReq
            /\ notice' = n3
            /\ msgId' = IF ok THEN msgId ELSE msgId + 1
            /\ modal' = IF ok /\ req.dlg # 0 /\ req.dlg = DialogGen THEN "none" ELSE modal
            /\ pExport' = IF ok THEN TRUE ELSE pExport
            /\ resolved' = res
            /\ duringLoad' = None
            /\ lostAtFinish' = (lostAtFinish \/
                   (dl # None /\ ~(dl.id <= res[dl.src]) /\ n3.id # dl.id
                    /\ ~(dl.src = "exp" /\ exportErr = dl.txt)))
            /\ lostPersist' = (lostPersist \/
                   (dl.src \in CfgSrc /\ ~(dl.id <= res[dl.src]) /\ n3.id # dl.id /\ ~CfgHeld))
            /\ Unch(<<gen, lastToken, exportErr, saveError, saveRetry, dirty, pList, pUpdate,
                      phase, cfgUnseen, a3viol>>)

\* Listed(Ok) of the current dialog clears the notice. Listed(Err) sets it, unless
\* ListErrGenCheck drops a stale generation.
ListDone(ok) ==
    /\ IOStep(IF ok THEN "Listed(Ok)" ELSE "Listed(Err)")
    /\ pList # 0 /\ pList' = 0
    /\ LET stale == pList # DialogGen
           m     == NM("list", "denied")
           n1    == IF ok THEN (IF stale THEN notice ELSE None)
                    ELSE IF ListErrGenCheck /\ stale THEN notice ELSE m
           made  == ~ok /\ ~(ListErrGenCheck /\ stale)
       IN /\ notice' = n1
          /\ msgId' = IF made THEN msgId + 1 ELSE msgId
          /\ resolved' = IF ok /\ ~stale THEN [resolved EXCEPT !["list"] = msgId] ELSE resolved
          /\ duringLoad' = IF made THEN DL(m) ELSE duringLoad
          /\ a3viol' = (a3viol \/ (stale /\ n1 # notice))
    /\ Unch(<<openReq, modal, gen, lastToken, exportErr, saveError, saveRetry, dirty, pOpen,
              pExport, pUpdate, phase, cfgUnseen, lostAtFinish, lostPersist>>)

\* Exported(Ok) clears only a notice whose source is the export
ExportDone(res) ==
    /\ IOStep(IF res = "ok" THEN "Exported(Ok)" ELSE "Exported(Err " \o res \o ")")
    /\ pExport /\ pExport' = FALSE
    /\ IF res = "ok"
       THEN /\ notice' = IF notice.src = "exp" THEN None ELSE notice
            /\ exportErr' = "none"
            /\ resolved' = [resolved EXCEPT !["exp"] = msgId]
            /\ Unch(<<msgId, duringLoad>>)
       ELSE LET m == NM("exp", res)
            IN /\ exportErr' = res
               /\ notice' = m
               /\ msgId' = msgId + 1
               /\ duringLoad' = DL(m)
               /\ UNCHANGED resolved
    /\ Unch(<<openReq, modal, gen, lastToken, saveError, saveRetry, dirty, pOpen, pList,
              pUpdate, phase, cfgUnseen, lostAtFinish, lostPersist, a3viol>>)

\* Updated, through Library::complete: success clears only a config notice, and a failure
\* goes through Library::save_failed, which reports each text once
UpdateDone(res) ==
    /\ IOStep("Updated(" \o res \o ")")
    /\ pUpdate /\ pUpdate' = FALSE
    /\ CASE res = "ok" ->
              /\ notice' = IF notice.src \in CfgSrc THEN None ELSE notice
              /\ saveError' = "none"
              /\ resolved' = [resolved EXCEPT !["cfgfail"] = msgId, !["cfgwarn"] = msgId]
              /\ cfgUnseen' = FALSE
              /\ Unch(<<msgId, saveRetry, duringLoad>>)
         [] res = "okwarn" ->
              LET m == NM("cfgwarn", "warn")
              IN /\ notice' = m
                 /\ msgId' = msgId + 1
                 /\ saveError' = "none"
                 /\ resolved' = [resolved EXCEPT !["cfgfail"] = msgId]
                 /\ cfgUnseen' = TRUE
                 /\ duringLoad' = DL(m)
                 /\ UNCHANGED saveRetry
         [] OTHER ->
              /\ saveRetry' = TRUE
              /\ IF saveError # res
                 THEN LET m == NM("cfgfail", res)
                      IN /\ notice' = m
                         /\ msgId' = msgId + 1
                         /\ saveError' = res
                         /\ cfgUnseen' = TRUE
                         /\ duringLoad' = DL(m)
                 ELSE Unch(<<notice, msgId, saveError, cfgUnseen, duringLoad>>)
              /\ UNCHANGED resolved
    /\ Unch(<<openReq, modal, gen, lastToken, exportErr, dirty, pOpen, pList, pExport,
              phase, lostAtFinish, lostPersist, a3viol>>)

\* All interleavings of user input, draws and IO completions
Next ==
    \/ /\ \/ KeyOther \/ SettingKey \/ OpenDialog \/ DialogEsc \/ DialogListDir \/ DialogPick
          \/ DropFile \/ EditOpen \/ EditInvalid \/ EditSave \/ EditEsc \/ EndInput \/ EndIO
          \/ \E ok \in BOOLEAN : OpenDone(ok) \/ ListDone(ok)
          \/ \E r \in {"ok", "e1", "e2"} : ExportDone(r)
          \/ \E r \in {"busy", "denied"} : UpdateDone(r)
       /\ UNCHANGED cfgWarn
    \/ \E r \in {"ok", "okwarn"} : UpdateDone(r) /\ cfgWarn' = (r = "okwarn")

Spec == Init /\ [][Next]_vars

\* Properties, same meaning as in Notices.tla. A2 also accepts the config fallback.
A1 == ~Loading => ~Stale(notice)
A2 == cfgUnseen => (notice.src \in CfgSrc \/ CfgHeld)
A3 == ~a3viol
A4a == Loading => Visible = LoadMsg
A4b == ~lostAtFinish
\* A4p: A4b restricted to persistent conditions (config failures and warnings).
\* Export errors already stay visible through the export status.
A4p == ~lostPersist

\* State bound on reported messages
Constr == msgId <= MaxMsg
=============================================================================
