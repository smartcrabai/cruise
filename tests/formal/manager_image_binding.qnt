// A single staged image; digest and repo stand for validated private bytes.
// A started provider request is not automatically resent after process loss.
module manager_image_binding {
  type Diagram = { state: str, via: str }
  type State = { repo: int, stored: int, owned: bool, messageConsent: bool,
    turnSends: int, reviewed: int, revision: int, approved: int,
    allowedTask: int, delivered: int, intact: bool }
  var s: State
  var diagram: Diagram
  pure val initial: State = { repo: 1, stored: 0, owned: false, messageConsent: false,
    turnSends: 0, reviewed: 0, revision: 0, approved: 0, allowedTask: 0,
    delivered: 0, intact: true }
  action init = all {
    s' = initial, diagram' = { state: "No image", via: "start" },
  }
  action go(state: str, via: str, after: State): bool = all {
    diagram' = { state: state, via: via }, s' = after,
  }
  action stage(valid: bool, repo: int): bool = all {
    diagram.state == "No image",
    if (valid and repo == s.repo)
      go("Private staged bytes", "validate type / size / scope", { ...s, stored: 1, owned: true })
    else go("Blocked", "invalid or foreign upload", s),
  }
  action saveTurn = all {
    diagram.state == "Private staged bytes", s.owned, s.intact,
    go("Turn saved", "explicit send; persist first", { ...s, messageConsent: true }),
  }
  action restartQueued = all {
    diagram.state == "Turn saved",
    go("Saved turn needs operator", "restart keeps saved reference", s),
  }
  action resumeQueued = all {
    diagram.state == "Saved turn needs operator", s.intact, s.messageConsent,
    go("Turn saved", "operator confirms retry of unstarted input", s),
  }
  action startTurn = all {
    diagram.state == "Turn saved", s.messageConsent, s.intact, s.turnSends == 0,
    go("Provider turn started", "send authorized turn once", { ...s, turnSends: 1 }),
  }
  action loseProvider = all {
    diagram.state == "Provider turn started",
    go("Interrupted; bytes retained", "restart; no implicit resend", s),
  }
  action completeTurn = all {
    diagram.state == "Provider turn started",
    go("Conversation ready", "record reply", s),
  }
  action preview(task: int): bool = all {
    Set("Conversation ready", "Task preview").contains(diagram.state),
    s.intact, s.revision < 2, s.owned, Set(1, 2).contains(task),
    go("Task preview", "review digest and task", { ...s, reviewed: s.stored, allowedTask: task, revision: s.revision + 1 }),
  }
  action approve(revision: int): bool = all {
    diagram.state == "Task preview", revision == s.revision, s.intact, s.reviewed == s.stored,
    go("Task attachment approved", "explicit exact revision", { ...s, approved: revision }),
  }
  action mutate = all {
    Set("Task preview", "Task attachment approved").contains(diagram.state),
    go("Blocked", "missing or changed bytes", { ...s, intact: false }),
  }
  action dispatch(task: int, repo: int): bool = all {
    diagram.state == "Task attachment approved", s.approved == s.revision, s.approved > 0,
    s.intact, s.reviewed == s.stored, s.allowedTask == task, s.repo == repo,
    go("Approved child copied image", "verify scope and digest; copy once", { ...s, delivered: task }),
  }
  val safety = all {
    s.turnSends <= 1, s.turnSends > 0 implies (s.messageConsent and s.owned),
    s.delivered > 0 implies (s.owned and s.approved == s.revision and s.intact
      and s.delivered == s.allowedTask and s.reviewed == s.stored),
    diagram.state == "Task preview" implies s.delivered == 0,
  }
  action toTurn = init.then(stage(true, 1)).then(saveTurn)
  action toReview = toTurn.then(startTurn).then(completeTurn).then(preview(1))
  run restartKeepsBytesBeforeFirstSendTest = toTurn.then(restartQueued).then(resumeQueued).then(startTurn)
    .expect(s.turnSends == 1 and s.stored == 1 and safety)
  run restartedQueuedTurnDoesNotAutoSendTest = toTurn.then(restartQueued).then(startTurn).fail()
  run restartDoesNotReplayStartedTurnTest = toTurn.then(startTurn).then(loseProvider)
    .expect(s.turnSends == 1 and s.stored == 1 and safety)
  run lostTurnCannotAutoSendTest = toTurn.then(startTurn).then(loseProvider).then(startTurn).fail()
  run exactTaskCopiesImageTest = toReview.then(approve(1)).then(dispatch(1, 1))
    .expect(s.delivered == 1 and safety)
  run taskMismatchCannotReadTest = toReview.then(approve(1)).then(dispatch(2, 1)).fail()
  run repoMismatchCannotReadTest = toReview.then(approve(1)).then(dispatch(1, 2)).fail()
  run staleApprovalCannotLaunchTest = toReview.then(preview(2)).then(approve(1)).fail()
  run unapprovedChildCannotReadTest = toReview.then(dispatch(1, 1)).fail()
  run wrongRepoUploadIsRefusedTest = init.then(stage(true, 2)).expect(s.stored == 0 and safety)
  run changedApprovedBytesAreBlockedTest = toReview.then(approve(1)).then(mutate)
    .expect(s.delivered == 0 and safety)
}
