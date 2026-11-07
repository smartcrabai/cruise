// Bounded single-session model. The batch limit is fixed, while a persisted
// count can exceed a newly lowered limit. Manual runs never auto-retry.
module auto_requeue {
  type State = { phase: str, count: int, max: int, lastErr: str,
    event: str, previousPhase: str, previousCount: int, previousErr: str, batch: bool }
  var s: State

  pure def canAuto(x: State): bool = x.batch and x.phase == "Failed" and
    x.lastErr == "Eligible" and x.count < x.max
  pure def canStart(x: State): bool = Set("Planned", "Suspended").contains(x.phase)
  action initWith(phase: str, count: int, max: int): bool = {
    s' = { phase: phase, count: count, max: max, lastErr: "None",
      event: "init", previousPhase: phase, previousCount: count, previousErr: "None", batch: false }
  }
  action init = {
    nondet phase = Set("Planned", "Suspended").oneOf()
    nondet max = 0.to(2).oneOf()
    nondet count = 0.to(2).oneOf()
    initWith(phase, count, max)
  }
  action startAttempt = all {
    canStart(s),
    s' = { ...s, phase: "Running", lastErr: "None", event: "start", batch: true,
      previousPhase: s.phase, previousCount: s.count, previousErr: s.lastErr }
  }
  action finishAttempt(outcome: str): bool = all {
    s.phase == "Running", Set("Success", "Eligible", "Ineligible", "Cancelled").contains(outcome),
    s' = { ...s, phase: if (outcome == "Success") "Completed" else "Failed",
      lastErr: if (outcome == "Success") "None" else outcome, event: "finish",
      previousPhase: s.phase, previousCount: s.count, previousErr: s.lastErr }
  }
  action finish = {
    nondet outcome = Set("Success", "Eligible", "Ineligible", "Cancelled").oneOf()
    finishAttempt(outcome)
  }
  action autoRequeue = all {
    canAuto(s),
    s' = { ...s, phase: "Running", count: s.count + 1, lastErr: "None", event: "auto",
      previousPhase: s.phase, previousCount: s.count, previousErr: s.lastErr }
  }
  action manualRetry = all {
    s.phase == "Failed",
    s' = { ...s, phase: "Running", lastErr: "None", event: "manual", batch: false,
      previousPhase: s.phase, previousCount: s.count, previousErr: s.lastErr }
  }
  action resetToPlanned = all {
    Set("Failed", "Completed", "Suspended").contains(s.phase),
    s' = { ...s, phase: "Planned", count: 0, lastErr: "None", event: "reset", batch: false,
      previousPhase: s.phase, previousCount: s.count, previousErr: s.lastErr }
  }
  action idle = { s' = s }
  action step = any { startAttempt, finish, autoRequeue, manualRetry, resetToPlanned, idle }

  val budget = s.count >= 0 and (s.event == "auto" implies s.count <= s.max)
  val automaticEligibility = s.event == "auto" implies
    s.batch and s.previousPhase == "Failed" and s.previousErr == "Eligible" and
    s.previousCount < s.max and s.count == s.previousCount + 1 and s.phase == "Running"
  val noOtherIncrement = s.event != "auto" implies s.count <= s.previousCount
  val resetClears = s.event == "reset" implies s.count == 0 and s.phase == "Planned"
  val batchStart = s.event == "start" implies Set("Planned", "Suspended").contains(s.previousPhase)
  val manualPreserves = s.event == "manual" implies s.count == s.previousCount
  val safety = budget and automaticEligibility and noOtherIncrement and resetClears and batchStart and manualPreserves

  run retrySuccessTest = initWith("Planned", 0, 1).then(startAttempt)
    .then(finishAttempt("Eligible")).then(autoRequeue).then(finishAttempt("Success"))
    .expect(s.phase == "Completed" and s.count == 1 and safety)
  run exhaustedTest = initWith("Suspended", 0, 1).then(startAttempt)
    .then(finishAttempt("Eligible")).then(autoRequeue).then(finishAttempt("Eligible"))
    .expect(not(canAuto(s)) and s.count == 1 and safety)
  run disabledTest = initWith("Planned", 0, 0).then(startAttempt).then(finishAttempt("Eligible"))
    .expect(not(canAuto(s)) and s.count == 0 and safety)
  run ineligibleTest = initWith("Planned", 0, 2).then(startAttempt).then(finishAttempt("Ineligible"))
    .expect(not(canAuto(s)) and not(canStart(s)) and s.count == 0 and safety)
  run cancelledTest = initWith("Planned", 0, 2).then(startAttempt).then(finishAttempt("Cancelled"))
    .expect(not(canAuto(s)) and s.count == 0 and safety)
  run manualAndResetTest = initWith("Planned", 1, 2).then(startAttempt)
    .then(finishAttempt("Ineligible")).then(manualRetry).then(finishAttempt("Success"))
    .then(resetToPlanned).expect(s.count == 0 and s.phase == "Planned" and safety)
  run manualEligibleFailureTest = initWith("Planned", 0, 2).then(startAttempt)
    .then(finishAttempt("Eligible")).then(manualRetry).then(finishAttempt("Eligible"))
    .expect(not(canAuto(s)) and s.count == 0 and safety)
  run failedNotBatchCandidateTest = initWith("Failed", 0, 2)
    .expect(not(canStart(s)) and not(canAuto(s)) and safety)
  // A lowered future batch limit does not erase persisted history.
  run loweredLimitTest = initWith("Planned", 2, 1).then(startAttempt).then(finishAttempt("Eligible"))
    .expect(not(canAuto(s)) and s.count == 2 and safety)
}
