// Contract: plan.md sections 0, 2.2, 2.3, 2.4 and 8.
// The workflow candidate universe includes planning and execution. Per-request
// candidates represent the selected prompt/planning chain after retry policy.
// SDKs: 0=jcode, 1=claude, 2=opencode. Entry owner: -1=absent,
// 3=bare string (workflow namespace), otherwise explicit SDK. Tokens 1/2
// distinguish workflow/step model IDs, 7/9 are existing user env values.
// One request, at most two fallback attempts. We abstract provider routing as
// an opaque model token. No claims about provider availability or swarm workers
// with explicitly chosen models, nor clearing resumed jcode overrides.
module subagent_model {
  type Input = { base: int, global: int, local: int, purpose: int,
    wellformed: bool, command: bool, candidates: Set[int], workflowCandidates: Set[int] }
  type State = { phase: int, attempt: int, target: int, applied: int,
    envModel: int, envForce: int, swarm: int, sent: bool, failed: bool }
  var input: Input
  var s: State
  var previous: State
  var event: str
  pure def owner(entry: int, base: int): int = if (entry == 3) base else entry
  pure def allowed(entry: int, i: Input, candidates: Set[int]): bool = entry == -1 or all {
    i.wellformed, not(i.command), Set(0, 1).contains(owner(entry, i.base)),
    candidates.contains(owner(entry, i.base)) }
  pure def accepted(i: Input): bool = allowed(i.global, i, i.workflowCandidates) and
    allowed(i.local, i, i.candidates) and (i.local == -1 or i.purpose == 0)
  pure def chosen(i: Input): int =
    if (i.purpose == 2) -1 else if (i.purpose == 0 and i.local != -1) i.local else i.global
  pure def token(i: Input): int = if (i.purpose == 0 and i.local != -1) 2 else 1
  pure def initial: State = { phase: 0, attempt: 0, target: -1, applied: 0,
    envModel: 7, envForce: 9, swarm: 0, sent: false, failed: false }
  action initWith(i: Input): bool = all {
    input' = i, s' = initial, previous' = initial, event' = "init" }
  action init = {
    nondet base = Set(0, 1, 2).oneOf()
    nondet global = Set(-1, 0, 1, 2, 3).oneOf()
    nondet local = Set(-1, 0, 1, 2, 3).oneOf()
    nondet purpose = Set(0, 1, 2).oneOf()
    nondet wellformed = Set(false, true).oneOf()
    nondet command = Set(false, true).oneOf()
    nondet candidates = Set(Set(0), Set(1), Set(2), Set(0, 1), Set(0, 2), Set(1, 2), Set(0, 1, 2)).oneOf()
    nondet otherPhase = Set(Set(), Set(0), Set(1), Set(2), Set(0, 1), Set(0, 2), Set(1, 2), Set(0, 1, 2)).oneOf()
    initWith({ base: base, global: global, local: local, purpose: purpose,
      wellformed: wellformed, command: command, candidates: candidates,
      workflowCandidates: candidates.union(otherPhase) })
  }
  action record(updated: State, name: str): bool = all {
    input' = input, previous' = s, s' = updated, event' = name }
  action validate = all { s.phase == 0,
    record({ ...s, phase: if (accepted(input)) 1 else 5 }, "validate") }
  action attemptOn(target: int): bool = all {
      input.candidates.contains(target), s.phase == 1, s.attempt < 2,
      record({ ...s, phase: 2, attempt: s.attempt + 1, target: target,
        applied: 0, envModel: 7, envForce: 9, swarm: 0, sent: false, failed: false }, "attempt") }
  action attempt = {
    nondet target = input.candidates.oneOf()
    attemptOn(target)
  }
  action configure = {
    val entry = chosen(input)
    val apply = entry != -1 and owner(entry, input.base) == s.target
    val model = if (apply) token(input) else 0
    all { s.phase == 2,
      record({ ...s, phase: 3, applied: model,
        envModel: if (apply and s.target == 1) model else 7,
        envForce: if (apply and s.target == 1) 1 else 9,
        swarm: if (apply and s.target == 0) model else 0 }, "configure") }
  }
  action configurationError = all { s.phase == 3, s.swarm != 0,
    record({ ...s, phase: 5, failed: true }, "agents-error") }
  action send = all { s.phase == 3,
    record({ ...s, phase: 4, sent: true }, "send") }
  // Only an eligible retryable failure before output permits the second attempt.
  action fallback = all { s.phase == 4, s.attempt < 2,
    record({ ...s, phase: 1 }, "fallback") }
  action finish = all { s.phase == 4, record({ ...s, phase: 5 }, "finish") }
  action idle = record(s, "idle")
  action step = any { validate, attempt, configure, configurationError, send, fallback, finish, idle }

