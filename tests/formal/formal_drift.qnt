// pstack maintain-formal-verification-skill -> Cruise formal_methods (owner-approved
// model; implemented by src/quint_workflow/maintain.rs and the formal-maintain stage in
// src/application/quint.rs). `.cruise/formal-coverage.toml` maps each repository Quint
// model to code paths and a sync commit. When a task changes covered
// code, a maintain step runs the check as is, classifies every change, edits only
// verification files, reruns, and judges drift vs bug. An updated model needs the
// #917 owner PNG approval before the sync commit moves.
module formal_drift {
  type Diagram = { state: str, via: str }
  type State = {
    covered: bool, head: int, sync: int, asIs: str, classes: Set[str],
    updated: bool, propertyChanged: bool, evidenceCited: bool, rerun: str,
    mutationCaught: bool, approved: bool, outcome: str, repairs: int, pr: bool
  }
  var s: State
  var diagram: Diagram

  pure val changeClasses = Set("no effect", "shape changed", "promise changed", "promise maybe broken")
  pure val maxRepairs = 2

  action go(state: str, via: str, after: State): bool = all {
    diagram' = { state: state, via: via }, s' = after,
  }
  action start(covered: bool): bool = all {
    s' = { covered: covered, head: 2, sync: 1, asIs: "none", classes: Set(), updated: false,
      propertyChanged: false, evidenceCited: false, rerun: "none", mutationCaught: false,
      approved: false, outcome: "none", repairs: 0, pr: false },
    diagram' = { state: "Task implemented", via: "start" },
  }
  action init = start(true)

  action notCovered = all {
    diagram.state == "Task implemented", not(s.covered),
    go("Ready for PR", "diff misses every coverage record", s),
  }
  action runAsIs(passed: bool): bool = all {
    diagram.state == "Task implemented", s.covered,
    go("Ran as is", "record result before any edit", { ...s, asIs: if (passed) "pass" else "fail" }),
  }
  action classify(classes: Set[str]): bool = all {
    diagram.state == "Ran as is", classes != Set(), classes.subseteq(changeClasses),
    go("Changes classified", "read every covered change since sync", { ...s, classes: classes }),
  }
  action inSync = all {
    diagram.state == "Changes classified", s.classes == Set("no effect"), s.asIs == "pass",
    go("Ready for PR", "in sync; move sync commit", { ...s, outcome: "in sync", sync: s.head }),
  }
  // Edit only models/harness/coverage record. A property changes only for an intended,
  // cited promise change; otherwise properties and bounds stay.
  action update(propertyChanged: bool, evidenceCited: bool): bool = all {
    diagram.state == "Changes classified",
    not(s.classes == Set("no effect") and s.asIs == "pass"),
    propertyChanged implies (s.classes.contains("promise changed") and evidenceCited),
    go("Verification updated", "edit verification files from the code",
      { ...s, updated: true, propertyChanged: propertyChanged, evidenceCited: evidenceCited }),
  }
  // Host: the maintain step changed a non-verification path.
  action productEdit = all {
    diagram.state == "Verification updated",
    go("Blocked: product code edited", "host path check", { ...s, outcome: "blocked" }),
  }
  action rerunCheck(passed: bool): bool = all {
    diagram.state == "Verification updated",
    go("Reran", "typecheck, test, seeded run", { ...s, rerun: if (passed) "pass" else "fail" }),
  }
  // Pass: break the guarded code once and require the check to catch it.
  action mutate(caught: bool): bool = all {
    diagram.state == "Reran", s.rerun == "pass",
    go(if (caught) "Awaiting model approval" else "Blocked: check cannot fail",
      if (caught) "scratch break caught; dropped" else "scratch break passed",
      { ...s, mutationCaught: caught, outcome: if (caught) s.outcome else "blocked" }),
  }
  // Fail: replay the counterexample against the code, step by step.
  action judge(everyStepPossible: bool): bool = all {
    diagram.state == "Reran", s.rerun == "fail",
    if (everyStepPossible)
      go("Stopped: code breaks property", "bug; report counterexample", { ...s, outcome: "bug" })
    else if (s.repairs < maxRepairs)
      go("Changes classified", "model drift; fix the verification", { ...s, repairs: s.repairs + 1 })
    else
      go("Blocked: drift unresolved", "repair limit", { ...s, outcome: "blocked" }),
  }
  // #917 approval of the regenerated PNG; reject stops like #917.
  action approve(ok: bool): bool = all {
    diagram.state == "Awaiting model approval",
    if (ok) go("Ready for PR", "owner approved; move sync commit",
      { ...s, approved: true, outcome: "updated", sync: s.head })
    else go("Stopped: model rejected", "owner rejected", { ...s, outcome: "blocked" }),
  }
  action openPr = all {
    diagram.state == "Ready for PR",
    go("PR created", "coverage record in same PR, separate commit", { ...s, pr: true }),
  }

  action step = any {
    notCovered, inSync, productEdit, openPr,
    nondet b1 = Set(true, false).oneOf()
    runAsIs(b1),
    nondet cs = changeClasses.powerset().oneOf()
    classify(cs),
    nondet b2 = Set(true, false).oneOf()
    nondet b3 = Set(true, false).oneOf()
    update(b2, b3),
    nondet b4 = Set(true, false).oneOf()
    rerunCheck(b4),
    nondet b5 = Set(true, false).oneOf()
    mutate(b5),
    nondet b6 = Set(true, false).oneOf()
    judge(b6),
    nondet b7 = Set(true, false).oneOf()
    approve(b7),
  }

  val invSync = s.sync == s.head implies Set("in sync", "updated").contains(s.outcome)
  val invUpdated = s.outcome == "updated" implies (s.approved and s.mutationCaught and s.rerun == "pass" and s.updated)
  val invProperty = s.propertyChanged implies (s.evidenceCited and s.classes.contains("promise changed"))
  val invPr = s.pr implies (Set("none", "in sync", "updated").contains(s.outcome) and (s.covered implies s.outcome != "none"))
  val invStalePass = s.outcome == "in sync" implies (s.classes == Set("no effect") and s.asIs == "pass")
  val safety = invSync and invUpdated and invProperty and invPr and invStalePass

  run notCoveredSkipsTest = start(false).then(notCovered).then(openPr)
    .expect(s.pr and s.sync == 1 and safety)
  run inSyncMovesSyncTest = init.then(runAsIs(true)).then(classify(Set("no effect"))).then(inSync).then(openPr)
    .expect(s.outcome == "in sync" and s.sync == 2 and safety)
  run stalePassMustUpdateTest = init.then(runAsIs(true)).then(classify(Set("shape changed"))).then(inSync).fail()
  run updatedNeedsApprovalTest = init.then(runAsIs(true)).then(classify(Set("shape changed")))
    .then(update(false, false)).then(rerunCheck(true)).then(mutate(true)).then(openPr).fail()
  run updatedApprovedTest = init.then(runAsIs(false)).then(classify(Set("shape changed")))
    .then(update(false, false)).then(rerunCheck(true)).then(mutate(true)).then(approve(true)).then(openPr)
    .expect(s.outcome == "updated" and s.sync == 2 and s.pr and safety)
  run uncitedPropertyChangeRejectedTest = init.then(runAsIs(false))
    .then(classify(Set("promise maybe broken"))).then(update(true, true)).fail()
  run citedPromiseChangeTest = init.then(runAsIs(false)).then(classify(Set("promise changed")))
    .then(update(true, true)).expect(s.propertyChanged and safety)
  run bugStopsWithoutPrTest = init.then(runAsIs(false)).then(classify(Set("promise maybe broken")))
    .then(update(false, false)).then(rerunCheck(false)).then(judge(true))
    .expect(s.outcome == "bug" and s.sync == 1 and safety).then(openPr).fail()
  run driftRepairLoopsTest = init.then(runAsIs(false)).then(classify(Set("shape changed")))
    .then(update(false, false)).then(rerunCheck(false)).then(judge(false))
    .expect(diagram.state == "Changes classified" and s.repairs == 1 and safety)
  run uncaughtMutationBlocksTest = init.then(runAsIs(true)).then(classify(Set("shape changed")))
    .then(update(false, false)).then(rerunCheck(true)).then(mutate(false))
    .expect(s.outcome == "blocked" and s.sync == 1 and safety).then(openPr).fail()
  run productEditBlocksTest = init.then(runAsIs(true)).then(classify(Set("shape changed")))
    .then(update(false, false)).then(productEdit).then(rerunCheck(true)).fail()
  run rejectStopsTest = init.then(runAsIs(true)).then(classify(Set("shape changed")))
    .then(update(false, false)).then(rerunCheck(true)).then(mutate(true)).then(approve(false))
    .expect(s.sync == 1 and safety).then(openPr).fail()
}
