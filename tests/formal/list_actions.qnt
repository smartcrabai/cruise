// Contract of list_cmd and its delegation boundary, not a second daemon scheduler.
// IDs 1 and 2 stand for distinct rows with identical labels. ID 3 represents
// one new row. Eligibility is an observation, not a second daemon policy.
module list_actions {
  type State = { entry: str, phase: str, rows: Set[int], shown: Set[int],
    cursor: int, selected: int, mode: str, task: str, batch: bool,
    created: str, result: str, origin: str, initialRows: Set[int],
    cleanable: bool, runnable: bool, cleaned: bool, started: bool }
  var s: State

  action startWith(entry: str, rows: Set[int], batch: bool, cleanable: bool, runnable: bool): bool = s' = {
    entry: entry, phase: "fetch", rows: rows, shown: Set(), cursor: 0,
    selected: 0, mode: "none", task: "", batch: batch,
    created: "none", result: "none", origin: "none", initialRows: rows,
    cleanable: cleanable, runnable: runnable, cleaned: false, started: false }
  action start(entry: str, rows: Set[int], batch: bool): bool = startWith(entry, rows, batch, true, true)
  action init = {
    nondet entry = Set("tty", "headless", "json").oneOf()
    nondet rows = Set(1, 2).powerset().oneOf()
    nondet batch = Set(false, true).oneOf()
    nondet cleanable = Set(false, true).oneOf()
    nondet runnable = Set(false, true).oneOf()
    startWith(entry, rows, batch, cleanable, runnable)
  }
  val first = if (s.rows.contains(1)) 1 else if (s.rows.contains(2)) 2 else if (s.rows.contains(3)) 3 else 0
  action fetch = all {
    s.phase == "fetch",
    s' = { ...s, shown: s.rows, cursor: first,
      phase: if (s.entry == "json") "json-output"
        else if (s.entry == "headless" and s.rows == Set()) "empty-exit" else "picker" },
  }
  action failedFetch = all { s.phase == "fetch", s' = { ...s, phase: "fetch-error" } }
  action session(id: int): bool = all {
    s.phase == "picker", s.shown.contains(id),
    s' = { ...s, phase: "session", selected: id },
  }
  action handoff = all { s.phase == "session", s' = { ...s, phase: "exit" } }
  action quit = all { s.phase == "picker", s' = { ...s, phase: "exit" } }
  action global(destination: str): bool = all {
    s.phase == "picker", s.entry == "tty",
    s' = { ...s, phase: destination, selected: 0, result: "none", origin: "none" },
  }
  action chooseMode(mode: str): bool = all {
    s.phase == "new", s' = { ...s, phase: "task", mode: mode, task: "" },
  }
  action submit(text: str): bool = all {
    s.phase == "task", text != "", not(s.rows.contains(3)),
    s' = { ...s, task: text,
      phase: if (s.mode == "interactive") "review" else "fetch",
      rows: if (s.mode == "interactive") s.rows else s.rows.union(Set(3)),
      created: if (s.mode == "draft") "draft" else if (s.mode == "background") "planning" else "none",
      result: s.mode, origin: "submitted" },
  }
  action approve = all {
    s.phase == "review", s' = { ...s, phase: "fetch", rows: s.rows.union(Set(3)),
      created: "planned", result: "approved", origin: "review" },
  }
  // Back, Esc, Ctrl+C, and default No at nested prompts all re-fetch.
  action back = all {
    Set("session", "new", "task", "review", "clean-confirm", "start-confirm", "attach-confirm").contains(s.phase),
    s' = { ...s, phase: "fetch", selected: 0, result: "none", origin: "none" },
  }
  val hasWork = (s.rows.contains(1) and s.runnable) or s.created == "planned"
  action availability = all {
    s.phase == "availability",
    s' = { ...s, phase: if (s.batch) "attach-confirm" else if (hasWork) "start-confirm" else "fetch",
      result: if (not(s.batch) and not(hasWork)) "no-work" else "none" },
  }
  action confirm = all {
    Set("clean-confirm", "start-confirm", "attach-confirm").contains(s.phase),
    s' = { ...s, phase: "await", origin: s.phase },
  }
  action batchChanges(active: bool): bool = all {
    Set("start-confirm", "attach-confirm").contains(s.phase) or
      (s.phase == "await" and s.origin != "clean-confirm"),
    s.batch != active,
    s' = { ...s, batch: active },
  }
  val dispatchResult = if (s.origin == "clean-confirm") "clean"
    else if (s.batch) "attach" else if (s.origin == "attach-confirm") "noActiveBatch" else "start"
  action complete = all {
    s.phase == "await",
    s' = { ...s, phase: "fetch", result: dispatchResult,
      rows: if (dispatchResult == "clean" and s.cleanable) s.rows.exclude(Set(1)) else s.rows,
      batch: s.batch or dispatchResult == "start",
      cleaned: s.cleaned or (dispatchResult == "clean" and s.cleanable),
      started: s.started or dispatchResult == "start" },
  }
  action actionError = all { s.phase == "await" or s.phase == "availability",
    s' = { ...s, phase: "fetch", result: "action-error" } }
  action idle = all { Set("exit", "empty-exit", "json-output", "fetch-error").contains(s.phase) or
    (s.phase == "task" and s.rows.contains(3)), s' = s }
  action step = any {
    fetch, failedFetch, session(1), session(2), session(3), handoff, quit,
    global("new"), global("clean-confirm"), global("availability"),
    chooseMode("interactive"), chooseMode("background"), chooseMode("draft"),
    submit("stdin"), submit("ordinary task"), approve, back, availability,
    confirm, batchChanges(false), batchChanges(true), complete, actionError, idle,
  }

  val boundary = all {
    s.entry != "tty" implies not(Set("new", "task", "review", "clean-confirm", "availability", "start-confirm", "attach-confirm", "await").contains(s.phase)),
    s.entry == "json" implies Set("fetch", "fetch-error", "json-output").contains(s.phase),
    s.phase == "json-output" implies s.shown == s.rows,
    s.phase == "picker" implies s.shown == s.rows and s.cursor == first and (s.rows != Set() or s.entry == "tty"),
    s.phase == "session" implies s.shown.contains(s.selected),
  }
  val consent = all {
    s.result == "clean" implies s.origin == "clean-confirm",
    s.result == "start" implies s.origin == "start-confirm" and hasWork and s.started and s.batch,
    s.result == "attach" implies Set("start-confirm", "attach-confirm").contains(s.origin) and s.batch,
    s.result == "noActiveBatch" implies s.origin == "attach-confirm" and not(s.batch),
    s.phase == "start-confirm" implies hasWork,
    s.phase == "await" implies Set("clean-confirm", "start-confirm", "attach-confirm").contains(s.origin)
      and s.result == "none",
    s.rows.exclude(Set(3)) == (if (s.cleaned) s.initialRows.exclude(Set(1)) else s.initialRows),
    s.cleaned implies s.cleanable and s.entry == "tty",
    s.started implies s.entry == "tty",
  }
  val creation = all {
    s.rows.contains(3) iff s.created != "none",
    s.phase == "review" implies s.mode == "interactive" and s.task != "" and not(s.rows.contains(3)),
    s.result == "approved" implies s.created == "planned" and s.origin == "review",
    s.result == "draft" implies s.created == "draft" and s.origin == "submitted",
    s.result == "background" implies s.created == "planning" and s.task != "" and s.origin == "submitted",
  }
  val safety = boundary and consent and creation

  run emptyCancelTest = start("tty", Set(), false).then(fetch).then(global("new"))
    .then(chooseMode("interactive")).then(back).then(fetch)
    .expect(s.phase == "picker" and s.shown == Set() and s.cursor == 0 and safety)
  run stableIdentityTest = start("tty", Set(1, 2), false).then(fetch)
    .expect(s.cursor == 1).then(session(2)).expect(s.selected == 2 and safety)
  run cleanTest = start("tty", Set(1), false).then(fetch).then(global("clean-confirm"))
    .then(back).then(fetch).expect(s.shown == Set(1)).then(global("clean-confirm"))
    .then(confirm).expect(s.phase == "await" and s.rows == Set(1)).then(complete)
    .expect(s.shown == Set(1) and s.rows == Set()).then(fetch)
    .expect(s.phase == "picker" and s.shown == Set() and safety)
  run noWorkTest = start("tty", Set(2), false).then(fetch).then(global("availability"))
    .then(availability).expect(s.result == "no-work" and s.phase == "fetch" and safety)
  run daemonSkipsIneligibleCleanTest = startWith("tty", Set(1), false, false, true).then(fetch)
    .then(global("clean-confirm")).then(confirm).then(complete).then(fetch)
    .expect(s.shown == Set(1) and not(s.cleaned) and safety)
  run startBatchTest = start("tty", Set(1, 2), false).then(fetch).then(global("availability"))
    .then(availability).then(back).then(fetch).expect(not(s.batch)).then(global("availability"))
    .then(availability).then(confirm).then(complete).expect(s.batch and s.result == "start" and safety)
  run attachBatchTest = start("tty", Set(), true).then(fetch).then(global("availability"))
    .then(availability).then(confirm).then(complete).expect(s.result == "attach" and safety)
  run staleAttachTest = start("tty", Set(1), true).then(fetch).then(global("availability"))
    .then(availability).then(confirm).then(batchChanges(false)).expect(hasWork)
    .then(complete).then(fetch)
    .expect(s.phase == "picker" and s.result == "noActiveBatch" and not(s.started) and safety)
  run reverseRaceTest = start("tty", Set(1), false).then(fetch).then(global("availability"))
    .then(availability).then(confirm).then(batchChanges(true)).then(complete).then(fetch)
    .expect(s.phase == "picker" and s.result == "attach" and not(s.started) and safety)
  run backgroundTest = start("tty", Set(), false).then(fetch).then(global("new"))
    .then(chooseMode("background")).then(submit("stdin")).then(fetch)
    .expect(s.task == "stdin" and s.created == "planning" and s.shown == Set(3) and safety)
  run draftTest = start("tty", Set(), false).then(fetch).then(global("new"))
    .then(chooseMode("draft")).then(submit("ordinary task")).then(fetch)
    .expect(s.created == "draft" and s.shown == Set(3) and safety)
  run reviewTest = start("tty", Set(), false).then(fetch).then(global("new"))
    .then(chooseMode("interactive")).then(submit("ordinary task"))
    .expect(s.phase == "review" and not(s.batch)).then(approve).then(fetch)
    .expect(s.created == "planned" and s.shown == Set(3) and safety)
  run jsonTest = start("json", Set(), false).then(fetch)
    .expect(s.phase == "json-output" and s.shown == Set() and safety)
  run headlessTest = start("headless", Set(), false).then(fetch)
    .expect(s.phase == "empty-exit" and safety)
  run errorRedrawTest = start("tty", Set(1), false).then(fetch).then(global("clean-confirm"))
    .then(confirm).then(actionError).then(fetch)
    .expect(s.phase == "picker" and s.result == "action-error" and s.shown == Set(1) and safety)
  run fetchFailureTest = start("tty", Set(), false).then(failedFetch)
    .expect(s.phase == "fetch-error" and safety)
}