  val rejection = not(accepted(input)) implies s.attempt == 0 and not(s.sent)
  val selection = s.applied != 0 implies all {
    chosen(input) != -1, s.target == owner(chosen(input), input.base),
    s.applied == token(input), Set(0, 1).contains(s.target) }
  val completeness = Set(3, 4).contains(s.phase) implies
    ((s.applied != 0) iff (chosen(input) != -1 and owner(chosen(input), input.base) == s.target))
  val scope = input.purpose == 2 implies s.applied == 0
  val environment = all {
    s.applied != 0 and s.target == 1 implies s.envModel == s.applied and s.envForce == 1,
    not(s.applied != 0 and s.target == 1) implies s.envModel == 7 and s.envForce == 9,
    s.swarm != 0 iff (s.applied != 0 and s.target == 0),
    s.swarm != 0 implies s.swarm == s.applied }
  val ordering = all {
    event == "send" implies previous.phase == 3 and not(previous.failed),
    s.failed implies not(s.sent) and s.phase == 5,
    event == "attempt" implies accepted(input) and previous.phase == 1,
    s.attempt >= 0 and s.attempt <= 2 }
  val safety = rejection and selection and completeness and scope and environment and ordering

  pure def fixture(g: int, l: int, p: int): Input = { base: 0, global: g,
    local: l, purpose: p, wellformed: true, command: false,
    candidates: Set(0, 1), workflowCandidates: Set(0, 1) }
  action jcodeAttempt = attemptOn(0)
  action claudeAttempt = attemptOn(1)
  run precedenceAndFallbackTest = initWith(fixture(0, 1, 0)).then(validate)
    .then(jcodeAttempt).then(configure).expect(s.applied == 0 and safety)
    .then(send).then(fallback).then(claudeAttempt).then(configure)
    .expect(s.applied == 2 and s.envModel == 2 and s.envForce == 1 and safety).then(send)
  run planningTest = initWith(fixture(3, -1, 1)).then(validate).then(jcodeAttempt)
    .then(configure).expect(s.swarm == 1 and safety).then(send)
  run metadataTest = initWith(fixture(1, -1, 2)).then(validate).then(claudeAttempt)
    .then(configure).expect(s.applied == 0 and s.envModel == 7 and safety).then(send)
  run legacyEnvTest = initWith(fixture(-1, -1, 0)).then(validate).then(claudeAttempt)
    .then(configure).expect(s.envModel == 7 and s.envForce == 9 and safety)
  run rejectUnsupportedTest = initWith(fixture(2, -1, 0)).then(validate)
    .expect(s.phase == 5 and s.attempt == 0 and safety)
  run agentsFailureTest = initWith(fixture(0, -1, 0)).then(validate).then(jcodeAttempt)
    .then(configure).then(configurationError).expect(s.failed and not(s.sent) and safety)
  run otherPhaseApplicabilityTest = initWith({ ...fixture(1, -1, 0), candidates: Set(0) })
    .then(validate).expect(s.phase == 1).then(jcodeAttempt).then(configure)
    .expect(s.applied == 0 and safety).then(send)
  run commandRejectionTest = initWith({ ...fixture(1, -1, 0), command: true })
    .then(validate).expect(s.phase == 5 and s.attempt == 0 and safety)
  run malformedRejectionTest = initWith({ ...fixture(1, -1, 0), wellformed: false })
    .then(validate).expect(s.phase == 5 and s.attempt == 0 and safety)
  run localApplicabilityRejectionTest = initWith({ ...fixture(-1, 1, 0), candidates: Set(0) })
    .then(validate).expect(s.phase == 5 and s.attempt == 0 and safety)
}
