// Publication lifecycle abstraction. PRs are open PRs on one branch.
// A failed persistence after creation retains the remote PR and its URL.
module pr_failed {
  type State = { phase: str, prUrl: bool, worktreeExists: bool,
    prsOnBranch: int, skipReason: str, workflowRuns: int, lastAction: str,
    beforeWorkflowRuns: int, deleted: bool }
  var s: State
  pure def isRunnable(phase: str): bool =
    Set("Planned", "Running", "Failed", "Suspended").contains(phase)
  action init = s' = { phase: "Planned", prUrl: false, worktreeExists: true,
    prsOnBranch: 0, skipReason: "none", workflowRuns: 0, lastAction: "init",
    beforeWorkflowRuns: 0, deleted: false }
  action runWorkflowOk = all {
    not(s.deleted), isRunnable(s.phase), s.phase != "Running", s.worktreeExists,
    s' = { ...s, phase: "Running", workflowRuns: s.workflowRuns + 1,
      beforeWorkflowRuns: s.workflowRuns, lastAction: "workflow" } }
  action runWorkflowErr = all {
    not(s.deleted), s.phase == "Running",
    s' = { ...s, phase: "Failed", lastAction: "workflowError" } }
  action publishOk = all {
    not(s.deleted), s.phase == "Running",
    s' = { ...s, phase: "Completed", prUrl: true, prsOnBranch: 1,
      skipReason: "none", lastAction: "publishOk" } }
  action publishNoCommits = all {
    not(s.deleted), s.phase == "Running",
    s' = { ...s, phase: "Completed", skipReason: "NoCommits", lastAction: "publishSkip" } }
  action publishErr(created: bool): bool = all {
    not(s.deleted), s.phase == "Running",
    s' = { ...s, phase: "PrFailed", prUrl: s.prUrl or created,
      prsOnBranch: if (created) 1 else s.prsOnBranch, lastAction: "publishErr" } }
  action createPrOk = all {
    not(s.deleted), s.phase == "PrFailed",
    s' = { ...s, phase: "Completed", prUrl: true, prsOnBranch: 1,
      skipReason: "none", beforeWorkflowRuns: s.workflowRuns, lastAction: "createPrOk" } }
  action createPrNoCommits = all {
    not(s.deleted), s.phase == "PrFailed",
    s' = { ...s, phase: "Completed", skipReason: "NoCommits",
      beforeWorkflowRuns: s.workflowRuns, lastAction: "createPrSkip" } }
  action createPrErr(created: bool): bool = all {
    not(s.deleted), s.phase == "PrFailed",
    s' = { ...s, prUrl: s.prUrl or created,
      prsOnBranch: if (created) 1 else s.prsOnBranch,
      beforeWorkflowRuns: s.workflowRuns, lastAction: "createPrErr" } }
  action createPrCancel = all {
    not(s.deleted), s.phase == "PrFailed",
    s' = { ...s, beforeWorkflowRuns: s.workflowRuns, lastAction: "createPrCancel" } }
  action resetToPlanned = all {
    not(s.deleted), s.phase == "PrFailed",
    s' = { ...s, phase: "Planned", prUrl: false, skipReason: "none", lastAction: "reset" } }
  action deleteSession = all {
    not(s.deleted), s.phase == "PrFailed",
    s' = { ...s, deleted: true, phase: "Planned", worktreeExists: false, lastAction: "delete" } }
  action cleanupAfterPr = all {
    not(s.deleted), s.phase == "Completed", s.prUrl, s.worktreeExists,
    s' = { ...s, worktreeExists: false, lastAction: "cleanup" } }
  action idle = s' = s
  action step = any { runWorkflowOk, runWorkflowErr, publishOk, publishNoCommits,
    publishErr(false), publishErr(true), createPrOk, createPrNoCommits,
    createPrErr(false), createPrErr(true), createPrCancel, resetToPlanned,
    deleteSession, cleanupAfterPr, idle }

  val PrFailedNotRunnable = s.phase == "PrFailed" implies not(isRunnable(s.phase))
  val CreatePrNeverRunsWorkflow = Set("createPrOk", "createPrSkip", "createPrErr", "createPrCancel").contains(s.lastAction)
    implies (s.phase != "Running" and s.workflowRuns == s.beforeWorkflowRuns)
  val CancelKeepsPrFailed = s.lastAction == "createPrCancel" implies s.phase == "PrFailed"
  val AtMostOnePrPerBranch = s.prsOnBranch >= 0 and s.prsOnBranch <= 1
  val UrlRefersToExistingPr = s.prUrl implies s.prsOnBranch == 1
  val WorktreeRetainedWhilePrFailed = s.phase == "PrFailed" implies s.worktreeExists
  val CompletedHasPrOrSkip = s.phase == "Completed" implies (s.prUrl or s.skipReason == "NoCommits")
  val safety = PrFailedNotRunnable and CreatePrNeverRunsWorkflow and CancelKeepsPrFailed
    and AtMostOnePrPerBranch and UrlRefersToExistingPr
    and WorktreeRetainedWhilePrFailed and CompletedHasPrOrSkip

  run retryTest = init.then(runWorkflowOk).then(publishErr(false)).then(createPrErr(false))
    .then(createPrCancel).then(createPrOk).expect(s.phase == "Completed" and s.workflowRuns == 1 and safety)
  run persistFailureTest = init.then(runWorkflowOk).then(publishErr(true)).then(createPrOk)
    .expect(s.prUrl and s.prsOnBranch == 1 and s.workflowRuns == 1 and safety)
  run retryPersistFailureTest = init.then(runWorkflowOk).then(publishErr(false)).then(createPrErr(true))
    .then(createPrOk).expect(s.prUrl and s.prsOnBranch == 1 and safety)
  run repeatedRetryFailureTest = init.then(runWorkflowOk).then(publishErr(true))
    .then(createPrErr(false)).then(createPrCancel).then(createPrErr(true))
    .expect(s.phase == "PrFailed" and s.prUrl and s.prsOnBranch == 1
      and s.worktreeExists and s.workflowRuns == 1 and safety)
  run resetAndRepublishTest = init.then(runWorkflowOk).then(publishErr(true))
    .then(resetToPlanned).then(runWorkflowOk).then(publishOk)
    .expect(s.phase == "Completed" and s.prsOnBranch == 1 and safety)
  run skipTest = init.then(runWorkflowOk).then(publishErr(false)).then(createPrNoCommits)
    .expect(s.phase == "Completed" and s.skipReason == "NoCommits" and safety)
  run workflowFailureTest = init.then(runWorkflowOk).then(runWorkflowErr)
    .expect(s.phase == "Failed" and safety)
  run resetTest = init.then(runWorkflowOk).then(publishErr(false)).then(resetToPlanned)
    .expect(s.phase == "Planned" and s.worktreeExists and safety)
  run deleteTest = init.then(runWorkflowOk).then(publishErr(false)).then(deleteSession)
    .expect(s.deleted and not(s.worktreeExists) and safety)
  run cleanupTest = init.then(runWorkflowOk).then(publishOk).then(cleanupAfterPr)
    .expect(not(s.worktreeExists) and safety)
}
