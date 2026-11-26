// pstack principle-attack-the-premise -> Cruise repair loops (owner-approved model;
// implemented by src/premise.rs and the premise steps in builtin/tdd.yaml and
// builtin/_shared/after-pr.yaml). A gate is one host-identified check: a CI check
// name in ci-followup/inventory.json, or one ledger finding ID that an apply round
// attempted and left `persists`. After two failed fixes
// against the same gate, the next fix starts only after a read-only premise step
// wrote the shared premise, ran a probe, and recorded a verdict. The premise step
// spends no fix budget and adds no extra fix attempts.
module premise_gate {
  type Diagram = { state: str, via: str }
  type State = {
    on: bool, failures: str -> int, premise: str -> str, fixes: int, budget: int,
    lastFailed: str, premiseInput: str
  }
  var s: State
  var diagram: Diagram

  pure val gates = Set("ci:test", "ci:lint")
  pure val verdicts = Set("held", "refuted", "no shared premise")
  pure val threshold = 2

  action go(state: str, via: str, after: State): bool = all {
    diagram' = { state: state, via: via }, s' = after,
  }
  action start(on: bool, budget: int): bool = all {
    s' = { on: on, failures: gates.mapBy(_ => 0), premise: gates.mapBy(_ => "none"),
      fixes: 0, budget: budget, lastFailed: "", premiseInput: "" },
    diagram' = { state: "Waiting for gates", via: "start" },
  }
  action init = start(true, 4)

  // Host observes the gate result for the current head.
  action observe(g: str, passed: bool): bool = all {
    Set("Waiting for gates", "Base resynced").contains(diagram.state), gates.contains(g),
    if (passed)
      go("Waiting for gates", "gate passed; reset its count",
        { ...s, failures: s.failures.set(g, 0), premise: s.premise.set(g, "none") })
    else
      go("Gate failed", "host records failing gate", { ...s, lastFailed: g }),
  }
  pure def needsPremise(st: State, g: str): bool =
    st.on and st.failures.get(g) >= threshold and st.premise.get(g) == "none"

  // Read-only: write the one sentence every failed fix assumed, probe it, give a verdict.
  action probePremise(verdict: str): bool = all {
    diagram.state == "Gate failed", needsPremise(s, s.lastFailed), verdicts.contains(verdict),
    go("Premise tested", "write premise; run probe; no edits",
      { ...s, premise: s.premise.set(s.lastFailed, verdict) }),
  }
  action probeWrote = all {
    diagram.state == "Gate failed", needsPremise(s, s.lastFailed),
    go("Blocked: premise step changed workspace", "host fingerprint mismatch", s),
  }
  action fix = all {
    Set("Gate failed", "Premise tested").contains(diagram.state),
    not(needsPremise(s, s.lastFailed)), s.fixes < s.budget,
    val g = s.lastFailed
    val tested = s.premise.get(g) != "none"
    go("Waiting for gates", if (tested) "fix with premise verdict as input" else "fix",
      { ...s, fixes: s.fixes + 1,
        premiseInput: if (tested) s.premise.get(g) else "",
        // A tested premise starts a new epoch: the next premise is owed after two more failures.
        failures: s.failures.set(g, if (tested) 1 else s.failures.get(g) + 1),
        premise: s.premise.set(g, "none") }),
  }
  action exhausted = all {
    Set("Gate failed", "Premise tested").contains(diagram.state), s.fixes >= s.budget,
    go("Loop protection", "existing edge budget exhausted", s),
  }
  // Base sync/resync restores edge budgets (existing behavior) but keeps premise counts.
  action resync = all {
    diagram.state == "Waiting for gates",
    go("Base resynced", "budgets reset; counts kept", { ...s, fixes: 0 }),
  }

  action step = any {
    nondet g = gates.oneOf()
    nondet passed = Set(true, false).oneOf()
    observe(g, passed),
    nondet v = verdicts.oneOf()
    probePremise(v),
    probeWrote, fix, exhausted, resync,
  }

  val invNoThirdBlindFix = s.on implies gates.forall(g => s.failures.get(g) <= threshold)
  val invBudget = s.fixes <= s.budget
  val invPremiseOnlyWhenOwed = gates.forall(g => s.premise.get(g) != "none" implies
    (s.on and s.failures.get(g) >= threshold))
  val safety = invNoThirdBlindFix and invBudget and invPremiseOnlyWhenOwed

  action failGate(g: str): bool = observe(g, false)
  action twoFailedFixes = init.then(failGate("ci:test")).then(fix).then(failGate("ci:test")).then(fix)
    .then(failGate("ci:test"))
  run thirdFixNeedsPremiseTest = twoFailedFixes.then(fix).fail()
  run premiseThenFixTest = twoFailedFixes.then(probePremise("refuted")).then(fix)
    .expect(s.premiseInput == "refuted" and s.fixes == 3 and s.failures.get("ci:test") == 1 and safety)
  run otherGateDoesNotCountTest = init.then(failGate("ci:test")).then(fix).then(failGate("ci:lint")).then(fix)
    .then(failGate("ci:test")).then(fix)
    .expect(s.fixes == 3 and s.premiseInput == "" and safety)
  run passResetsCountTest = init.then(failGate("ci:test")).then(fix).then(observe("ci:test", true))
    .then(failGate("ci:test")).then(fix).then(failGate("ci:test")).then(fix)
    .expect(s.fixes == 3 and safety)
  run premiseNotOwedEarlyTest = init.then(failGate("ci:test")).then(probePremise("held")).fail()
  run probeMustNotWriteTest = twoFailedFixes.then(probeWrote)
    .expect(diagram.state == "Blocked: premise step changed workspace").then(fix).fail()
  run premiseSpendsNoBudgetTest = start(true, 2).then(failGate("ci:test")).then(fix)
    .then(failGate("ci:test")).then(fix).then(failGate("ci:test")).then(probePremise("held"))
    .expect(s.fixes == 2).then(exhausted).expect(diagram.state == "Loop protection" and safety)
  run resyncKeepsCountTest = twoFailedFixes.then(probePremise("held")).then(fix).then(resync)
    .then(failGate("ci:test")).then(fix).then(failGate("ci:test"))
    .expect(needsPremise(s, "ci:test") and s.fixes == 1 and safety)
  run offKeepsCurrentLoopTest = start(false, 4).then(failGate("ci:test")).then(fix)
    .then(failGate("ci:test")).then(fix).then(failGate("ci:test")).then(fix)
    .expect(s.fixes == 3 and s.premiseInput == "")
}
