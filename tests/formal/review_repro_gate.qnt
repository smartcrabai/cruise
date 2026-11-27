// pstack interrogate -> builtin:tdd verify-review loop (owner-approved revision 2: reproduce
// after consolidate; implemented by src/interrogate.rs and the reproduce steps in builtin/tdd.yaml).
// With flag `interrogate`, behavioral worklist items (REV/PLAN/WIRE) reach apply only when
// a separate reproduce step ran their command on the same snapshot and the output showed
// the failure; a host filter then drops the rest. SIMP/DOC keep the existing static bar.
// Off = current workflow (consolidate output goes straight to apply).
module review_repro_gate {
  type Diagram = { state: str, via: str }
  type State = {
    gate: bool, round: int, snap: int, reproSnap: int,
    findings: Set[int], repro: int -> str,
    worklist: Set[int], dropped: Set[int], blocked: Set[int],
    resolved: Set[int], persists: Set[int], wontFix: Set[int]
  }
  var s: State
  var diagram: Diagram

  // 1 and 2 are behavioral claims; 3 is a SIMP/DOC item.
  pure val behavioral = Set(1, 2)
  pure val ids = Set(1, 2, 3)
  pure val outcomes = Set("reproduced", "not-reproduced", "blocked")

  action go(state: str, via: str, after: State): bool = all {
    diagram' = { state: state, via: via }, s' = after,
  }
  action start(gate: bool): bool = all {
    s' = { gate: gate, round: 1, snap: 1, reproSnap: 0, findings: ids,
      repro: ids.mapBy(_ => "none"), worklist: Set(), dropped: Set(), blocked: Set(),
      resolved: Set(), persists: Set(), wontFix: Set() },
    diagram' = { state: "Analyzers reported", via: "read-only analyze" },
  }
  action init = start(true)

  // Existing consolidate: dedupe and the static evidence bar (every finding passes here).
  action consolidate = all {
    diagram.state == "Analyzers reported",
    go(if (s.gate) "Static worklist" else "Worklist",
      if (s.gate) "existing consolidate; not yet applicable" else "existing static bar",
      { ...s, worklist: s.findings, dropped: Set(), blocked: Set() }),
  }
  // Separate agent turn reruns each behavioral item's command. No edits.
  action reproduce(results: int -> str): bool = all {
    diagram.state == "Static worklist", s.gate,
    results.keys() == s.worklist.intersect(behavioral),
    results.keys().forall(i => outcomes.contains(results.get(i))),
    go("Reproduction recorded", "rerun each item's command on snapshot",
      { ...s, reproSnap: s.snap, repro: s.repro.keys().mapBy(i =>
          if (results.keys().contains(i)) results.get(i) else "none") }),
  }
  // Host fingerprint check: reproduce must leave the workspace unchanged.
  action reproduceWrote = all {
    diagram.state == "Static worklist", s.gate,
    go("Blocked: reproduce changed workspace", "host fingerprint mismatch", s),
  }
  // Host filter: keep reproduced behavioral items and every SIMP/DOC item.
  action hostFilter = all {
    diagram.state == "Reproduction recorded", s.reproSnap == s.snap,
    val beh = s.worklist.intersect(behavioral)
    go("Worklist", "host keeps reproduced; drops the rest with reason",
      { ...s, worklist: beh.filter(i => s.repro.get(i) == "reproduced").union(s.worklist.exclude(behavioral)),
        dropped: beh.filter(i => s.repro.get(i) == "not-reproduced"),
        blocked: beh.filter(i => s.repro.get(i) == "blocked") }),
  }
  // Workspace changed between review and apply (existing ledger staleness).
  action drift = all {
    Set("Reproduction recorded", "Worklist").contains(diagram.state),
    go("Review invalidated", "snapshot changed; rerun analyze", { ...s, snap: s.snap + 1 }),
  }
  // fixed = worklist items whose repro command passes after the edit.
  action apply(fixed: Set[int]): bool = all {
    diagram.state == "Worklist", fixed.subseteq(s.worklist),
    s.gate implies s.reproSnap == s.snap,
    // The latest round's disposition replaces an ID's prior ledger state.
    val seen = s.worklist.union(s.dropped).union(s.blocked)
    go("Ledger written", "apply; owed check includes each repro command",
      { ...s, snap: if (s.worklist == Set()) s.snap else s.snap + 1,
        resolved: s.resolved.exclude(seen).union(fixed),
        persists: s.persists.exclude(seen).union(s.worklist.exclude(fixed)).union(s.blocked),
        wontFix: s.wontFix.exclude(seen).union(s.dropped) }),
  }
  // Files changed -> existing group retry; findings and reproductions start fresh.
  action nextRound(reported: Set[int]): bool = all {
    diagram.state == "Ledger written", s.worklist != Set(), s.round < 3,
    reported.subseteq(ids),
    go("Analyzers reported", "files changed; analyze again",
      { ...s, round: s.round + 1, findings: reported, repro: ids.mapBy(_ => "none"),
        worklist: Set(), dropped: Set(), blocked: Set() }),
  }
  action converge = all {
    diagram.state == "Ledger written", s.worklist == Set(),
    go("Converged", "empty worklist", s),
  }

  action step = any {
    consolidate,
    nondet res = s.worklist.intersect(behavioral).setOfMaps(outcomes).oneOf()
    reproduce(res),
    reproduceWrote, hostFilter, drift, converge,
    nondet fixed = s.worklist.powerset().oneOf()
    apply(fixed),
    nondet again = ids.powerset().oneOf()
    nextRound(again),
  }

  val invGate = (s.gate and Set("Worklist", "Ledger written").contains(diagram.state)) implies
    s.worklist.intersect(behavioral).forall(i => s.repro.get(i) == "reproduced")
  val invDropped = s.worklist.intersect(s.dropped) == Set() and s.worklist.intersect(s.blocked) == Set()
  val invLedger = all {
    s.resolved.intersect(s.persists) == Set(), s.resolved.intersect(s.wontFix) == Set(),
    s.persists.intersect(s.wontFix) == Set(),
    diagram.state == "Ledger written" implies s.blocked.subseteq(s.persists),
  }
  val safety = invGate and invDropped and invLedger

  action reported = init.then(consolidate).then(reproduce(Map(1 -> "reproduced", 2 -> "not-reproduced")))
  run reproducedOnlyTest = reported.then(hostFilter)
    .expect(s.worklist == Set(1, 3) and s.dropped == Set(2) and safety)
  run notReproducedCannotBeAppliedTest = reported.then(hostFilter).then(apply(Set(2))).fail()
  run droppedBecomesWontFixTest = reported.then(hostFilter).then(apply(Set(1, 3)))
    .expect(s.wontFix == Set(2) and s.resolved == Set(1, 3) and safety)
  run blockedStaysOpenTest = init.then(consolidate).then(reproduce(Map(1 -> "blocked", 2 -> "reproduced")))
    .then(hostFilter).then(apply(Set(2, 3)))
    .expect(s.persists == Set(1) and not(s.wontFix.contains(1)) and safety)
  run failedFixPersistsTest = reported.then(hostFilter).then(apply(Set(3)))
    .expect(s.persists == Set(1) and not(s.resolved.contains(1)) and safety)
  run reproduceMustNotWriteTest = init.then(consolidate).then(reproduceWrote)
    .expect(diagram.state == "Blocked: reproduce changed workspace").then(hostFilter).fail()
  run cannotSkipReproductionTest = init.then(consolidate).then(apply(Set(1))).fail()
  run driftInvalidatesTest = reported.then(hostFilter).then(drift).then(apply(Set(1))).fail()
  run reproposedNeedsFreshRunTest = reported.then(hostFilter).then(apply(Set(1, 3)))
    .then(nextRound(Set(2))).then(consolidate).then(apply(Set(2))).fail()
  run gateOffKeepsCurrentFlowTest = start(false).then(consolidate).then(apply(Set(1, 2, 3)))
    .expect(s.resolved == ids and s.wontFix == Set() and safety)
  run emptyWorklistConvergesTest = reported.then(hostFilter).then(apply(Set(1, 3)))
    .then(nextRound(Set(2))).then(consolidate).then(reproduce(Map(2 -> "not-reproduced")))
    .then(hostFilter).then(apply(Set())).then(converge)
    .expect(diagram.state == "Converged" and s.wontFix == Set(2) and s.resolved == Set(1, 3) and safety)
}
