// Finite lifecycle abstraction. Persistence is atomic; interruption before
// marking and a mismatch are modeled. Cleanup failure leaves resources intact. The
// environment is fixed for the entire trace, so concurrent checkout changes
// are not modeled. Finalization, squash and cleanup are abstract operations,
// not a proof about individual Git commands or content preservation.
module local_merge
open util/ordering[State]
abstract sig Phase {}
one sig Eligible, Finalized, Squashed, Staged, Intended, Committed, Mismatch, MergedMarked, Cleaned, Refused, Restored extends Phase {}
abstract sig Event {}
one sig Init, Refuse, Finalize, Squash, Stage, Restore, Intent, IntentFail, Commit, Interrupt, RejectMismatch, Mark, CleanupFail, Cleanup, Delete, Idle extends Event {}
abstract sig Bit {}
one sig Yes, No extends Bit {}
sig State {
  phase: one Phase, event: one Event,
  dirty, behind, matched, staged, wt, branch, session, marker: one Bit,
  commits: one Int
}
pred initial[a: State] {
  a.phase = Eligible and a.event = Init and a.staged = No and a.commits = 0
  and a.wt = Yes and a.branch = Yes and a.session = Yes and a.marker = No
}
pred sameEnv[a,b: State] { b.dirty = a.dirty and b.behind = a.behind and b.matched = a.matched }
pred sameResources[a,b: State] { b.wt = a.wt and b.branch = a.branch and b.session = a.session }
pred sameData[a,b: State] { b.staged = a.staged and b.commits = a.commits and b.marker = a.marker and sameResources[a,b] }
pred transition[a,b: State] {
  sameEnv[a,b]
  {
  (b.event = Refuse and a.phase = Eligible and (a.dirty = Yes or a.behind = Yes or a.matched = No)
    and b.phase = Refused and sameData[a,b])
  or (b.event = Finalize and a.phase = Eligible and a.dirty = No and a.behind = No and a.matched = Yes
    and b.phase = Finalized and sameData[a,b])
  or (b.event = Squash and a.phase = Finalized and b.phase = Squashed and b.staged = Yes
    and b.commits = a.commits and b.marker = a.marker and sameResources[a,b])
  or (b.event = Restore and a.phase = Squashed and b.phase = Restored and b.staged = No
    and b.commits = a.commits and b.marker = a.marker and sameResources[a,b])
  or (b.event = Stage and a.phase = Squashed and b.phase = Staged and sameData[a,b])
  or (b.event = Intent and a.phase = Squashed and b.phase = Intended and sameData[a,b])
  or (b.event = IntentFail and a.phase = Squashed and b.phase = Restored and b.staged = No
    and b.commits = a.commits and b.marker = a.marker and sameResources[a,b])
  or (b.event = Commit and a.phase = Intended and a.marker = No and b.phase = Committed
    and b.staged = No and b.commits = add[a.commits,1] and b.marker = a.marker and sameResources[a,b])
  or (b.event = Interrupt and a.phase = Committed and b.phase = Committed and sameData[a,b])
  or (b.event = RejectMismatch and a.phase = Committed and b.phase = Mismatch and sameData[a,b])
  or (b.event = Mark and a.phase = Committed and b.phase = MergedMarked and b.marker = Yes
    and b.staged = a.staged and b.commits = a.commits and sameResources[a,b])
  or (b.event = CleanupFail and a.phase = MergedMarked and b.phase = a.phase and sameData[a,b])
  or (b.event = Cleanup and a.phase = MergedMarked and a.marker = Yes and b.phase = Cleaned
    and b.wt = No and b.branch = No and b.session = a.session and b.marker = a.marker
    and b.staged = a.staged and b.commits = a.commits)
  or (b.event = Delete and a.phase = Cleaned and b.phase = Cleaned and b.session = No
    and b.wt = a.wt and b.branch = a.branch and b.marker = a.marker
    and b.staged = a.staged and b.commits = a.commits)
  or (b.event = Idle and b.phase = a.phase and sameData[a,b])
  }
}
fact Trace { initial[first] and all a: State - last | transition[a,next[a]] }
assert RefusalAndRollback { all s: State | s.phase in Refused + Restored implies s.staged = No and s.commits = 0 }
assert AtMostOneCommit { all s: State | s.commits >= 0 and s.commits <= 1 }
assert CleanupAfterMarker { all s: State | (s.wt = No or s.branch = No) implies s.marker = Yes }
assert MismatchPreservesResources { all s: State | s.phase = Mismatch implies s.marker = No and s.wt = Yes and s.branch = Yes }
assert DeleteAfterCleanup { all s: State | s.session = No implies s.marker = Yes and s.wt = No and s.branch = No }
assert SafeCommit { all s: State | s.commits > 0 implies s.dirty = No and s.behind = No and s.matched = Yes }
assert CleanupFailurePreservesState {
  all a: State - last | next[a].event = CleanupFail implies
    next[a].phase = a.phase and sameData[a,next[a]]
}
assert StageRetainsSession {
  all s: State | s.phase = Staged implies s.staged = Yes and s.commits = 0
    and s.marker = No and s.wt = Yes and s.branch = Yes and s.session = Yes
}
assert RefusalRetainsResources {
  all s: State | s.phase = Refused implies s.wt = Yes and s.branch = Yes
    and s.session = Yes and s.marker = No
}
pred RetryWitness {
  some a: State - last | next[a].event = CleanupFail and
    some b: next[a].^next | b.event = Cleanup and b.commits = 1 and
      some c: b.^next | c.event = Delete and c.commits = 1 and c.session = No
}
pred StageWitness { some s: State | s.phase = Staged and s.staged = Yes and s.commits = 0 and s.session = Yes }
pred RestoreWitness { some s: State | s.phase = Restored }
pred RefuseWitness { some s: State | s.phase = Refused }
pred BehindRefuseWitness { some s: State | s.phase = Refused and s.behind = Yes and s.dirty = No and s.matched = Yes }
pred MismatchRefuseWitness { some s: State | s.phase = Refused and s.matched = No and s.dirty = No and s.behind = No }
pred DirtyRefuseWitness { some s: State | s.phase = Refused and s.dirty = Yes and s.behind = No and s.matched = Yes }
check RefusalAndRollback for 20 but exactly 8 State, 4 Int expect 0
check AtMostOneCommit for 20 but exactly 8 State, 4 Int expect 0
check CleanupAfterMarker for 20 but exactly 8 State, 4 Int expect 0
check DeleteAfterCleanup for 20 but exactly 8 State, 4 Int expect 0
check SafeCommit for 20 but exactly 8 State, 4 Int expect 0
check CleanupFailurePreservesState for 20 but exactly 8 State, 4 Int expect 0
check StageRetainsSession for 20 but exactly 8 State, 4 Int expect 0
check RefusalRetainsResources for 20 but exactly 8 State, 4 Int expect 0
run RetryWitness for 20 but exactly 9 State, 4 Int expect 1
run StageWitness for 20 but exactly 8 State, 4 Int expect 1
run RestoreWitness for 20 but exactly 8 State, 4 Int expect 1
run RefuseWitness for 20 but exactly 8 State, 4 Int expect 1
run BehindRefuseWitness for 20 but exactly 8 State, 4 Int expect 1
run MismatchRefuseWitness for 20 but exactly 8 State, 4 Int expect 1
run DirtyRefuseWitness for 20 but exactly 8 State, 4 Int expect 1
check MismatchPreservesResources for 20 but exactly 8 State, 4 Int expect 0
pred InterruptWitness { some a: State - last | next[a].event = Interrupt and some b: next[a].^next | b.event = Mark }
pred MismatchWitness { some s: State | s.phase = Mismatch }
run InterruptWitness for 20 but exactly 8 State, 4 Int expect 1
run MismatchWitness for 20 but exactly 8 State, 4 Int expect 1
