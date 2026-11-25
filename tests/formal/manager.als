// Bounded manager authority and execution abstraction, not a proof of Rust code.
module manager
open util/ordering[State]
open util/ordering[Preview] as revisions

sig Task { deps: set Task }
lone sig CorrectiveTask extends Task {}
sig Revision {}
sig Tip {}
// Each ordered atom represents both the identity and revision of a preview.
sig Preview { reviewedTip: one Tip }
abstract sig Phase {}
one sig Draft, Active, Paused, Blocked, Verifying, ReadyToPublish, AwaitingMerge, Completed extends Phase {}
abstract sig Event {}
one sig Init, Edit, Approve, Dispatch, Integrate, Resolve, Pause, Resume, Fail,
        BeginVerify, VerifyPass, VerifyFail, RetryVerify, RejectRetry,
        SaveCorrective, ApproveCorrective, RejectCorrective,
        Publish, ObserveMerge, ObserveOtherMerge, Idle extends Event {}

sig State {
  phase: one Phase,
  proposal: one Revision,
  approved: lone Revision,
  launched: set Task,
  integrated: set Task,
  resolved: set Task,
  tip: one Tip,
  verified: lone Tip,
  published: lone Tip,
  observed: lone Tip,
  failedCheck: lone State,
  verificationAudit: set State,
  preview: lone Preview,
  lastRevision: lone Preview,
  discarded: set Preview,
  correctiveApproval: lone Preview,
  requested: lone Preview,
  event: one Event
}
fact TaskGraph { no t: Task | t in t.^deps }
fun authorized[s: State]: set Task {
  Task - CorrectiveTask + (some s.correctiveApproval => CorrectiveTask else none)
}
// Dependents and verification wait for an integration or an evidence resolution.
fun done[s: State]: set Task { s.integrated + s.resolved }
pred initial[s: State] {
  s.phase = Draft and no s.approved + s.launched + done[s] + s.verified + s.published + s.observed
  no s.failedCheck + s.verificationAudit
  no s.preview + s.lastRevision + s.discarded + s.correctiveApproval + s.requested
  s.event = Init
}
pred unchanged[a,b: State] {
  b.proposal = a.proposal and b.approved = a.approved and
  b.launched = a.launched and b.integrated = a.integrated and b.resolved = a.resolved and
  b.tip = a.tip and b.verified = a.verified
}
pred edit[a,b: State] {
  a.phase = Draft and b.phase = Draft and b.proposal != a.proposal
  no b.approved + b.launched + done[b] + b.verified
  b.tip = a.tip and b.event = Edit
}
pred approve[a,b: State] {
  a.phase = Draft and b.phase = Active and b.proposal = a.proposal
  b.approved = a.proposal and no b.launched + done[b] + b.verified
  b.tip = a.tip and b.event = Approve
}
pred dispatch[a,b: State] {
  a.phase = Active and some t: authorized[a] - a.launched |
    t.deps in done[a] and b.launched = a.launched + t
  b.phase = a.phase and b.proposal = a.proposal and b.approved = a.approved
  b.integrated = a.integrated and b.resolved = a.resolved
  b.tip = a.tip and b.verified = a.verified
  b.event = Dispatch
}
pred integrate[a,b: State] {
  a.phase = Active and some t: a.launched - done[a] |
    b.integrated = a.integrated + t
  b.phase = a.phase and b.proposal = a.proposal and b.approved = a.approved
  b.launched = a.launched and b.resolved = a.resolved and b.tip != a.tip and no b.verified
  b.event = Integrate
}
// Approved evidence resolves a launched task that failed. Nothing is committed, so the
// tip stays, and the task is never counted as integrated.
pred resolve[a,b: State] {
  a.phase = Active and some t: a.launched - done[a] |
    b.resolved = a.resolved + t
  b.phase = a.phase and b.proposal = a.proposal and b.approved = a.approved
  b.launched = a.launched and b.integrated = a.integrated
  b.tip = a.tip and b.verified = a.verified
  b.event = Resolve
}
// Include a paused Goal with a saved preview to model the rejected-retry fixture.
pred pause[a,b: State] {
  (a.phase = Active or (a.phase = Blocked and some a.preview))
  b.phase = Paused and unchanged[a,b] and b.event = Pause
}
pred resume[a,b: State] {
  a.phase = Paused and b.phase = (some a.failedCheck => Blocked else Active)
  unchanged[a,b] and b.event = Resume
}
pred fail[a,b: State] { a.phase = Active and b.phase = Blocked and unchanged[a,b] and b.event = Fail }
pred beginVerify[a,b: State] {
  a.phase = Active and done[a] = authorized[a] and b.phase = Verifying
  unchanged[a,b] and b.event = BeginVerify
}
pred verifyPass[a,b: State] {
  a.phase = Verifying and b.phase = ReadyToPublish and b.verified = a.tip
  b.proposal = a.proposal and b.approved = a.approved and b.launched = a.launched
  b.integrated = a.integrated and b.resolved = a.resolved and b.tip = a.tip and b.event = VerifyPass
}
pred verifyFail[a,b: State] { a.phase = Verifying and b.phase = Blocked and unchanged[a,b] and b.event = VerifyFail }
pred eligibleForRetry[s: State] {
  s.phase = Blocked and done[s] = authorized[s] and some s.failedCheck
}
pred retryVerify[a,b: State] {
  eligibleForRetry[a] and b.phase = Verifying
  unchanged[a,b] and b.event = RetryVerify
}
pred rejectRetry[a,b: State] {
  not eligibleForRetry[a] and b.phase = a.phase
  unchanged[a,b] and b.event = RejectRetry
}
pred eligibleForCorrective[s: State] {
  eligibleForRetry[s] and some CorrectiveTask and no s.correctiveApproval
}
pred saveCorrective[a,b: State] {
  eligibleForCorrective[a] and b.phase = a.phase and unchanged[a,b]
  b.preview = (no a.lastRevision => revisions/first else revisions/next[a.lastRevision])
  some b.preview and b.preview.reviewedTip = a.tip
  b.event = SaveCorrective
}
pred mayApproveCorrective[s: State, p: Preview] {
  eligibleForCorrective[s] and p = s.preview and p.reviewedTip = s.tip
}
pred approveCorrective[a,b: State] {
  some p: Preview | b.requested = p and mayApproveCorrective[a,p]
  b.phase = Active and unchanged[a,b] and b.event = ApproveCorrective
}
pred rejectCorrective[a,b: State] {
  some p: Preview | b.requested = p and not mayApproveCorrective[a,p]
  b.phase = a.phase and unchanged[a,b] and b.event = RejectCorrective
}
pred publish[a,b: State] {
  a.phase = ReadyToPublish and a.verified = a.tip and b.phase = AwaitingMerge
  unchanged[a,b] and b.event = Publish
}
// Observation is external input, not an action performed by the manager.
pred observeMerge[a,b: State] {
  a.phase = AwaitingMerge and b.phase = Completed and b.observed = a.published
  some b.observed and unchanged[a,b] and b.event = ObserveMerge
}
pred observeOtherMerge[a,b: State] {
  a.phase = AwaitingMerge and b.phase = AwaitingMerge and some b.observed
  b.observed != a.published and unchanged[a,b] and b.event = ObserveOtherMerge
}
pred idle[a,b: State] { b.phase = a.phase and unchanged[a,b] and b.event = Idle }
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] |
    edit[a,b] or approve[a,b] or dispatch[a,b] or integrate[a,b] or resolve[a,b] or
    pause[a,b] or resume[a,b] or fail[a,b] or beginVerify[a,b] or
    verifyPass[a,b] or verifyFail[a,b] or retryVerify[a,b] or rejectRetry[a,b] or
    saveCorrective[a,b] or approveCorrective[a,b] or rejectCorrective[a,b] or publish[a,b] or
    observeMerge[a,b] or observeOtherMerge[a,b] or idle[a,b]
  all a: State - last | let b = next[a] | {
    b.published = (b.event = Publish => a.tip else a.published)
    b.event not in ObserveMerge + ObserveOtherMerge implies b.observed = a.observed
    b.event not in ApproveCorrective + RejectCorrective implies no b.requested
    b.event not in SaveCorrective + ApproveCorrective + RetryVerify implies b.preview = a.preview
    b.event in ApproveCorrective + RetryVerify implies no b.preview
    b.lastRevision = (b.event = SaveCorrective => b.preview else a.lastRevision)
    b.discarded = (b.event = RetryVerify => a.discarded + a.preview else a.discarded)
    b.correctiveApproval = (b.event = ApproveCorrective => b.requested else a.correctiveApproval)
    b.event in ApproveCorrective + RetryVerify implies {
      no b.failedCheck
      b.verificationAudit = a.verificationAudit + a.failedCheck
    } else {
      b.failedCheck = (b.event = VerifyFail => b else a.failedCheck)
      b.verificationAudit = a.verificationAudit
    }
  }
}
assert ApprovalGatesExecution {
  all s: State | (some s.launched or s.phase in Verifying + ReadyToPublish + AwaitingMerge + Completed)
    implies s.approved = s.proposal
}
assert DependenciesBeforeDispatch {
  all a: State - last | let b = next[a] | b.event = Dispatch implies
    (b.launched - a.launched).deps in done[a]
}
assert IntegratedWasLaunched { all s: State | done[s] in s.launched }
assert EvidenceIsNotIntegration {
  all s: State | no s.integrated & s.resolved
  all a: State - last | let b = next[a] | b.event = Resolve implies {
    b.tip = a.tip and b.integrated = a.integrated and b.launched = a.launched
    a.resolved in b.resolved and b.resolved != a.resolved
  }
}
assert VerificationGatesPublication {
  all s: State | s.phase in ReadyToPublish + AwaitingMerge + Completed implies
    done[s] = authorized[s] and s.verified = s.tip
}
assert CompletionRequiresMatchingHumanMerge {
  all s: State | s.phase = Completed implies
    some s.published and s.published = s.tip and s.observed = s.published
}
assert PublicationDoesNotComplete {
  all s: State | s.event = Publish implies s.phase = AwaitingMerge
}
assert PausedDoesNotAdvanceWork {
  all a: State - last | a.phase = Paused implies
    next[a].launched = a.launched and done[next[a]] = done[a]
}
assert RetryArchivesAndClearsPreview {
  all a: State - last | let b = next[a] | b.event = RetryVerify implies {
    b.tip = a.tip and no b.preview + b.failedCheck
    b.discarded = a.discarded + a.preview
    b.verificationAudit = a.verificationAudit + a.failedCheck
    b.lastRevision = a.lastRevision and authorized[b] = authorized[a]
  }
}
assert DiscardedPreviewNeverAuthorizes {
  all s: State | {
    no s.discarded & (s.preview + s.correctiveApproval)
    all p: s.discarded | not mayApproveCorrective[s,p]
    some s.preview implies s.discarded in revisions/prevs[s.preview]
    s.launched in authorized[s]
  }
  all a: State - last | let b = next[a] | {
    a.discarded in b.discarded and a.verificationAudit in b.verificationAudit
    b.event = ApproveCorrective implies {
      b.requested = a.preview and a.preview.reviewedTip = a.tip
      b.correctiveApproval = b.requested and CorrectiveTask in authorized[b]
    }
  }
}
assert RejectedRetryLeavesGoalUnchanged {
  all a: State - last | let b = next[a] | b.event = RejectRetry implies {
    not eligibleForRetry[a]
    unchanged[a,b] and b.phase = a.phase and b.published = a.published and b.observed = a.observed
    b.preview = a.preview and b.lastRevision = a.lastRevision and b.discarded = a.discarded
    b.correctiveApproval = a.correctiveApproval
    b.failedCheck = a.failedCheck and b.verificationAudit = a.verificationAudit
  }
}
pred SuccessfulGoal { some s: State | s.phase = Completed }
pred DependencyExecution {
  some disj x,y: Task | x in y.deps and some a,b: State |
    a.event = Integrate and x in a.integrated and b in a.^next and
    b.event = Dispatch and y in b.launched
}
pred EvidenceUnblocksDependent {
  some disj x,y: Task | x in y.deps and some a,b: State |
    a.event = Resolve and x in a.resolved and b in a.^next and
    b.event = Dispatch and y in b.launched
}
pred FailedVerificationRetry {
  some a,b: State | a.event = VerifyFail and b in a.^next and b.event = RetryVerify
}
pred PausedThenResumed { some a,b: State | a.event = Pause and b in a.^next and b.event = Resume }
pred OtherMergeDoesNotComplete { some s: State | s.event = ObserveOtherMerge and s.phase = AwaitingMerge }
pred sameTipFailureAfterRetry[saved, retried, failedAgain: State] {
  saved.event = SaveCorrective and retried = next[saved] and retried.event = RetryVerify
  failedAgain = next[retried] and failedAgain.event = VerifyFail
  saved.tip = failedAgain.tip and no failedAgain.preview
  saved.preview in failedAgain.discarded and saved.failedCheck in failedAgain.verificationAudit
}
pred SameTipRejectsDiscardedPreview {
  some saved, retried, failedAgain, rejected: State | {
    sameTipFailureAfterRetry[saved,retried,failedAgain]
    rejected = next[failedAgain] and rejected.event = RejectCorrective
    rejected.requested = saved.preview and no rejected.correctiveApproval
  }
}
pred SameTipApprovesFreshPreview {
  some saved, retried, failedAgain, fresh, rejected, approved: State | {
    sameTipFailureAfterRetry[saved,retried,failedAgain]
    fresh = next[failedAgain] and fresh.event = SaveCorrective and fresh.tip = saved.tip
    fresh.preview = revisions/next[saved.preview]
    rejected = next[fresh] and rejected.event = RejectCorrective
    rejected.requested = saved.preview and rejected.preview = fresh.preview
    approved = next[rejected] and approved.event = ApproveCorrective
    approved.correctiveApproval = fresh.preview and CorrectiveTask in authorized[approved]
    saved.preview in approved.discarded
    saved.failedCheck + failedAgain in approved.verificationAudit
  }
}
pred RejectedRetryRetainsLivePreview {
  some paused: State - last | let rejected = next[paused] | {
    paused.phase = Paused and some paused.preview
    rejected.event = RejectRetry and rejected.preview = paused.preview
    rejected.discarded = paused.discarded and rejected.verificationAudit = paused.verificationAudit
  }
}
check ApprovalGatesExecution for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 0
check DependenciesBeforeDispatch for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 0
check IntegratedWasLaunched for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 0
check VerificationGatesPublication for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 0
check CompletionRequiresMatchingHumanMerge for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 0
check PublicationDoesNotComplete for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 0
check PausedDoesNotAdvanceWork for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 0
check EvidenceIsNotIntegration for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 0
check RetryArchivesAndClearsPreview for exactly 12 State, exactly 2 Task, exactly 1 CorrectiveTask, exactly 2 Preview, exactly 2 Revision, exactly 3 Tip expect 0
check DiscardedPreviewNeverAuthorizes for exactly 12 State, exactly 2 Task, exactly 1 CorrectiveTask, exactly 2 Preview, exactly 2 Revision, exactly 3 Tip expect 0
check RejectedRetryLeavesGoalUnchanged for exactly 12 State, exactly 2 Task, exactly 1 CorrectiveTask, exactly 2 Preview, exactly 2 Revision, exactly 3 Tip expect 0
run SuccessfulGoal for exactly 11 State, exactly 2 Task, exactly 0 CorrectiveTask, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 1
run DependencyExecution for exactly 10 State, exactly 2 Task, exactly 0 CorrectiveTask, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 1
run EvidenceUnblocksDependent for exactly 10 State, exactly 2 Task, exactly 0 CorrectiveTask, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 1
run FailedVerificationRetry for exactly 10 State, exactly 2 Task, exactly 0 CorrectiveTask, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 1
run PausedThenResumed for exactly 10 State, exactly 2 Task, exactly 0 CorrectiveTask, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 1
run OtherMergeDoesNotComplete for exactly 10 State, exactly 2 Task, exactly 0 CorrectiveTask, exactly 2 Revision, exactly 3 Tip, exactly 1 Preview expect 1
run SameTipRejectsDiscardedPreview for exactly 10 State, exactly 2 Task, exactly 1 CorrectiveTask, exactly 2 Preview, exactly 2 Revision, exactly 3 Tip expect 1
run SameTipApprovesFreshPreview for exactly 12 State, exactly 2 Task, exactly 1 CorrectiveTask, exactly 2 Preview, exactly 2 Revision, exactly 3 Tip expect 1
run RejectedRetryRetainsLivePreview for exactly 9 State, exactly 2 Task, exactly 1 CorrectiveTask, exactly 2 Preview, exactly 2 Revision, exactly 3 Tip expect 1
