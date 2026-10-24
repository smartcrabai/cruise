// SDK-qualified model arrays: plan sections 3.1, 3.3, 3.5.
// Representative input: ["shared:high", {sdk: other, model:"shared:low"},
//                       "shared:low", "next:high"]. Only known effort is erased.
// Fail denotes a retryable model-missing failure under enabled fallback and a
// nonzero retry budget. Other error classes/backoff are concrete Rust checks,
// not claims of this small model. No real provider/model availability assumed.
module sdk_models {
  type State = { base: int, owner: int, phase: int, cursor: int, current: int,
    calls: int, attempts: int, visible: bool, cool: Set[int],
    resumeOwner: int, resumeId: int, checkpointOwner: int, checkpointId: int }
  var s: State
  var previous: State
  var event: str
  // SDKs 0/1 represent jcode/claude in either order. Model IDs 0/1 are shared/next.
  pure def sdk(base: int, index: int): int = if (index == 1) 1 - base else base
  pure def model(index: int): int = if (index == 3) 1 else 0
  pure def key(base: int, index: int): int = 2 * sdk(base, index) + model(index)
  pure def available(base: int, cursor: int, cool: Set[int]): int =
    if (cursor <= 0 and not(cool.contains(key(base, 0)))) 0
    else if (cursor <= 1 and not(cool.contains(key(base, 1)))) 1
    else if (cursor <= 2 and not(cool.contains(key(base, 2)))) 2
    else if (cursor <= 3 and not(cool.contains(key(base, 3)))) 3 else 4
  pure def initial(base: int, owner: int): State = {
    base: base, owner: owner, phase: 0, cursor: 0, current: -1, calls: 1,
    attempts: 0, visible: false, cool: Set(), resumeOwner: -1, resumeId: 0,
    checkpointOwner: -1, checkpointId: 0 }
  action initWith(base: int, owner: int): bool = all {
    s' = initial(base, owner), previous' = initial(base, owner), event' = "init" }
  action init = {
    nondet base = Set(0, 1).oneOf()
    nondet owner = Set(-1, 0, 1).oneOf()
    initWith(base, owner)
  }
  action record(updated: State, name: str): bool = all {
    s' = updated, previous' = s, event' = name }
  // The original caller's ID is 1. Attempt-issued IDs are >=2, never reused.
  action start = {
    val index = available(s.base, s.cursor, s.cool)
    val target = sdk(s.base, index)
    all { s.phase == 0, index < 4,
      record({ ...s, phase: 1, current: index, attempts: s.attempts + 1,
        visible: false, resumeOwner: if (s.owner == target) target else -1,
        resumeId: if (s.owner == target) 1 else 0 }, "start") }
  }
  action blocked = all { s.phase == 0, available(s.base, s.cursor, s.cool) == 4,
    record({ ...s, phase: 2 }, "blocked") }
  action checkpoint = all { s.phase == 1,
    record({ ...s, checkpointOwner: sdk(s.base, s.current),
      checkpointId: s.attempts + 1 }, "checkpoint") }
  action text = all { s.phase == 1, record({ ...s, visible: true }, "text") }
  action modelMissing = all { s.phase == 1,
    record({ ...s, phase: if (s.visible) 2 else 0, cursor: s.current + 1,
      cool: s.cool.union(Set(key(s.base, s.current))) }, "fail") }
  action succeed = all { s.phase == 1, record({ ...s, phase: 2 }, "succeed") }
  action cancel = all { s.phase == 1, record({ ...s, phase: 2 }, "cancel") }
  // A second caller/policy within the same cooldown window, not a process restart.
  action newCall = all { s.phase == 2, s.calls < 2,
    record({ ...s, phase: 0, cursor: 0, current: -1, calls: s.calls + 1,
      visible: false }, "newCall") }
  action idle = record(s, "idle")
  action step = any { start, blocked, checkpoint, text, modelMissing, succeed, cancel, newCall, idle }

  val selection = all {
    // The effort-only duplicate is never an independently eligible attempt.
    s.current != 2,
    event == "start" implies all {
      s.current == available(previous.base, previous.cursor, previous.cool),
      not(previous.cool.contains(key(s.base, s.current))),
      s.current == 1 implies sdk(s.base, s.current) != s.base,
      s.current != 1 implies sdk(s.base, s.current) == s.base,
    },
  }
  val resumeSafety = event == "start" implies all {
    s.resumeId == 0 or s.resumeId == 1,
    s.resumeId == 1 iff s.owner == sdk(s.base, s.current),
    s.resumeId == 1 implies s.resumeOwner == sdk(s.base, s.current),
    s.resumeId != previous.checkpointId or previous.checkpointId == 0,
  }
  val checkpointSafety = event == "checkpoint" implies all {
    s.checkpointOwner == sdk(s.base, s.current), s.checkpointId == s.attempts + 1,
  }
  val cooldownSafety = all {
    previous.cool.subseteq(s.cool),
    event == "fail" implies s.cool == previous.cool.union(Set(key(previous.base, previous.current))),
    event != "fail" implies s.cool == previous.cool,
    event == "newCall" implies s.cool == previous.cool,
  }
  val replaySafety = all {
    event == "fail" and previous.visible implies s.phase == 2,
    event == "start" implies previous.phase == 0,
    event == "cancel" implies s.phase == 2 and s.attempts == previous.attempts,
  }
  val domains = all { Set(0, 1).contains(s.base), Set(-1, 0, 1).contains(s.owner),
    0.to(2).contains(s.phase), 0.to(4).contains(s.cursor),
    (-1).to(3).contains(s.current), 1.to(2).contains(s.calls),
    0.to(6).contains(s.attempts), s.cool.subseteq(0.to(3)),
    s.phase == 1 implies s.current >= 0 and s.current < 4,
    s.base == previous.base, s.owner == previous.owner }
  val safety = selection and resumeSafety and checkpointSafety and cooldownSafety and replaySafety and domains

  run crossSdkFallbackTest = initWith(0, 0).then(start).then(checkpoint).then(modelMissing)
    .then(start).expect(s.current == 1 and s.resumeId == 0 and safety)
    .then(checkpoint).then(modelMissing).then(start)
    .expect(s.current == 3 and s.resumeId == 1 and s.attempts == 3 and safety)
  run reverseInheritanceTest = initWith(1, 1).then(start).then(modelMissing).then(start)
    .expect(sdk(s.base, s.current) == 0 and safety).then(modelMissing).then(start)
    .expect(sdk(s.base, s.current) == 1 and s.current == 3 and safety)
  run visibleFailureTest = initWith(0, -1).then(start).then(text).then(modelMissing)
    .expect(s.phase == 2 and s.attempts == 1 and safety)
  run cooldownSurvivesPolicyTest = initWith(0, 1).then(start).then(modelMissing).then(start)
    .then(succeed).then(newCall).then(start)
    .expect(s.current == 1 and s.resumeId == 1 and s.attempts == 3 and safety)
  run cancellationTest = initWith(1, 0).then(start).then(checkpoint).then(cancel)
    .expect(s.phase == 2 and s.checkpointOwner == 1 and s.resumeId == 0 and safety)
  run allCoolingTest = initWith(0, -1).then(start).then(modelMissing).then(start)
    .then(modelMissing).then(start).then(modelMissing).then(blocked).then(newCall).then(blocked)
    .expect(s.phase == 2 and s.attempts == 3 and safety)
}
