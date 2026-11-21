// Bounded abstraction of Manager approval, dependency scheduling and publication.
// Task 1 depends on task 0. Atomic actions abstract durable writer/control locks.
// Does not model filesystem, Git, forge failures, retries, or liveness.
// Production: manager::approve, manager_coordinator::reserve_and_claim and
// manager_publication::reconcile_inner.
module manager {
  type State = { approved: bool, reserved: Set[int], running: Set[int],
    integrated: Set[int], verifying: bool, checksPassed: bool, published: bool,
    merged: bool, paused: bool }
  var s: State
  var previous: State
  var event: str
  pure val initial: State = { approved: false, reserved: Set(), running: Set(),
    integrated: Set(), verifying: false, checksPassed: false, published: false,
    merged: false, paused: false }
  action init = all { s' = initial, previous' = initial, event' = "init" }
  action record(n: State, e: str): bool = all { previous' = s, s' = n, event' = e }
  action approve = all { not(s.approved), record({ ...s, approved: true }, "approve") }
  action reserve(t: int): bool = all {
    s.approved, not(s.paused), Set(0, 1).contains(t), not(s.reserved.contains(t)),
    t == 0 or s.integrated.contains(0),
    record({ ...s, reserved: s.reserved.union(Set(t)) }, "reserve"),
  }
  action launch(t: int): bool = all {
    s.approved, not(s.paused), s.reserved.contains(t), not(s.running.contains(t)),
    not(s.integrated.contains(t)),
    record({ ...s, running: s.running.union(Set(t)) }, "launch"),
  }
  action integrate(t: int): bool = all {
    not(s.paused), s.running.contains(t),
    record({ ...s, running: s.running.exclude(Set(t)), integrated: s.integrated.union(Set(t)) }, "integrate"),
  }
  action verify = all {
    not(s.paused), s.integrated == Set(0, 1), not(s.verifying),
    record({ ...s, verifying: true }, "verify"),
  }
  action passChecks = all { not(s.paused), s.verifying, not(s.checksPassed),
    record({ ...s, checksPassed: true }, "checks") }
  action publish = all { not(s.paused), s.checksPassed, not(s.published),
    record({ ...s, published: true }, "publish") }
  action merge = all { not(s.paused), s.published, not(s.merged),
    record({ ...s, merged: true }, "merge") }
  action pause = all { s.approved, not(s.paused), not(s.merged), record({ ...s, paused: true }, "pause") }
  action resume = all { s.paused, record({ ...s, paused: false }, "resume") }
  action idle = record(s, "idle")
  action step = any { approve, reserve(0), reserve(1), launch(0), launch(1),
    integrate(0), integrate(1), verify, passChecks, publish, merge, pause, resume, idle }

  val authority = not(s.approved) implies s.reserved == Set() and s.running == Set()
    and s.integrated == Set() and not(s.verifying)
  val dependencies = s.reserved.contains(1) implies s.integrated.contains(0)
  val lifecycle = all {
    s.running.subseteq(s.reserved), s.integrated.subseteq(s.reserved),
    s.running.intersect(s.integrated) == Set(),
    s.verifying implies s.integrated == Set(0, 1),
    s.checksPassed implies s.verifying,
    s.published implies s.checksPassed,
    s.merged implies s.published,
  }
  val pauseSafety = previous.paused and event != "resume" implies s == previous
  val approvalDoesNotDispatch = event != "approve" or all {
    s.approved, s.reserved == Set(), s.running == Set(), s.integrated == Set(), not(s.verifying)
  }
  val safety = authority and dependencies and lifecycle and pauseSafety and approvalDoesNotDispatch

  run approvalAuthorizesAsynchronousSchedulingTest = init.then(approve).then(reserve(0)).then(launch(0))
    .expect(s.approved and s.running == Set(0) and safety)
  run dependencyAndPublicationTest = init.then(approve).then(reserve(0)).then(launch(0))
    .then(integrate(0)).then(reserve(1)).then(launch(1)).then(integrate(1))
    .then(verify).then(passChecks).then(publish).then(merge).expect(s.merged and safety)
  run pauseResumeTest = init.then(approve).then(reserve(0)).then(pause).then(idle)
    .then(resume).then(launch(0)).expect(s.running == Set(0) and safety)
}
