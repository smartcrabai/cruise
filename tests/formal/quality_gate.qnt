// Bounded abstraction of one gated prompt and its outgoing transition.
// A pending failure and feedback are checkpointed together. A crash between
// a shell effect and checkpoint is outside the model, not exactly-once.
module quality_gate {
  type State = { phase: int, count: int, limit: int, feedback: bool, effects: int }
  var s: State
  var previous: State
  var event: str
  pure val initial: State = { phase: 0, count: 0, limit: 2, feedback: false, effects: 0 }
  action init = all { s' = initial, previous' = initial, event' = "init" }
  action record(n: State, e: str): bool = all { previous' = s, s' = n, event' = e }
  // phase 0 prompt, 1 pending gate failure, 2 success, 3 exhausted.
  action pass = all { s.phase == 0, record({ ...s, phase: 2, feedback: false, effects: s.effects + 1 }, "pass") }
  action gateFail = all { s.phase == 0, record({ ...s, phase: 1, feedback: true, effects: s.effects + 1 }, "fail") }
  action retry = all { s.phase == 1, s.count < s.limit, record({ ...s, phase: 0, count: s.count + 1 }, "retry") }
  action exhaust = all { s.phase == 1, s.count >= s.limit, record({ ...s, phase: 3, feedback: false }, "exhaust") }
  action exhaustedGoto = all { s.phase == 1, s.count >= s.limit, record({ ...s, phase: 4, feedback: false }, "exhaustGoto") }
  action resume = record(s, "resume")
  action idle = record(s, "idle")
  action step = any { pass, gateFail, retry, exhaust, exhaustedGoto, resume, idle }
  // Supplementary failure and success behavior.
  val gate_behavior = all {
    event == "fail" implies s.phase == 1 and s.feedback,
    event == "pass" implies s.phase == 2 and not(s.feedback),
    s.phase == 1 implies s.feedback,
  }
  // Supplementary retry-budget behavior.
  val budget_behavior = all {
    0.to(s.limit).contains(s.count),
    event == "retry" implies previous.count < previous.limit and s.count == previous.count + 1,
    event != "retry" implies s.count == previous.count,
    previous.phase >= 2 implies s.phase == previous.phase and s.effects == previous.effects,
  }
  // Supplementary resume and exhaustion behavior.
  val resume_behavior = all {
    event == "resume" implies s == previous,
    event == "retry" or event == "exhaust" or event == "exhaustGoto" implies s.effects == previous.effects,
    event == "exhaust" or event == "exhaustGoto" implies not(s.feedback),
  }
  val safety = gate_behavior and budget_behavior and resume_behavior
  run retryPassTest = init.then(gateFail).then(resume).then(retry).then(pass)
    .expect(s.phase == 2 and s.count == 1 and not(s.feedback) and s.effects == 2 and safety)
  run exhaustionTest = init.then(gateFail).then(retry).then(gateFail).then(retry).then(gateFail).then(exhaust)
    .then(resume).expect(s.phase == 3 and s.count == 2 and s.effects == 3 and not(s.feedback) and safety)
  run exhaustGotoTest = init.then(gateFail).then(retry).then(gateFail).then(retry).then(gateFail).then(exhaustedGoto)
    .expect(s.phase == 4 and not(s.feedback) and safety)
}
