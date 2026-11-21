// One accepted exec in a finite three-node graph: prefix, body, tail.
// IDs and workflow/workspace metadata are immutable. An effect count models
// files/external effects only by observing execution, not by modeling rollback.
// Runtime and traversal counters stand in for the full saved prefix snapshot.
// Durable storage operations are atomic in this model, not a filesystem proof.
module exec_recovery {
  type Cursor = { node: int, runtime: int, edge: int, group: int }
  type Checkpoint = { cursor: Cursor, first: Cursor, unresolved: int, pending: bool }
  type Session = { phase: str, claim: bool, terminal: bool, saved: bool,
    valid: bool, id: int, cp: Checkpoint, prefixEffects: int, bodyEffects: int, tailEffects: int }
  var s: Session
  var prior: Session
  var op: str

  pure val entry: Cursor = { node: 0, runtime: 0, edge: 0, group: 0 }
  pure val absent: Cursor = { ...entry, node: -1 }
  pure val empty: Checkpoint = { cursor: entry, first: absent, unresolved: 0, pending: false }
  pure val initial: Session = { phase: "new", claim: false, terminal: false,
    saved: false, valid: true, id: 1, cp: empty, prefixEffects: 0, bodyEffects: 0, tailEffects: 0 }
  action init = all { s' = initial, prior' = initial, op' = "init" }
  action record(n: Session, tag: str): bool = all { s' = n, prior' = s, op' = tag }

  pure def rewind(i: Session): bool =
    i.cp.unresolved > 0 and (i.phase == "failed" or i.cp.cursor.node == 3)
  pure def retry(i: Session): Cursor =
    if (not(i.valid)) absent
    else if (rewind(i)) i.cp.first
    else if (i.phase == "failed" and i.cp.cursor.node == 3) absent
    else i.cp.cursor
  pure def listOptIn(i: Session): bool = i.phase == "failed" and i.saved and not(i.claim)
  // All states in this model are exec, so only the explicit CLI inventory can include one.
  pure def inventory(i: Session, optIn: bool): bool = optIn and listOptIn(i)
  pure def actions(i: Session): Set[str] = if (listOptIn(i)) Set("Resume", "Delete") else Set()
  pure def recoverable(i: Session): bool = listOptIn(i) and i.terminal
  pure def preserved(i: Session, j: Session): bool = i.id == j.id
    and i.prefixEffects == j.prefixEffects and i.bodyEffects == j.bodyEffects and i.tailEffects == j.tailEffects

  action submit = all { s.phase == "new", record({ ...s, phase: "setup", claim: true }, "submit") }
  action enter = all { s.phase == "setup", record({ ...s, phase: "running" }, "enter") }
  action runNode(outcome: str): bool = all {
    s.phase == "running", s.cp.cursor.node < 3, not(s.cp.pending),
    val c = s.cp.cursor
    val failed = outcome == "soft"
    // Save the pre-command prefix only for an unresolved failure. A handled
    // failure is counted as an effect but does not set this anchor.
    val cp = { ...s.cp, cursor: { ...c, runtime: if (outcome == "pass") 1 else 2 },
      pending: true,
      first: if (failed and s.cp.unresolved == 0) c else s.cp.first,
      unresolved: s.cp.unresolved + (if (failed) 1 else 0) }
    record({ ...s, cp: cp,
      prefixEffects: s.prefixEffects + (if (c.node == 0) 1 else 0),
      bodyEffects: s.bodyEffects + (if (c.node == 1) 1 else 0),
      tailEffects: s.tailEffects + (if (c.node == 2) 1 else 0) }, outcome),
  }
  action acceptEdge = all {
    s.phase == "running", s.cp.pending,
    val c = s.cp.cursor
    val advanced = { ...c, node: c.node + 1, edge: c.edge + 1,
      group: c.group + (if (c.node == 1) 1 else 0) }
    record({ ...s, cp: { ...s.cp, cursor: advanced, pending: false } }, "edge"),
  }
  action failAttempt(saved: bool): bool = all {
    s.phase == "setup" or (s.phase == "running"
      and (s.cp.cursor.node < 3 or s.cp.unresolved > 0)),
    record({ ...s, phase: "failed", saved: saved }, if (saved) "failSaved" else "failUnsaved"),
  }
  action finish = all {
    s.phase == "running", s.cp.cursor.node == 3, s.cp.unresolved == 0,
    record({ ...s, phase: "completed", saved: true }, "finish"),
  }
  action dispose = all {
    s.phase == "completed", s.saved, s.claim,
    record({ ...s, phase: "removed" }, "dispose"),
  }
  action release = all {
    s.claim and (s.phase == "failed" or s.phase == "removed"),
    record({ ...s, claim: false }, "release"),
  }
  action publish = all {
    not(s.claim), not(s.terminal), s.phase == "failed" or s.phase == "removed",
    record({ ...s, terminal: true }, "publish"),
  }
  action interrupt = all {
    s.phase == "running",
    record({ ...s, phase: "suspended", claim: false, saved: true }, "interrupt"),
  }
  action preview = all { not(s.claim), s.saved, record(s, "preview") }
  action clean = all { listOptIn(s), record(s, "clean") }
  action restart = all { not(s.claim), s.saved, record(s, "restart") }
  action reject = all {
    not(s.claim), s.phase == "failed" or s.phase == "suspended", record(s, "reject"),
  }
  action badCheckpoint = all {
    not(s.claim), s.saved, s.cp != empty,
    s.phase == "failed" or s.phase == "suspended",
    record({ ...s, valid: false }, "badCheckpoint"),
  }
  action resume = all {
    not(s.claim), s.saved, s.phase == "failed" or s.phase == "suspended", retry(s) != absent,
    val cp = if (rewind(s)) { ...empty, cursor: retry(s) } else s.cp
    record({ ...s, phase: "running", claim: true, terminal: false, cp: cp }, "resume"),
  }
  action delete = all { listOptIn(s), record({ ...s, phase: "removed" }, "delete") }
  action idle = record(s, "idle")
  action step = any { submit, enter, runNode("pass"), runNode("soft"), runNode("handled"),
    acceptEdge, failAttempt(true), failAttempt(false), finish, dispose, release, publish, interrupt,
    preview, clean, restart, reject, badCheckpoint, resume, delete, idle }

  val identity = s.id == prior.id and s.id == 1
  val noRollback = s.prefixEffects >= prior.prefixEffects and s.bodyEffects >= prior.bodyEffects
    and s.tailEffects >= prior.tailEffects
  val durableDisposal = (op == "dispose" implies prior.claim and prior.saved and prior.phase == "completed")
    and (s.phase == "removed" and prior.phase != "removed" implies op == "dispose" or op == "delete")
    and (op == "delete" implies listOptIn(prior) and preserved(prior, s))
  val explicitOnly = not(inventory(s, false))
    and (inventory(s, true) implies s.phase == "failed" and s.saved and not(s.claim))
    and (listOptIn(s) implies actions(s) == Set("Resume", "Delete"))
    and (recoverable(s) implies s.saved and not(s.claim))
    and (s.terminal implies not(s.claim))
  val durableAnchor = (s.cp.unresolved == 0 iff s.cp.first == absent)
    and (s.cp.unresolved > 0 implies s.cp.first.node <= s.cp.cursor.node)
    and (op == "soft" implies s.cp.unresolved == prior.cp.unresolved + 1
      and s.cp.first == (if (prior.cp.unresolved == 0) prior.cp.cursor else prior.cp.first))
    and (Set("pass", "handled", "edge", "failSaved", "interrupt").contains(op)
      implies s.cp.unresolved == prior.cp.unresolved and s.cp.first == prior.cp.first)
  val admission = (Set("preview", "clean", "restart", "reject").contains(op) implies s == prior)
    and (op == "resume" implies prior.valid and retry(prior) != absent and s.claim
      and s.cp.cursor == retry(prior) and preserved(prior, s))
    and (op == "resume" and rewind(prior) implies
      s.cp.first == absent and s.cp.unresolved == 0 and not(s.cp.pending))
    and (op == "resume" and not(rewind(prior)) implies s.cp == prior.cp)
  val safety = identity and noRollback and durableDisposal and explicitOnly and durableAnchor and admission

  run replayTest = init.then(submit).then(enter).then(runNode("pass")).then(acceptEdge)
    .then(runNode("soft")).then(acceptEdge).then(runNode("soft")).then(acceptEdge)
    .expect(s.cp.first == { node: 1, runtime: 1, edge: 1, group: 0 } and s.cp.unresolved == 2)
    .then(failAttempt(true)).then(release).then(publish).then(clean).then(restart).then(preview).then(reject)
    .expect(recoverable(s) and retry(s).node == 1 and s.tailEffects == 1)
    .then(resume).expect(s.cp.cursor == { node: 1, runtime: 1, edge: 1, group: 0 } and safety)
    .then(runNode("pass")).then(acceptEdge).then(runNode("pass")).then(acceptEdge)
    .then(finish).then(dispose).then(release).then(publish)
    .expect(s.phase == "removed" and s.prefixEffects == 1 and s.bodyEffects == 2 and s.tailEffects == 2 and safety)
  run failedAgainTest = init.then(submit).then(failAttempt(true)).then(release).then(publish)
    .expect(retry(s) == entry).then(resume).then(failAttempt(true)).then(release).then(publish)
    .expect(recoverable(s) and s.id == 1).then(delete).expect(s.phase == "removed" and safety)
  run hardPendingTest = init.then(submit).then(enter).then(runNode("pass"))
    .then(failAttempt(true)).then(release).then(resume).expect(s.cp.pending and safety)
    .then(acceptEdge).expect(s.prefixEffects == 1 and s.cp.cursor.node == 1 and safety)
  run livePauseTest = init.then(submit).then(enter).then(runNode("soft")).then(acceptEdge)
    .then(interrupt).then(resume).expect(s.cp.cursor.node == 1 and s.cp.unresolved == 1 and safety)
    .then(failAttempt(true)).then(release).then(resume).expect(s.cp.cursor == entry and s.cp.unresolved == 0 and safety)
  run interruptedCompletionTest = init.then(submit).then(enter).then(runNode("pass")).then(acceptEdge)
    .then(runNode("pass")).then(acceptEdge).then(runNode("soft")).then(acceptEdge)
    .then(interrupt).then(resume).expect(s.cp.cursor.node == 2 and s.tailEffects == 1 and safety)
  run handledTest = init.then(submit).then(enter).then(runNode("handled")).then(acceptEdge)
    .then(runNode("pass")).then(acceptEdge).then(runNode("pass")).then(acceptEdge)
    .then(finish).expect(s.cp.first == absent and safety).then(dispose).expect(safety)
  run invalidTest = init.then(submit).then(enter).then(runNode("soft")).then(failAttempt(true))
    .then(release).then(publish).then(badCheckpoint).then(reject).then(preview).then(clean)
    .expect(retry(s) == absent and recoverable(s) and s.prefixEffects == 1 and safety)
  run unsavedTest = init.then(submit).then(failAttempt(false)).then(release).then(publish)
    .expect(not(recoverable(s)) and not(inventory(s, true)) and safety)
}
