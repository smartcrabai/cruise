// Bounded abstraction: intent and marker persistence are atomic. Matching
// abstracts the parent/tree comparison, not Git or concurrent mutation.
module local_merge {
  type State = { phase: str, dirty: bool, behind: bool, matched: bool,
    staged: bool, commits: int, wt: bool, branch: bool, session: bool,
    marker: bool, intent: bool, safeAtCommit: bool }
  var s: State
  var before: State
  var event: str
  pure def initial(d: bool, b: bool, m: bool): State = {
    phase: "Eligible", dirty: d, behind: b, matched: m, staged: false,
    commits: 0, wt: true, branch: true, session: true, marker: false, intent: false,
    safeAtCommit: false }
  action initWith(d: bool, b: bool, m: bool): bool = all {
    s' = initial(d,b,m), before' = initial(d,b,m), event' = "init" }
  action init = {
    nondet d = Set(true,false).oneOf()
    nondet b = Set(true,false).oneOf()
    nondet m = Set(true,false).oneOf()
    initWith(d,b,m)
  }
  action record(n: State, e: str): bool = all { before' = s, s' = n, event' = e }
  action refuse = all { s.phase == "Eligible", s.dirty or s.behind or not(s.matched),
    record({ ...s, phase: "Refused" }, "refuse") }
  action finalize = all { s.phase == "Eligible", not(s.dirty), not(s.behind), s.matched,
    record({ ...s, phase: "Finalized" }, "finalize") }
  action squash = all { s.phase == "Finalized", record({ ...s, phase: "Squashed", staged: true }, "squash") }
  // A conflicting squash is rolled back before returning an error.
  action restore = all { s.phase == "Squashed", record({ ...s, phase: "Restored", staged: false }, "conflict") }
  action stage = all { s.phase == "Squashed", record({ ...s, phase: "Staged" }, "stage") }
  action intent = all { s.phase == "Squashed", record({ ...s, phase: "Intent", intent: true }, "intent") }
  action intentFail = all { s.phase == "Squashed", record({ ...s, phase: "Restored", staged: false }, "intentFail") }
  action commit = all { s.phase == "Intent", s.intent, not(s.marker),
    record({ ...s, phase: "Committed", staged: false, commits: s.commits + 1,
      safeAtCommit: not(s.dirty) and not(s.behind) and s.matched }, "commit") }
  // Failure or interruption retains the intent and commit; retry can mark only a match.
  action interrupt = all { s.phase == "Committed", record(s, "interrupt") }
  action mismatch = all { s.phase == "Committed", record({ ...s, phase: "Mismatch" }, "mismatch") }
  action mark = all { s.phase == "Committed", record({ ...s, phase: "MergedMarked", marker: true }, "mark") }
  action cleanupFail = all { s.phase == "MergedMarked", record(s, "cleanupFail") }
  action cleanup = all { s.phase == "MergedMarked", s.marker,
    record({ ...s, phase: "Cleaned", wt: false, branch: false }, "cleanup") }
  action delete = all { s.phase == "Cleaned", record({ ...s, session: false }, "delete") }
  action idle = record(s, "idle")
  action step = any { refuse, finalize, squash, restore, stage, intent, intentFail, commit, interrupt, mismatch, mark, cleanupFail, cleanup, delete, idle }
  val safety = all {
    Set("Refused", "Restored").contains(s.phase) implies not(s.staged) and s.commits == 0,
    Set("Refused", "Restored", "Staged").contains(s.phase) implies s.session and s.wt and s.branch and not(s.marker),
    s.phase == "Staged" implies s.staged and s.commits == 0,
    s.marker implies s.commits == 1,
    s.commits > 0 implies s.intent,
    s.phase == "Mismatch" implies not(s.marker) and s.wt and s.branch,
    s.commits <= 1,
    (not(s.wt) or not(s.branch)) implies s.marker,
    not(s.session) implies s.marker and not(s.wt) and not(s.branch),
    (s.dirty or s.behind) implies s.commits == 0,
    s.commits > 0 implies s.safeAtCommit,
    event == "commit" implies not(before.dirty) and not(before.behind) and before.matched,
    event == "cleanupFail" implies s == before }
  run retryWitnessTest = initWith(false,false,true).then(finalize).then(squash).then(intent).then(commit)
    .then(interrupt).then(mark).then(cleanupFail).then(cleanup).then(delete)
    .expect(s.commits == 1 and s.marker and not(s.session) and safety)
  run stageWitnessTest = initWith(false,false,true).then(finalize).then(squash).then(stage)
    .expect(s.staged and s.commits == 0 and s.session and not(s.marker) and safety)
  run rollbackWitnessTest = initWith(false,false,true).then(finalize).then(squash).then(restore)
    .expect(not(s.staged) and s.commits == 0 and s.session and safety)
  run refuseWitnessTest = initWith(true,false,true).then(refuse).expect(s.phase == "Refused" and safety)
  run behindRefusalTest = initWith(false,true,true).then(refuse)
    .expect(s.phase == "Refused" and s.session and safety)
  run wrongBaseRefusalTest = initWith(false,false,false).then(refuse)
    .expect(s.phase == "Refused" and s.session and safety)
  run mismatchWitnessTest = initWith(false,false,true).then(finalize).then(squash).then(intent).then(commit).then(mismatch)
    .expect(s.phase == "Mismatch" and not(s.marker) and s.wt and safety)
  run intentFailureTest = initWith(false,false,true).then(finalize).then(squash).then(intentFail)
    .expect(s.commits == 0 and not(s.intent) and s.wt and safety)
}
