// Bounded manager authority and execution abstraction, not a proof of Rust code.
module manager
open util/ordering[State]

sig Task { deps: set Task }
sig Revision {}
sig Tip {}
abstract sig Phase {}
one sig Draft, Active, Paused, Blocked, Verifying, ReadyToPublish, AwaitingMerge, Completed extends Phase {}
abstract sig Event {}
one sig Init, Edit, Approve, Dispatch, Integrate, Pause, Resume, Fail,
        BeginVerify, VerifyPass, VerifyFail, RetryVerify, Publish, ObserveMerge, ObserveOtherMerge, Idle extends Event {}

sig State {
  phase: one Phase,
  proposal: one Revision,
  approved: lone Revision,
  launched: set Task,
  integrated: set Task,
  tip: one Tip,
  verified: lone Tip,
  published: lone Tip,
  observed: lone Tip,
  event: one Event
}
fact TaskGraph { no t: Task | t in t.^deps }
pred initial[s: State] {
  s.phase = Draft and no s.approved + s.launched + s.integrated + s.verified + s.published + s.observed
  s.event = Init
}
pred unchanged[a,b: State] {
  b.proposal = a.proposal and b.approved = a.approved and
  b.launched = a.launched and b.integrated = a.integrated and
  b.tip = a.tip and b.verified = a.verified
}
pred edit[a,b: State] {
  a.phase = Draft and b.phase = Draft and b.proposal != a.proposal
  no b.approved + b.launched + b.integrated + b.verified
  b.tip = a.tip and b.event = Edit
}
pred approve[a,b: State] {
  a.phase = Draft and b.phase = Active and b.proposal = a.proposal
  b.approved = a.proposal and no b.launched + b.integrated + b.verified
  b.tip = a.tip and b.event = Approve
}
pred dispatch[a,b: State] {
  a.phase = Active and some t: Task - a.launched |
    t.deps in a.integrated and b.launched = a.launched + t
  b.phase = a.phase and b.proposal = a.proposal and b.approved = a.approved
  b.integrated = a.integrated and b.tip = a.tip and b.verified = a.verified
  b.event = Dispatch
}
pred integrate[a,b: State] {
  a.phase = Active and some t: a.launched - a.integrated |
    b.integrated = a.integrated + t
  b.phase = a.phase and b.proposal = a.proposal and b.approved = a.approved
  b.launched = a.launched and b.tip != a.tip and no b.verified
  b.event = Integrate
}
pred pause[a,b: State] { a.phase = Active and b.phase = Paused and unchanged[a,b] and b.event = Pause }
pred resume[a,b: State] { a.phase = Paused and b.phase = Active and unchanged[a,b] and b.event = Resume }
pred fail[a,b: State] { a.phase = Active and b.phase = Blocked and unchanged[a,b] and b.event = Fail }
pred beginVerify[a,b: State] {
  a.phase = Active and a.integrated = Task and b.phase = Verifying
  unchanged[a,b] and b.event = BeginVerify
}
pred verifyPass[a,b: State] {
  a.phase = Verifying and b.phase = ReadyToPublish and b.verified = a.tip
  b.proposal = a.proposal and b.approved = a.approved and b.launched = a.launched
  b.integrated = a.integrated and b.tip = a.tip and b.event = VerifyPass
}
pred verifyFail[a,b: State] { a.phase = Verifying and b.phase = Blocked and unchanged[a,b] and b.event = VerifyFail }
pred retryVerify[a,b: State] {
  a.phase = Blocked and a.integrated = Task and b.phase = Verifying
  unchanged[a,b] and b.event = RetryVerify
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
    edit[a,b] or approve[a,b] or dispatch[a,b] or integrate[a,b] or
    pause[a,b] or resume[a,b] or fail[a,b] or beginVerify[a,b] or
    verifyPass[a,b] or verifyFail[a,b] or retryVerify[a,b] or publish[a,b] or
    observeMerge[a,b] or observeOtherMerge[a,b] or idle[a,b]
  all a: State - last | let b = next[a] | {
    b.published = (b.event = Publish => a.tip else a.published)
    b.event not in ObserveMerge + ObserveOtherMerge implies b.observed = a.observed
  }
}
assert ApprovalGatesExecution {
  all s: State | (some s.launched or s.phase in Verifying + ReadyToPublish + AwaitingMerge + Completed)
    implies s.approved = s.proposal
}
assert DependenciesBeforeDispatch {
  all a: State - last | let b = next[a] | b.event = Dispatch implies
    (b.launched - a.launched).deps in a.integrated
}
assert IntegratedWasLaunched { all s: State | s.integrated in s.launched }
assert VerificationGatesPublication {
  all s: State | s.phase in ReadyToPublish + AwaitingMerge + Completed implies
    s.integrated = Task and s.verified = s.tip
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
    next[a].launched = a.launched and next[a].integrated = a.integrated
}
pred SuccessfulGoal { some s: State | s.phase = Completed }
pred DependencyExecution {
  some disj x,y: Task | x in y.deps and some a,b: State |
    a.event = Integrate and x in a.integrated and b in a.^next and
    b.event = Dispatch and y in b.launched
}
pred FailedVerificationRetry {
  some a,b: State | a.event = VerifyFail and b in a.^next and b.event = RetryVerify
}
pred PausedThenResumed { some a,b: State | a.event = Pause and b in a.^next and b.event = Resume }
pred OtherMergeDoesNotComplete { some s: State | s.event = ObserveOtherMerge and s.phase = AwaitingMerge }
check ApprovalGatesExecution for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 0
check DependenciesBeforeDispatch for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 0
check IntegratedWasLaunched for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 0
check VerificationGatesPublication for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 0
check CompletionRequiresMatchingHumanMerge for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 0
check PublicationDoesNotComplete for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 0
check PausedDoesNotAdvanceWork for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 0
run SuccessfulGoal for exactly 11 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 1
run DependencyExecution for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 1
run FailedVerificationRetry for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 1
run PausedThenResumed for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 1
run OtherMergeDoesNotComplete for exactly 10 State, exactly 2 Task, exactly 2 Revision, exactly 3 Tip expect 1
