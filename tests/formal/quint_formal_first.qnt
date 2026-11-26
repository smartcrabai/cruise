module quint_formal_first {
  type Diagram = { state: str, via: str }
  type State = {
    formal: bool, canPublish: bool, revision: int, shown: int, approved: int,
    epoch: int, evidence: Set[str], prEvidence: Set[str]
  }
  var diagram: Diagram
  var s: State

  pure val evidenceKinds = Set("command output", "saved value / before and after", "recording link")

  action start(formal: bool, canPublish: bool): bool = all {
    diagram' = { state: "Resolve run flags", via: "start" },
    s' = {
      formal: formal, canPublish: canPublish, revision: 0, shown: 0, approved: 0,
      epoch: 0, evidence: Set(), prEvidence: Set()
    }
  }
  action init = {
    nondet formal = Set(false, true).oneOf()
    nondet canPublish = Set(false, true).oneOf()
    start(formal, canPublish)
  }
  action go(state: str, via: str, after: State): bool = all {
    diagram' = { state: state, via: via }, s' = after
  }

  action beginFormal = all {
    diagram.state == "Resolve run flags", s.formal,
    go("Draft Quint model", "formal_methods on", s)
  }
  action beginLegacy = all {
    diagram.state == "Resolve run flags", not(s.formal),
    go("Existing main workflow", "formal_methods off", s)
  }
  action draft = all {
    diagram.state == "Draft Quint model",
    go("Check Quint and render PNG", "new revision", { ...s, revision: s.revision + 1, shown: 0 })
  }
  action checkedAndRendered = all {
    diagram.state == "Check Quint and render PNG",
    go("Await model decision", "checks pass; PNG ready", { ...s, shown: s.revision, epoch: s.epoch + 1 })
  }
  action previewFailed = all {
    diagram.state == "Check Quint and render PNG",
    go("Preview blocked", "check or render failed", s)
  }
  action retryPreview = all {
    diagram.state == "Preview blocked",
    go("Check Quint and render PNG", "retry", s)
  }
  action suspend = all {
    diagram.state == "Await model decision",
    go("Decision suspended", "interrupt", { ...s, shown: 0 })
  }
  action resume = all {
    diagram.state == "Decision suspended",
    go("Check Quint and render PNG", "resume", s)
  }
  action requestChanged = all {
    Set("Await model decision", "Model approved", "Preview blocked").contains(diagram.state),
    go("Draft Quint model", "candidate changed", { ...s, shown: 0, approved: 0 })
  }
  action decide(choice: str, epoch: int, revision: int): bool = all {
    diagram.state == "Await model decision",
    epoch == s.epoch, revision == s.shown, revision == s.revision,
    any {
      all { choice == "revise", go("Draft Quint model", "request revision", { ...s, shown: 0 }) },
      all { choice == "reject", go("Rejected: stop", "reject", { ...s, shown: 0 }) },
      all { choice == "approve", go("Model approved", "approve", { ...s, approved: revision }) }
    }
  }
  action answer(choice: str): bool = decide(choice, s.epoch, s.revision)
  action staleAnswer(epoch: int, revision: int): bool = all {
    diagram.state == "Await model decision", epoch != s.epoch or revision != s.revision,
    go("Await model decision", "ignore stale answer", s)
  }
  action beginImplementation = all {
    diagram.state == "Model approved", s.approved > 0, s.approved == s.revision,
    go("Existing main workflow", "start approved work", s)
  }
  action finalCheckFailed = all {
    diagram.state == "Verify Quint against final behavior",
    go("Final verification blocked", "check failed", s)
  }
  action retryFinalCheck = all {
    diagram.state == "Final verification blocked",
    go("Verify Quint against final behavior", "retry approved model", s)
  }
  action completeLegacy = all {
    diagram.state == "Existing main workflow", not(s.formal),
    go("Main workflow complete", "existing checks", s)
  }
  action completeFormal = all {
    diagram.state == "Existing main workflow", s.formal,
    go("Verify Quint against final behavior", "implementation and review complete", s)
  }
  action checkAndRecord(evidence: Set[str]): bool = all {
    diagram.state == "Verify Quint against final behavior", evidence.subseteq(evidenceKinds),
    go("Main workflow complete", "verified; retain observed evidence", { ...s, evidence: evidence })
  }
  action finishWithoutPr = all {
    diagram.state == "Main workflow complete", not(s.canPublish),
    go("Done without PR", "no eligible publication", s)
  }
  action publishLegacy = all {
    diagram.state == "Main workflow complete", s.canPublish, not(s.formal),
    go("PR created", "existing publication", s)
  }
  action publishFormal = all {
    diagram.state == "Main workflow complete", s.canPublish, s.formal,
    go("PR created", "include available evidence", { ...s, prEvidence: s.evidence })
  }
  action stop = all {
    Set("Rejected: stop", "Done without PR", "PR created").contains(diagram.state),
    diagram' = diagram, s' = s
  }

  action step = any {
    beginFormal, beginLegacy, draft, checkedAndRendered, previewFailed, retryPreview, suspend, resume,
    finalCheckFailed, retryFinalCheck,
    requestChanged, answer("revise"), answer("reject"), answer("approve"),
    staleAnswer(s.epoch - 1, s.revision), beginImplementation, completeLegacy, completeFormal,
    { nondet evidence = evidenceKinds.powerset().oneOf(); checkAndRecord(evidence) },
    finishWithoutPr, publishLegacy, publishFormal, stop
  }

  val needsApproval = Set("Existing main workflow", "Verify Quint against final behavior",
    "Final verification blocked", "Main workflow complete", "Done without PR", "PR created")

  val invGate = (s.formal and needsApproval.contains(diagram.state)) implies
    (s.revision > 0 and s.approved == s.revision and s.shown == s.revision)
  val invPreview = diagram.state == "Await model decision" implies
    (s.formal and s.revision > 0 and s.shown == s.revision and s.epoch > 0 and s.approved == 0)
  val invRejected = diagram.state == "Rejected: stop" implies (s.approved == 0 and s.prEvidence == Set())
  val invOff = not(s.formal) implies (s.revision == 0 and s.shown == 0 and s.approved == 0 and s.epoch == 0)
  val invEvidence = all {
    s.evidence.subseteq(evidenceKinds),
    if (diagram.state == "PR created" and s.formal) s.prEvidence == s.evidence else s.prEvidence == Set(),
    diagram.state == "PR created" implies s.canPublish
  }
  val invSafety = invGate and invPreview and invRejected and invOff and invEvidence

  action toDecision = start(true, true).then(beginFormal).then(draft).then(checkedAndRendered)
  action toFormalPublication = toDecision.then(answer("approve")).then(beginImplementation).then(completeFormal)

  run formalWithEvidenceTest = toFormalPublication.then(checkAndRecord(evidenceKinds)).then(publishFormal)
    .expect(diagram.state == "PR created" and s.prEvidence == evidenceKinds and invSafety)
  run formalWithoutEvidenceTest = toFormalPublication.then(checkAndRecord(Set())).then(publishFormal)
    .expect(diagram.state == "PR created" and s.prEvidence == Set() and invSafety)
  run offPublishesNormallyTest = start(false, true).then(beginLegacy).then(completeLegacy).then(publishLegacy)
    .expect(diagram.state == "PR created" and s.revision == 0 and invSafety)
  run execNeverPublishesTest = start(true, false).then(beginFormal).then(draft).then(checkedAndRendered)
    .then(answer("approve")).then(beginImplementation).then(completeFormal)
    .then(checkAndRecord(Set("command output"))).then(finishWithoutPr)
    .expect(diagram.state == "Done without PR" and s.prEvidence == Set() and invSafety)
  run offExecNeverPublishesTest = start(false, false).then(beginLegacy).then(completeLegacy).then(finishWithoutPr)
    .expect(diagram.state == "Done without PR" and s.revision == 0 and invSafety)
  run rejectStopsTest = toDecision.then(answer("reject")).then(stop)
    .expect(diagram.state == "Rejected: stop" and invSafety)
  run revisionNeedsNewApprovalTest = toDecision.then(answer("revise")).then(draft).then(checkedAndRendered)
    .then(staleAnswer(1, 1)).expect(diagram.state == "Await model decision" and s.approved == 0 and invSafety)
    .then(decide("approve", 2, 2)).then(beginImplementation)
    .expect(diagram.state == "Existing main workflow" and s.approved == 2 and invSafety)
  run interruptionNeedsNewPromptTest = toDecision.then(suspend).then(resume).then(checkedAndRendered)
    .then(staleAnswer(1, 1)).expect(diagram.state == "Await model decision" and s.approved == 0 and invSafety)
    .then(decide("approve", 2, 1)).expect(s.approved == 1 and invSafety)
  run renderFailureBlocksApprovalTest = start(true, true).then(beginFormal).then(draft).then(previewFailed)
    .expect(diagram.state == "Preview blocked" and s.shown == 0 and s.epoch == 0 and invSafety)
    .then(retryPreview).then(checkedAndRendered).expect(diagram.state == "Await model decision" and invSafety)
  run failedDraftCanBeRepairedTest = start(true, true).then(beginFormal).then(draft).then(previewFailed)
    .then(requestChanged).then(draft).then(checkedAndRendered)
    .expect(diagram.state == "Await model decision" and s.shown == 2 and s.approved == 0 and invSafety)
  run finalVerificationFailureBlocksPrTest = toFormalPublication.then(finalCheckFailed)
    .expect(diagram.state == "Final verification blocked" and s.prEvidence == Set() and invSafety)
    .then(retryFinalCheck).then(checkAndRecord(Set("command output"))).then(publishFormal)
    .expect(diagram.state == "PR created" and s.prEvidence == Set("command output") and invSafety)
  run editedAfterApprovalInvalidatesTest = toDecision.then(answer("approve")).then(requestChanged)
    .then(draft).then(checkedAndRendered)
    .expect(diagram.state == "Await model decision" and s.shown == 2 and s.approved == 0 and invSafety)
  run editedDuringReviewInvalidatesTest = toDecision.then(requestChanged).then(draft).then(checkedAndRendered)
    .expect(diagram.state == "Await model decision" and s.shown == 2 and s.approved == 0 and invSafety)
}
