// Bounded abstraction of Manager approval, dependency scheduling, verification,
// corrective approval and publication. Task 1 depends on task 0; task 2 is the
// single corrective task. Atomic actions abstract durable writer/control locks.
// Does not model filesystem, Git, forge failures, child task retries, integrated
// tip changes, or liveness.
// Production: manager::approve, manager::preview_corrective,
// manager::approve_corrective, manager_coordinator::reserve_and_claim,
// manager_coordinator::retry_verification and manager_publication::reconcile_inner.
module manager {
  type State = { proposalRev: int, approved: bool, approvedRev: int, tasks: Set[int],
    reserved: Set[int], running: Set[int], integrated: Set[int], verifying: bool,
    checksPassed: bool, blocked: bool, preview: int, previews: int, published: bool,
    merged: bool, paused: bool }
  var s: State
  var previous: State
  var event: str
  pure val initial: State = { proposalRev: 0, approved: false, approvedRev: -1,
    tasks: Set(0, 1), reserved: Set(), running: Set(), integrated: Set(), verifying: false,
    checksPassed: false, blocked: false, preview: 0, previews: 0, published: false,
    merged: false, paused: false }
  action init = all { s' = initial, previous' = initial, event' = "init" }
  action record(n: State, e: str): bool = all { previous' = s, s' = n, event' = e }
  // A new preview replaces the reviewed one until a proposal is approved.
  action editProposal = all { not(s.approved), s.proposalRev < 2,
    record({ ...s, proposalRev: s.proposalRev + 1 }, "edit") }
  // Approval names the reviewed revision; a stale revision is not enabled.
  action approve(r: int): bool = all { not(s.approved), r == s.proposalRev,
    record({ ...s, approved: true, approvedRev: r }, "approve") }
  action reserve(t: int): bool = all {
    s.approved, not(s.paused), not(s.blocked), s.tasks.contains(t), not(s.reserved.contains(t)),
    t != 1 or s.integrated.contains(0),
    record({ ...s, reserved: s.reserved.union(Set(t)) }, "reserve"),
  }
  action launch(t: int): bool = all {
    s.approved, not(s.paused), not(s.blocked), s.reserved.contains(t), not(s.running.contains(t)),
    not(s.integrated.contains(t)),
    record({ ...s, running: s.running.union(Set(t)) }, "launch"),
  }
  action integrate(t: int): bool = all {
    not(s.paused), s.running.contains(t),
    record({ ...s, running: s.running.exclude(Set(t)), integrated: s.integrated.union(Set(t)) }, "integrate"),
  }
  action verify = all {
    not(s.paused), not(s.blocked), s.integrated == s.tasks, not(s.verifying),
    record({ ...s, verifying: true }, "verify"),
  }
  action passChecks = all { not(s.paused), s.verifying, not(s.checksPassed),
    record({ ...s, checksPassed: true }, "checks") }
  action failChecks = all { not(s.paused), s.verifying, not(s.checksPassed),
    record({ ...s, verifying: false, blocked: true }, "failChecks") }
  // `cruise manager retry GOAL`: fresh verification of a blocked, fully integrated Goal.
  action retryVerification = all { not(s.paused), s.blocked, s.integrated == s.tasks,
    record({ ...s, blocked: false, verifying: true }, "retryVerify") }
  // `cruise manager corrective`: saving a preview never authorizes the task.
  action previewCorrective = all { not(s.paused), s.blocked, not(s.tasks.contains(2)),
    s.previews < 2,
    record({ ...s, preview: s.previews + 1, previews: s.previews + 1 }, "preview") }
  // `cruise manager approve-corrective`: only the displayed preview revision.
  action approveCorrective(r: int): bool = all { not(s.paused), s.blocked, r > 0, s.preview == r,
    record({ ...s, tasks: s.tasks.union(Set(2)), preview: 0, blocked: false }, "approveCorrective") }
  action publish = all { not(s.paused), s.checksPassed, not(s.published),
    record({ ...s, published: true }, "publish") }
  action merge = all { not(s.paused), s.published, not(s.merged),
    record({ ...s, merged: true }, "merge") }
  action pause = all { s.approved, not(s.paused), not(s.merged), record({ ...s, paused: true }, "pause") }
  action resume = all { s.paused, record({ ...s, paused: false }, "resume") }
  action idle = record(s, "idle")
  action step = any { editProposal, approve(0), approve(1), approve(2), reserve(0), reserve(1),
    reserve(2), launch(0), launch(1), launch(2), integrate(0), integrate(1), integrate(2), verify,
    passChecks, failChecks, retryVerification, previewCorrective, approveCorrective(1),
    approveCorrective(2), publish, merge, pause, resume, idle }

  val authority = not(s.approved) implies s.reserved == Set() and s.running == Set()
    and s.integrated == Set() and not(s.verifying) and not(s.blocked)
  val revisionBound = s.approved implies s.approvedRev == s.proposalRev
  val dependencies = s.reserved.contains(1) implies s.integrated.contains(0)
  val lifecycle = all {
    s.reserved.subseteq(s.tasks),
    s.running.subseteq(s.reserved), s.integrated.subseteq(s.reserved),
    s.running.intersect(s.integrated) == Set(),
    s.verifying implies s.integrated == s.tasks,
    s.blocked implies not(s.verifying) and not(s.checksPassed),
    s.checksPassed implies s.verifying,
    s.published implies s.checksPassed,
    s.merged implies s.published,
  }
  val correctiveAuthority = all {
    // Like production, retrying verification keeps an unapproved preview; it
    // stays unusable until the Goal is blocked again.
    s.preview > 0 implies not(s.tasks.contains(2)),
    event == "approveCorrective" implies previous.blocked,
    event == "preview" implies s.tasks == previous.tasks and s.reserved == previous.reserved,
    event == "approveCorrective" implies previous.preview > 0 and s.tasks.contains(2)
      and not(previous.tasks.contains(2)),
    s.tasks.contains(2) implies s.previews > 0,
  }
  val blockedSafety = previous.blocked and s.blocked implies
    s.reserved == previous.reserved and s.running == previous.running
  val pauseSafety = previous.paused and event != "resume" implies s == previous
  val approvalDoesNotDispatch = event != "approve" or all {
    s.approved, s.reserved == Set(), s.running == Set(), s.integrated == Set(), not(s.verifying)
  }
  val safety = authority and revisionBound and dependencies and lifecycle and correctiveAuthority
    and blockedSafety and pauseSafety and approvalDoesNotDispatch

  run approvalAuthorizesAsynchronousSchedulingTest = init.then(approve(0)).then(reserve(0)).then(launch(0))
    .expect(s.approved and s.running == Set(0) and safety)
  run staleApprovalTest = init.then(editProposal).then(approve(0)).fail()
  run dependencyAndPublicationTest = init.then(approve(0)).then(reserve(0)).then(launch(0))
    .then(integrate(0)).then(reserve(1)).then(launch(1)).then(integrate(1))
    .then(verify).then(passChecks).then(publish).then(merge).expect(s.merged and safety)
  run pauseResumeTest = init.then(approve(0)).then(reserve(0)).then(pause).then(idle)
    .then(resume).then(launch(0)).expect(s.running == Set(0) and safety)
  run retryVerificationTest = init.then(approve(0)).then(reserve(0)).then(launch(0))
    .then(integrate(0)).then(reserve(1)).then(launch(1)).then(integrate(1)).then(verify)
    .then(failChecks).then(retryVerification).then(passChecks).then(publish)
    .expect(s.published and safety)
  run correctiveApprovalTest = init.then(approve(0)).then(reserve(0)).then(launch(0))
    .then(integrate(0)).then(reserve(1)).then(launch(1)).then(integrate(1)).then(verify)
    .then(failChecks).then(previewCorrective)
    .then(approveCorrective(1)).then(reserve(2)).then(launch(2)).then(integrate(2))
    .then(verify).then(passChecks).then(publish).expect(s.published and s.tasks.contains(2) and safety)
  run staleCorrectiveTest = init.then(approve(0)).then(reserve(0)).then(launch(0))
    .then(integrate(0)).then(reserve(1)).then(launch(1)).then(integrate(1)).then(verify)
    .then(failChecks).then(previewCorrective).then(previewCorrective).then(approveCorrective(1)).fail()
  run correctivePreviewDoesNotAuthorizeTest = init.then(approve(0)).then(reserve(0)).then(launch(0))
    .then(integrate(0)).then(reserve(1)).then(launch(1)).then(integrate(1)).then(verify)
    .then(failChecks).then(previewCorrective).then(reserve(2)).fail()
}
