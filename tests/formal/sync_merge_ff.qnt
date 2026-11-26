// Delta on after_pr_host_git: ordinary base merge policy, not review-head sync.
module sync_merge_ff {
  import after_pr_host_git as host from "./after_pr_host_git"
  type Diagram = { state: str, via: str }
  var diagram: Diagram
  var baseCurrent: bool
  var chosen: str
  pure val policies = Set("only", "no", "default")

  action init = all {
    host::init, baseCurrent' = true, chosen' = "none",
    diagram' = { state: "Unbound", via: "start" },
  }
  action go(state: str, via: str, current: bool, result: str): bool = all {
    diagram' = { state: state, via: via }, baseCurrent' = current, chosen' = result,
  }
  action bind = all {
    diagram.state == "Unbound", host::bindCurrent(0),
    go("PR head and base pinned", "authenticated binding", true, "none"),
  }
  pure def result(relation: str, config: str, explicitFf: bool): str =
    if (relation == "already") "noop"
    else if (relation == "linear") (if (not(explicitFf) and config == "no") "merge" else "ff")
    else if (not(explicitFf) and config == "only") "policy error"
    else if (relation == "conflicting") "conflict" else "merge"
  action admit(relation: str, config: str): bool = {
    val observed = result(relation, config, true)
    all {
      diagram.state == "PR head and base pinned", baseCurrent, policies.contains(config),
      Set("already", "linear", "diverged", "conflicting").contains(relation),
      if (observed == "conflict") all {
        host::observeConflict(1), go("Conflict recorded", "real conflict; no commit", true, observed),
      } else all {
        host::syncWorkspace,
        go(if (observed == "ff") "Local fast-forward" else if (observed == "noop") "Already contains base" else "Local two-parent merge",
          observed, true, observed),
      },
    }
  }
  action baseDrift = all {
    diagram.state == "PR head and base pinned", baseCurrent, host::hardError,
    go("Blocked", "base SHA changed", false, "none"),
  }
  action driftDuringMerge = all {
    Set("Local fast-forward", "Local two-parent merge").contains(diagram.state), host::hardError,
    go("Blocked", "fresh observation rejects local result", false, chosen),
  }
  action headDrift = all {
    diagram.state == "PR head and base pinned", host::advanceRemote(1, 1),
    go("Blocked", "PR head changed; rebind required", false, "none"),
  }
  val safety = all {
    host::safety, host::s.publications == 0, not(host::s.nativeMerged),
    diagram.state == "Conflict recorded" implies (host::s.mergeActive and host::s.conflictRecorded),
    diagram.state == "Local fast-forward" implies chosen == "ff",
    diagram.state == "Local two-parent merge" implies chosen == "merge",
    diagram.state == "Blocked" implies not(baseCurrent),
  }
  run configOnlyStillMergesDivergenceTest = init.then(bind).then(admit("diverged", "only"))
    .expect(chosen == "merge" and result("diverged", "only", false) == "policy error" and safety)
  run configNoStillFastForwardsTest = init.then(bind).then(admit("linear", "no"))
    .expect(chosen == "ff" and result("linear", "no", false) == "merge" and safety)
  run containingBaseDoesNotWriteTest = init.then(bind).then(admit("already", "default"))
    .expect(chosen == "noop" and safety)
  run realConflictStillStopsTest = init.then(bind).then(admit("conflicting", "only"))
    .expect(chosen == "conflict" and safety)
  run baseDriftBlocksTest = init.then(bind).then(baseDrift).expect(safety)
  run headDriftBlocksTest = init.then(bind).then(headDrift).expect(safety)
  run localMergeCannotAuthorizePublicationAfterDriftTest = init.then(bind)
    .then(admit("diverged", "only")).then(driftDuringMerge)
    .expect(diagram.state == "Blocked" and chosen == "merge" and safety)
  run cannotSyncStaleBindingTest = init.then(bind).then(baseDrift).then(admit("linear", "default")).fail()
}
