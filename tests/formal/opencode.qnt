// OpenCode V2 observable attempt contract, plan sections 4.3, 5.2, 5.3, 5.5.
// Two attempts, one prompt per attempt, one representative external tool effect.
// setup abstracts validated V2/plugin/environment, not HTTP implementation details.
// evidence abstracts a matching execution.succeeded after current inbox delivery,
// from SSE or reconciled durable log. HTTP acceptance/idle/EOF are NOT evidence.
// stop abstracts completion of CLI, bridge and storage cleanup, not crash recovery.
// previous/event are observers. No liveness, OS sandbox, or refinement proof claim.
module opencode {
  type State = { phase: int, attempts: int, prompts: int, alive: bool,
    checkpoint: bool, subscribed: bool, barrier: bool, evidence: bool,
    effects: int, result: int }
  // phases: 0 new, 1 setup, 2 checkpointed, 3 subscribed, 4 executing,
  // 5 resolved, 6 stopped, 7 published. result: 0 none, 1 success,
  // 2 safe provider failure, 3 uncertain/local failure, 4 cancelled.
  var s: State
  var previous: State
  var event: str
  pure val initial: State = { phase: 0, attempts: 0, prompts: 0,
    alive: false, checkpoint: false, subscribed: false, barrier: false,
    evidence: false, effects: 0, result: 0 }
  action init = all { s' = initial, previous' = initial, event' = "init" }
  action record(updated: State, name: str): bool = all {
    previous' = s, s' = updated, event' = name,
  }
  action start = all {
    s.phase == 0 or (s.phase == 6 and s.result == 2 and not(s.barrier)),
    s.attempts < 2,
    record({ ...s, phase: 1, attempts: s.attempts + 1, alive: true,
      checkpoint: false, subscribed: false, evidence: false, result: 0 }, "start"),
  }
  // SessionReady acknowledgment must succeed before input admission.
  action checkpoint = all { s.phase == 1,
    record({ ...s, phase: 2, checkpoint: true }, "checkpoint") }
  action subscribe = all { s.phase == 2,
    record({ ...s, phase: 3, subscribed: true }, "subscribe") }
  action submit = all { s.phase == 3,
    record({ ...s, phase: 4, prompts: s.prompts + 1 }, "submit") }
  // The authenticated native bridge reports effects even without visible text.
  action effect = all { s.phase == 4, s.effects == 0,
    record({ ...s, effects: 1, barrier: true }, "effect") }
  action succeed = all { s.phase == 4,
    record({ ...s, phase: 5, result: 1, evidence: true }, "succeed") }
  action providerFailure = all { s.phase == 4,
    record({ ...s, phase: 5, result: if (s.barrier) 3 else 2 }, "providerFailure") }
  // Lost acceptance, unrecoverable log, malformed protocol, or setup rejection.
  action uncertain = all { 1.to(4).contains(s.phase),
    record({ ...s, phase: 5, result: 3, barrier: true }, "uncertain") }
  action cancel = all { 1.to(4).contains(s.phase),
    record({ ...s, phase: 5, result: 4, barrier: true }, "cancel") }
  action stop = all { s.phase == 5,
    record({ ...s, phase: 6, alive: false }, "stop") }
  action publish = all { s.phase == 6,
    record({ ...s, phase: 7 }, "publish") }
  // Foreign/past-turn/text-ended/idle events cannot resolve the attempt. A
  // recoverable disconnect can replay durable evidence, never resubmit input.
  action irrelevant = record(s, "irrelevant")
  action step = any { start, checkpoint, subscribe, submit, effect, succeed,
    providerFailure, uncertain, cancel, stop, publish, irrelevant }

  val admission = all {
    0.to(2).contains(s.prompts), s.prompts <= s.attempts,
    event == "submit" implies previous.checkpoint and previous.subscribed
      and previous.phase == 3 and s.prompts == previous.prompts + 1,
    event != "submit" implies s.prompts == previous.prompts,
  }
  val retrySafety = all {
    previous.barrier implies s.barrier and s.prompts == previous.prompts,
    event == "start" and previous.attempts > 0 implies
      previous.phase == 6 and not(previous.alive)
        and previous.result == 2 and not(previous.barrier),
    s.effects > 0 implies s.barrier,
  }
  val completion = all {
    s.result == 1 implies s.evidence,
    s.phase == 7 implies not(s.alive) and s.result != 0,
    event == "publish" implies previous.phase == 6 and not(previous.alive),
    event == "irrelevant" implies s == previous,
    previous.phase == 7 implies s == previous,
  }
  val domains = all { 0.to(7).contains(s.phase), 0.to(2).contains(s.attempts),
    0.to(1).contains(s.effects), 0.to(4).contains(s.result),
    s.phase >= 1 and s.phase <= 5 implies s.alive }
  val safety = admission and retrySafety and completion and domains

  run successTest = init.then(start).then(checkpoint).then(subscribe).then(submit)
    .then(irrelevant).then(succeed).then(stop).then(publish)
    .expect(s.result == 1 and s.prompts == 1 and safety)
  run silentEffectFailureTest = init.then(start).then(checkpoint).then(subscribe)
    .then(submit).then(effect).then(providerFailure).then(stop).then(publish)
    .expect(s.result == 3 and s.prompts == 1 and s.barrier and safety)
  run safeFallbackTest = init.then(start).then(checkpoint).then(subscribe).then(submit)
    .then(providerFailure).then(stop).then(start).then(checkpoint).then(subscribe)
    .then(submit).then(succeed).then(stop).then(publish)
    .expect(s.attempts == 2 and s.prompts == 2 and s.result == 1 and safety)
  run uncertainTest = init.then(start).then(checkpoint).then(subscribe).then(submit)
    .then(uncertain).then(stop).then(publish)
    .expect(s.attempts == 1 and s.prompts == 1 and s.barrier and s.result == 3 and safety)
  run rejectedCheckpointTest = init.then(start).then(uncertain).then(stop).then(publish)
    .expect(s.prompts == 0 and not(s.checkpoint) and s.result == 3 and safety)
  run cancellationTest = init.then(start).then(checkpoint).then(subscribe).then(submit)
    .then(cancel).then(stop).then(publish)
    .expect(s.attempts == 1 and s.result == 4 and not(s.alive) and s.prompts == 1 and safety)
}
