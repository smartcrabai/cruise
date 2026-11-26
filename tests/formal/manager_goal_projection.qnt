// One Goal crosses the existing redacted provider boundary. The operator's
// copy keeps the complete record; this model grants no new authority or access.
module manager_goal_projection {
  type Diagram = { state: str, via: str }
  type State = { status: str, stored: Set[str], captured: str, sent: Set[str], sentStatus: str }
  var s: State
  var previous: State
  var diagram: Diagram
  pure val summary = Set("id", "objective", "status")
  pure val detail = Set("tasks", "attempts", "verification")
  pure val privateFields = Set("origin", "credentials", "workflow config")
  pure val storedFields = summary.union(detail).union(privateFields)
  pure val statuses = Set("active", "paused", "blocked", "verifying", "awaiting merge", "completed")

  action start(status: str): bool = all {
    statuses.contains(status),
    s' = { status: status, stored: storedFields, captured: "none", sent: Set(), sentStatus: "none" },
    previous' = { status: status, stored: storedFields, captured: "none", sent: Set(), sentStatus: "none" },
    diagram' = { state: "Repository record", via: "start" },
  }
  action init = start("active")
  action go(state: str, via: str, after: State): bool = all {
    diagram' = { state: state, via: via }, previous' = s, s' = after,
  }
  action capture = all {
    Set("Repository record", "Provider sent summary", "Provider sent details").contains(diagram.state),
    go("Redacted provider snapshot", "capture current revision", { ...s, captured: s.status, sent: Set(), sentStatus: "none" }),
  }
  action complete = all {
    s.status == "awaiting merge",
    go(diagram.state, "finish in durable record", { ...s, status: "completed" }),
  }
  action send = all {
    diagram.state == "Redacted provider snapshot",
    if (s.captured == "completed")
      go("Provider sent summary", "ID / objective / status only", { ...s, sent: summary, sentStatus: s.captured })
    else
      go("Provider sent details", "keep unfinished Goal context", { ...s, sent: summary.union(detail), sentStatus: s.captured }),
  }
  val safety = all {
    s.stored == storedFields, s.stored == previous.stored,
    s.sent.intersect(privateFields) == Set(),
    s.sentStatus == "completed" implies s.sent == summary,
    statuses.exclude(Set("completed")).contains(s.sentStatus) implies s.sent == summary.union(detail),
  }

  run completedOnlySharesSummaryTest = start("completed").then(capture).then(send)
    .expect(s.sent == summary and s.stored == storedFields and safety)
  run blockedGoalKeepsUsefulContextTest = start("blocked").then(capture).then(send)
    .expect(s.sent == summary.union(detail) and safety)
  run awaitingMergeIsNotCompletedTest = start("awaiting merge").then(capture).then(send)
    .expect(s.sentStatus == "awaiting merge" and safety)
  run pausedAndVerifyingKeepDetailsTest = start("paused").then(capture).then(send)
    .expect(s.sent == summary.union(detail) and safety)
    .then(start("verifying")).then(capture).then(send)
    .expect(s.sent == summary.union(detail) and safety)
  run completedDuringSnapshotIsVisibleOnNextTurnTest = start("awaiting merge").then(capture).then(complete)
    .then(send).expect(s.sentStatus == "awaiting merge" and safety).then(capture).then(send)
    .expect(s.sentStatus == "completed" and s.sent == summary and safety)
  run sendWithoutSnapshotTest = init.then(send).fail()
}
