// One mandatory-invariant Quint block and one process-local session claim.
// Stage verdicts abstract process outcomes; multi-block aggregation is tested
// separately because a single block stops at its first non-passing stage.
// Optional temporal targets, cancellation and tool/setup errors are not modeled.
module plan_verify {
  // phase: 0 idle, 1 parse, 2 typecheck, 3 run, 4 verify, 5 done
  // verdict: 0 not yet run, 1 passed, 2 error, 3 failed
  var phase: int
  var results: int -> int
  var claim: int
  var plan: int
  action init = all { phase' = 0, results' = Map(1 -> 0, 2 -> 0, 3 -> 0, 4 -> 0), claim' = 0, plan' = 42 }
  action start = all { phase == 0, claim == 0, phase' = 1, claim' = 1, results' = results, plan' = plan }
  action complete(v: int): bool = all {
    1.to(4).contains(phase), Set(1, 2, 3).contains(v),
    results' = results.set(phase, v),
    phase' = if (v == 1 and phase < 4) phase + 1 else 5,
    claim' = if (v == 1 and phase < 4) claim else 0, plan' = plan
  }
  action finish = { nondet v = Set(1, 2, 3).oneOf() complete(v) }
  // Other planning operations compete for the same claim: Fix, Ask or Replan.
  action otherStart = { nondet op = Set(2, 3, 4).oneOf() all { phase == 0, claim == 0, claim' = op, phase' = phase, results' = results, plan' = plan } }
  action otherEnd = all { phase == 0, claim >= 2, claim' = 0, phase' = phase, results' = results, plan' = plan }
  action idle = all { phase' = phase, claim' = claim, results' = results, plan' = plan }
  action step = any { start, finish, otherStart, otherEnd, idle }
  val inv_stage_gating = all {
    phase >= 2 and phase <= 4 implies 1.to(phase - 1).forall(i => results.get(i) == 1),
    1.to(4).forall(i => results.get(i) != 0 implies 1.to(i - 1).forall(j => results.get(j) == 1))
  }
  pure def aggregate(rs: int -> int): int = if (1.to(4).exists(i => rs.get(i) == 3)) 3 else if (1.to(4).exists(i => rs.get(i) == 2)) 2 else 1
  val inv_verdict_precedence = all {
    1.to(4).exists(i => results.get(i) == 3) implies aggregate(results) == 3,
    not(1.to(4).exists(i => results.get(i) == 3)) and
      1.to(4).exists(i => results.get(i) == 2) implies aggregate(results) == 2,
    1.to(4).forall(i => results.get(i) != 2 and results.get(i) != 3) implies aggregate(results) == 1,
  }
  val inv_no_plan_mutation = plan == 42
  val inv_claim_exclusion = all { (phase >= 1 and phase <= 4) implies claim == 1, claim == 1 implies phase >= 1 and phase <= 4 }
  val safety = inv_stage_gating and inv_verdict_precedence and inv_no_plan_mutation and inv_claim_exclusion

  run successTest = init.then(start).then(complete(1)).then(complete(1))
    .then(complete(1)).then(complete(1))
    .expect(phase == 5 and claim == 0 and aggregate(results) == 1 and safety)
  run parseErrorTest = init.then(start).then(complete(2))
    .expect(phase == 5 and results.get(2) == 0 and results.get(4) == 0
      and aggregate(results) == 2 and safety)
  run typeErrorTest = init.then(start).then(complete(1)).then(complete(2))
    .expect(phase == 5 and results.get(3) == 0 and results.get(4) == 0
      and aggregate(results) == 2 and safety)
  run simulationFailureTest = init.then(start).then(complete(1)).then(complete(1))
    .then(complete(3)).expect(phase == 5 and results.get(4) == 0
      and aggregate(results) == 3 and safety)
  run verifyFailureTest = init.then(start).then(complete(1)).then(complete(1))
    .then(complete(1)).then(complete(3))
    .expect(phase == 5 and aggregate(results) == 3 and safety)
  // Different blocks may contain both errors and failures, unlike one block.
  run precedenceTest = init.expect(all {
    aggregate(Map(1 -> 2, 2 -> 3, 3 -> 1, 4 -> 0)) == 3,
    aggregate(Map(1 -> 1, 2 -> 2, 3 -> 0, 4 -> 0)) == 2,
    aggregate(Map(1 -> 1, 2 -> 1, 3 -> 0, 4 -> 0)) == 1,
  })
  run competingClaimTest = init.then(otherStart)
    .expect(claim >= 2 and phase == 0 and safety)
    .then(otherEnd).then(start).expect(claim == 1 and phase == 1 and safety)
}
