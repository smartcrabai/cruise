// Bounded abstraction of reviewers: all. IDs 0..2 represent threads.
module pr_review_fix {
  type State = { unresolved: Set[int], prompted: Set[int], before: Set[int],
    iteration: int, status: str }
  var s: State
  var previous: State
  var eligible: Set[int]
  var event: str
  pure def initial(u: Set[int]): State = { unresolved: u, prompted: Set(),
    before: Set(), iteration: 0, status: "running" }
  action initWith(u: Set[int], e: Set[int]): bool = all {
    eligible' = e, s' = initial(u), previous' = initial(u), event' = "init" }
  action init = {
    nondet u = Set(0, 1, 2).powerset().oneOf()
    nondet e = Set(0, 1, 2).powerset().oneOf()
    initWith(u, e)
  }
  action record(updated: State, ev: str): bool = all {
    s' = updated, previous' = s, eligible' = eligible, event' = ev }
  action emptySuccess = all { s.status == "running", s.unresolved.intersect(eligible) == Set(),
    record({ ...s, status: "success" }, "empty") }
  // The environment may change the eligible unresolved set, including adding IDs.
  action iterate(after: Set[int]): bool = all {
    s.status == "running", s.unresolved.intersect(eligible) != Set(),
    s.iteration < 2, after.subseteq(eligible),
    record({ ...s, before: s.unresolved.intersect(eligible),
      prompted: s.unresolved.intersect(eligible),
      unresolved: s.unresolved.exclude(eligible).union(after),
      iteration: s.iteration + 1,
      status: if (after == Set()) "success"
        else if (after == s.unresolved.intersect(eligible)) "no_progress"
        else if (s.iteration + 1 == 2) "limit" else "running" }, "iterate") }
  action iteration = { nondet after = eligible.powerset().oneOf() iterate(after) }
  action idle = record(s, "idle")
  action step = any { emptySuccess, iteration, idle }
  val successImpliesNoEligibleUnresolved = s.status == "success" implies s.unresolved.intersect(eligible) == Set()
  val noPromptOnExcluded = s.prompted.subseteq(eligible)
  val iterationsBounded = s.iteration >= 0 and s.iteration <= 2
  val terminalAbsorbing = previous.status != "running" implies s == previous
  // This equivalence is scoped to completed iterations with a nonempty result:
  // success has priority over no progress, and no progress over the limit.
  val noProgressIffSameSet = event == "iterate" implies
    ((s.status == "no_progress") == (s.unresolved.intersect(eligible) != Set() and s.unresolved.intersect(eligible) == s.before))
  val safety = successImpliesNoEligibleUnresolved and noPromptOnExcluded and iterationsBounded and terminalAbsorbing and noProgressIffSameSet
  run successTest = initWith(Set(0, 2), Set(0, 1)).then(iterate(Set())).expect(s.status == "success" and s.unresolved == Set(2) and safety)
  run noProgressTest = initWith(Set(0), Set(0, 1)).then(iterate(Set(0))).expect(s.status == "no_progress" and safety).then(idle).expect(safety)
  run limitTest = initWith(Set(0), Set(0, 1)).then(iterate(Set(1))).then(iterate(Set(0))).expect(s.status == "limit" and safety)
}
