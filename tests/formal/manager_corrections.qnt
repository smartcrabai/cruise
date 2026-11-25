// Bounded abstraction of the owner-approved corrections of one Manager Goal: one failed
// task attempt and the single preview slot, with a contract amendment, a corrected resume
// and an evidence run, while a sibling integrates, the child worktree changes and the
// daemon restarts. A review is the tuple a preview was saved against. Atomic actions
// abstract the control and writer locks. Does not model Git, check commands, the plan
// text, capacity, pause, or more than one task.
// Production: manager_coordinator::corrections::{preview, approve, launch_mismatch},
// dispatch_pending_resumes, manager_publication::{run_evidence, validate_amendable}.
module manager_corrections {
  type Review = { tip: int, contract: int, attempt: int, worktree: int }
  type State = {
    tip: int, contract: int, attempt: int, worktree: int, planContract: int,
    kind: str, reviewed: Review,
    armed: bool, armedFor: Review, arms: int, refused: bool,
    starting: bool, running: bool, launches: int, delivered: int, plain: int,
    evidence: str, evidenceFor: Review, passed: int,
    integrated: bool, dependents: bool, blocked: bool,
    verified: bool, verifiedTip: int, verifiedContract: int, verifiedChecks: int,
    siblings: int, original: int, checks: int }
  var s: State
  var previous: State
  var event: str
  pure val nothing: Review = { tip: -1, contract: -1, attempt: -1, worktree: -1 }
  // Attempt 1 of the task has failed. `contract` counts approved amendments; `original`
  // and `checks` stand for the approved proposal text and its checks. `evidence` is
  // none, pending, started, resolved or unresolved.
  pure val initial: State = {
    tip: 0, contract: 0, attempt: 1, worktree: 0, planContract: 0,
    kind: "none", reviewed: nothing,
    armed: false, armedFor: nothing, arms: 0, refused: false,
    starting: false, running: false, launches: 0, delivered: 0, plain: 0,
    evidence: "none", evidenceFor: nothing, passed: 0,
    integrated: false, dependents: false, blocked: false,
    verified: false, verifiedTip: -1, verifiedContract: -1, verifiedChecks: -1,
    siblings: 2, original: 0, checks: 2 }
  action init = all { s' = initial, previous' = initial, event' = "init" }
  action record(n: State, e: str): bool = all { previous' = s, s' = n, event' = e }

  pure def facts(x: State): Review =
    { tip: x.tip, contract: x.contract, attempt: x.attempt, worktree: x.worktree }
  // Evidence is about the Goal tip, so it does not depend on the failed child's worktree.
  pure def goalFacts(x: State): Review = { ...facts(x), worktree: -1 }
  val now = facts(s)
  val open = not(s.blocked) and not(s.verified)
  val noRun = s.evidence != "pending" and s.evidence != "started"
  val done = s.integrated or s.evidence == "resolved"
  // The attempt is a stopped failure that nothing is about to resume.
  val settled = not(done) and not(s.running) and not(s.armed) and not(s.starting) and noRun
  val fresh = s.reviewed.tip == s.tip and s.reviewed.contract == s.contract

  // A preview fills the single slot and authorizes nothing.
  action previewAmendment = all { open,
    record({ ...s, kind: "amendment", reviewed: now }, "preview") }
  action previewResume = all { open, settled,
    record({ ...s, kind: "resume", reviewed: now }, "preview") }
  action previewEvidence = all { open, settled,
    record({ ...s, kind: "evidence", reviewed: now }, "preview") }

  // The proposal is never rewritten: an amendment only advances the contract revision.
  action approveAmendment = all { open, s.kind == "amendment", fresh,
    record({ ...s, kind: "none", reviewed: nothing, contract: s.contract + 1 }, "amend") }
  // Approval and the armed resume are one write.
  action approveResume = all { open, settled, s.kind == "resume", s.reviewed == now,
    record({ ...s, kind: "none", reviewed: nothing, armed: true, armedFor: now,
      arms: s.arms + 1, refused: false }, "arm") }
  action approveEvidence = all { open, settled, s.kind == "evidence", fresh,
    s.reviewed.attempt == s.attempt,
    record({ ...s, kind: "none", reviewed: nothing, evidence: "pending",
      evidenceFor: goalFacts(s), passed: 0 }, "requestEvidence") }

  action integrateSibling = all { open, s.siblings > 0,
    record({ ...s, tip: s.tip + 1, siblings: s.siblings - 1 }, "integrate") }
  action editWorktree = all { not(done), not(s.running), s.worktree < 2,
    record({ ...s, worktree: s.worktree + 1 }, "edit") }
  // A fresh attempt is planned with the contract as amended so far.
  action retry = all { open, settled, s.attempt < 3,
    record({ ...s, attempt: s.attempt + 1, worktree: 0, planContract: s.contract,
      refused: false }, "retry") }
  // A plain resume runs the attempt again without any correction.
  action plainResume = all { open, settled, not(s.refused), s.plain < 2,
    record({ ...s, plain: s.plain + 1 }, "plainResume") }

  // Launch delivers the correction only to exactly the reviewed state; otherwise the
  // approval is spent without a launch and has to be reviewed again.
  action dispatch = all { not(s.blocked), s.armed,
    if (s.armedFor == now)
      record({ ...s, armed: false, armedFor: nothing, starting: true, running: true,
        launches: s.launches + 1, delivered: s.delivered + 1 }, "launch")
    else record({ ...s, armed: false, armedFor: nothing, refused: true }, "refuse") }
  action childIntegrates = all { not(s.blocked), s.running,
    record({ ...s, running: false, starting: false, integrated: true, tip: s.tip + 1 },
      "childIntegrates") }
  action childFails = all { not(s.blocked), s.running,
    record({ ...s, running: false, starting: false }, "childFails") }
  // An armed correction is durable. A started launch or evidence run has an unknown
  // outcome after a restart: it is reported, never replayed.
  action restart = all { not(s.blocked),
    record({ ...s, running: false, blocked: s.starting,
      evidence: if (s.evidence == "started") "unresolved" else s.evidence }, "restart") }

  action startEvidence = all { not(s.blocked), s.evidence == "pending",
    record({ ...s, evidence: "started" }, "startEvidence") }
  // Each check validates the Goal ref against the approved tip around its run.
  action checkPasses = all { not(s.blocked), s.evidence == "started", s.passed < s.checks,
    s.evidenceFor.tip == s.tip, record({ ...s, passed: s.passed + 1 }, "checkPasses") }
  action checkFails = all { not(s.blocked), s.evidence == "started",
    record({ ...s, evidence: "unresolved" }, "checkFails") }
  // Every approved check passed. The result counts only for what was approved.
  action finishEvidence = all { not(s.blocked), s.evidence == "started",
    s.passed == s.checks,
    if (s.evidenceFor == goalFacts(s)) record({ ...s, evidence: "resolved" }, "resolve")
    else record({ ...s, evidence: "unresolved" }, "notAdopted") }

  action startDependent = all { open, done, not(s.dependents),
    record({ ...s, dependents: true }, "startDependent") }
  // The final verification runs every approved check at the final tip.
  action verify = all { open, done, s.dependents, noRun,
    record({ ...s, verified: true, verifiedTip: s.tip, verifiedContract: s.contract,
      verifiedChecks: s.checks }, "verify") }
  action idle = record(s, "idle")
  action step = any { previewAmendment, previewResume, previewEvidence, approveAmendment,
    approveResume, approveEvidence, integrateSibling, editWorktree, retry, plainResume,
    dispatch, childIntegrates, childFails, restart, startEvidence, checkPasses, checkFails,
    finishEvidence, startDependent, verify, idle }

  val previewAuthorizesNothing = event == "preview" implies
    s == { ...previous, kind: s.kind, reviewed: s.reviewed }
  val proposalAndChecksKept = s.original == initial.original and s.checks == initial.checks
  val amendedOnlyAsReviewed = s.contract != previous.contract implies all {
    event == "amend", s.contract == previous.contract + 1, previous.kind == "amendment",
    previous.reviewed.tip == previous.tip, previous.reviewed.contract == previous.contract }
  // An existing attempt keeps the contract revision its plan was written with.
  val attemptPlanKept = s.planContract != previous.planContract implies
    event == "retry" and s.planContract == s.contract
  // A correction reaches the child only by the launch its approval armed, as reviewed.
  val deliveredOnlyAsReviewed = s.delivered != previous.delivered implies all {
    event == "launch", previous.armed, previous.armedFor == facts(previous),
    s.delivered == previous.delivered + 1 }
  val launchedOncePerApproval = s.launches <= s.arms and s.delivered == s.launches
  // The worktree is outside Manager's control; nothing Manager records changes.
  val blockedIsFinal = previous.blocked implies
    { ...s, worktree: previous.worktree } == previous
  val startedNeverReplayed = (event == "restart" and previous.starting) implies
    s.blocked and s.launches == previous.launches
  val resolvedOnlyAsApproved =
    (s.evidence == "resolved" and previous.evidence != "resolved") implies all {
      event == "resolve", previous.evidence == "started",
      previous.passed == previous.checks, previous.evidenceFor == goalFacts(previous) }
  // An ended run never changes its outcome; only a new approval starts another.
  val unresolvedStays = previous.evidence == "unresolved" implies
    s.evidence == "unresolved" or event == "requestEvidence"
  val resolutionIsNotIntegration = not(s.evidence == "resolved" and s.integrated)
  val dependentsNeedCompletion = s.dependents implies done
  val verifiedAtFinalState = s.verified implies all { done, s.verifiedTip == s.tip,
    s.verifiedContract == s.contract, s.verifiedChecks == s.checks }
  val lifecycle = all {
    s.armed implies not(s.running) and not(done) and noRun,
    s.running implies s.starting and noRun,
    s.refused implies not(s.armed),
    s.kind == "none" iff s.reviewed == nothing,
  }
  val safety = all { previewAuthorizesNothing, proposalAndChecksKept, amendedOnlyAsReviewed,
    attemptPlanKept, deliveredOnlyAsReviewed, launchedOncePerApproval, blockedIsFinal,
    startedNeverReplayed, resolvedOnlyAsApproved, unresolvedStays,
    resolutionIsNotIntegration, dependentsNeedCompletion, verifiedAtFinalState, lifecycle }

  run correctedResumeLaunchesOnceTest = init.then(previewResume).then(approveResume)
    .then(dispatch).expect(s.launches == 1 and s.delivered == 1 and safety)
  run approvalIsSpentTest = init.then(previewResume).then(approveResume).then(approveResume)
    .fail()
  run launchIsNotRepeatedTest = init.then(previewResume).then(approveResume).then(dispatch)
    .then(dispatch).fail()
  run movedTipVoidsResumeTest = init.then(previewResume).then(integrateSibling)
    .then(approveResume).fail()
  run editedWorktreeVoidsResumeTest = init.then(previewResume).then(editWorktree)
    .then(approveResume).fail()
  run otherAttemptVoidsResumeTest = init.then(previewResume).then(retry).then(approveResume)
    .fail()
  run plainResumeDeliversNothingTest = init.then(previewResume).then(plainResume)
    .expect(s.delivered == 0 and s.kind == "resume" and safety)
  run changeBeforeLaunchIsRefusedTest = init.then(previewResume).then(approveResume)
    .then(editWorktree).then(dispatch)
    .expect(s.refused and s.launches == 0 and s.delivered == 0 and safety)
  run refusedCorrectionBlocksPlainResumeTest = init.then(previewResume).then(approveResume)
    .then(editWorktree).then(dispatch).then(plainResume).fail()
  run refusedCorrectionIsReviewedAgainTest = init.then(previewResume).then(approveResume)
    .then(editWorktree).then(dispatch).then(previewResume).then(approveResume).then(dispatch)
    .expect(s.launches == 1 and s.arms == 2 and not(s.refused) and safety)
  run armedCorrectionSurvivesRestartTest = init.then(previewResume).then(approveResume)
    .then(restart).then(dispatch).expect(s.launches == 1 and not(s.blocked) and safety)
  run startedCorrectionIsNotReplayedTest = init.then(previewResume).then(approveResume)
    .then(dispatch).then(restart).expect(s.blocked and s.launches == 1 and safety)
  run blockedGoalLaunchesNothingTest = init.then(previewResume).then(approveResume)
    .then(dispatch).then(restart).then(previewResume).fail()

  run amendmentKeepsProposalAndPlanTest = init.then(previewAmendment).then(approveAmendment)
    .expect(s.contract == 1 and s.original == 0 and s.planContract == 0 and safety)
  run amendmentIsApprovedOnceTest = init.then(previewAmendment).then(approveAmendment)
    .then(approveAmendment).fail()
  run movedTipVoidsAmendmentTest = init.then(previewAmendment).then(integrateSibling)
    .then(approveAmendment).fail()
  run laterAttemptUsesAmendmentTest = init.then(previewAmendment).then(approveAmendment)
    .then(retry).expect(s.planContract == 1 and safety)

  run evidenceResolvesAndFinalTipIsVerifiedTest = init.then(previewEvidence)
    .then(approveEvidence).then(startEvidence).then(checkPasses).then(checkPasses)
    .then(finishEvidence).then(startDependent).then(integrateSibling).then(verify)
    .expect(s.evidence == "resolved" and not(s.integrated) and s.verified
      and s.verifiedTip == 1 and s.verifiedChecks == 2 and safety)
  run partialEvidenceIsNotAResultTest = init.then(previewEvidence).then(approveEvidence)
    .then(startEvidence).then(checkPasses).then(finishEvidence).fail()
  run redEvidenceLeavesTaskFailedTest = init.then(previewEvidence).then(approveEvidence)
    .then(startEvidence).then(checkPasses).then(checkFails)
    .expect(s.evidence == "unresolved" and not(done) and safety)
  run unresolvedTaskBlocksDependentsTest = init.then(previewEvidence).then(approveEvidence)
    .then(startEvidence).then(checkFails).then(startDependent).fail()
  run interruptedEvidenceIsNotReplayedTest = init.then(previewEvidence).then(approveEvidence)
    .then(startEvidence).then(checkPasses).then(restart).then(checkPasses).fail()
  run movedTipEvidenceIsNotAdoptedTest = init.then(previewEvidence).then(approveEvidence)
    .then(startEvidence).then(checkPasses).then(checkPasses).then(integrateSibling)
    .then(finishEvidence).expect(s.evidence == "unresolved" and not(done) and safety)
  run amendedContractEvidenceIsNotAdoptedTest = init.then(previewEvidence)
    .then(approveEvidence).then(startEvidence).then(checkPasses).then(checkPasses)
    .then(previewAmendment).then(approveAmendment).then(finishEvidence)
    .expect(s.evidence == "unresolved" and safety)
  run movedTipVoidsEvidenceApprovalTest = init.then(previewEvidence).then(integrateSibling)
    .then(approveEvidence).fail()
  run otherAttemptVoidsEvidenceApprovalTest = init.then(previewEvidence).then(retry)
    .then(approveEvidence).fail()
}
