// Jcode-only ordered model fallback, including effort-identity deduplication.
module sdk_models {
  type State = { phase: int, cursor: int, current: int, attempts: int,
    attempted: Set[int], visible: bool, event: str }
  var s: State
  var previous: State
  pure val initial = { phase: 0, cursor: 0, current: -1, attempts: 0,
    attempted: Set(), visible: false, event: "init" }
  pure def candidateAt(position: int): int = if (position <= 1) 0 else 1
  pure def nextPosition(cursor: int, attempted: Set[int]): int =
    if (cursor <= 0 and not(attempted.contains(0))) 0
    else if (cursor <= 1 and not(attempted.contains(0))) 1
    else if (cursor <= 2 and not(attempted.contains(1))) 2
    else 3
  action init = all { s' = initial, previous' = initial }
  action record(updated: State, name: str): bool = all {
    previous' = s, s' = { ...updated, event: name }
  }
  action start = {
    val position = nextPosition(s.cursor, s.attempted)
    all {
      s.phase == 0,
      if (position == 3)
        record({ ...s, phase: 2, current: -1 }, "blocked")
      else
        record({ ...s, phase: 1, current: candidateAt(position),
          attempts: s.attempts + 1 }, "start")
    }
  }
  action failure = all {
    s.phase == 1,
    record({ ...s, phase: if (s.visible) 2 else 0, cursor: s.cursor + 1,
      attempted: s.attempted.union(Set(s.current)) }, "fail")
  }
  action text = all { s.phase == 1, record({ ...s, visible: true }, "text") }
  action succeed = all { s.phase == 1, record({ ...s, phase: 2 }, "succeed") }
  action idle = record(s, "idle")
  action step = any { start, failure, text, succeed, idle }

  val selection = s.event != "start" or all {
    previous.phase == 0, s.current >= 0,
    not(previous.attempted.contains(s.current)),
    s.current == candidateAt(nextPosition(previous.cursor, previous.attempted))
  }
  val fallbackSafety = all {
    previous.attempted.subseteq(s.attempted),
    s.event == "fail" implies s.attempted == previous.attempted.union(Set(previous.current)),
    previous.visible implies s.visible and s.event != "start",
    s.event == "start" and previous.attempts > 0 implies
      previous.phase == 0 and not(previous.visible)
      and (previous.event == "fail" or previous.event == "idle"),
    s.attempts <= 2
  }

  val domains = s.phase >= 0 and s.phase <= 2 and s.cursor >= 0 and s.cursor <= 4
    and s.current >= -1 and s.current <= 1 and s.attempts >= 0 and s.attempts <= 2
    and s.attempted.subseteq(Set(0, 1))
  val safety = selection and fallbackSafety and domains

  run orderedFallbackTest = init.then(start).expect(s.current == 0 and safety)
    .then(failure).then(start)
    .expect(s.current == 1 and s.attempts == 2 and safety).then(succeed)
    .expect(s.phase == 2 and safety)
  run visibleFailureStopsTest = init.then(start).then(text).then(failure).then(idle)
    .expect(s.phase == 2 and s.attempts == 1 and s.visible and safety)
  run exhaustedCandidatesTest = init.then(start).then(failure).then(start).then(failure)
    .then(start).expect(s.phase == 2 and s.attempts == 2 and s.event == "blocked" and safety)
}
