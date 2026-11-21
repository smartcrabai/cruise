module exec_recovery

// Bounded contract for accepted exec, independent of which entry point submits it.
// Three sequential graph nodes distinguish a successful prefix, first failure,
// and a later effect. Budget and runtime are abstract checkpoint values.
abstract sig Node { next: lone Node }
one sig Entry, Middle, Tail extends Node {}
fact Route { next = Entry->Middle + Middle->Tail }
abstract sig Phase {}
one sig Ready, Running, Failed, Suspended, Completed, Removed extends Phase {}
abstract sig Value {}
one sig BeforeEntry, BeforeMiddle, BeforeTail extends Value {}
fun prefix[n: Node]: one Value {
  n = Entry => BeforeEntry else n = Middle => BeforeMiddle else BeforeTail
}

// The singleton represents one original session ID, never replaced during retry.
one sig Session {
  var phase: one Phase,
  var cursor: lone Node,
  var anchor: lone Node,
  var snapshot: lone Value,
  var runtime: one Value,
  var checkpoint: one Checkpoint,
  var effects: set Node
}
abstract sig Checkpoint {}
one sig Unstarted, Live, Exhausted, Unreliable extends Checkpoint {}

pred init {
  Session.phase = Ready
  Session.checkpoint = Unstarted
  no Session.cursor + Session.anchor
  no Session.snapshot
  Session.runtime = BeforeEntry
  no Session.effects
}
pred stableGraphAndEffects {
  Session.cursor' = Session.cursor
  Session.anchor' = Session.anchor
  Session.snapshot' = Session.snapshot
  Session.runtime' = Session.runtime
  Session.effects' = Session.effects
}
pred observe {
  Session.phase' = Session.phase
  stableGraphAndEffects
  Session.checkpoint' = Session.checkpoint
}
pred setupFailure {
  Session.phase in Ready + Failed
  Session.phase' = Failed
  stableGraphAndEffects
  Session.checkpoint' = Session.checkpoint
}
pred resume {
  Session.phase in Ready + Failed + Suspended
  Session.checkpoint != Unreliable
  (Session.phase = Failed and Session.checkpoint = Exhausted) implies some Session.anchor
  Session.phase' = Running
  Session.effects' = Session.effects
  let rewind = (Session.phase = Failed or Session.checkpoint = Exhausted) => Session.anchor else none |
    some rewind implies {
      Session.checkpoint' = Live
      Session.cursor' = rewind
      Session.runtime' = Session.snapshot
      no Session.anchor' + Session.snapshot'
    } else {
      Session.checkpoint' = (Session.checkpoint = Unstarted => Live else Session.checkpoint)
      Session.cursor' = (Session.checkpoint = Unstarted => Entry else Session.cursor)
      Session.runtime' = Session.runtime
      Session.anchor' = Session.anchor
      Session.snapshot' = Session.snapshot
    }
}
pred execute[unhandled: Int] {
  Session.phase = Running
  Session.checkpoint = Live
  some Session.cursor
  Session.phase' = Running
  Session.cursor' = Session.cursor.next
  Session.checkpoint' = (some Session.cursor' => Live else Exhausted)
  Session.runtime' = (some Session.cursor' => prefix[Session.cursor'] else Session.runtime)
  Session.effects' = Session.effects + Session.cursor
  (unhandled = 1 and no Session.anchor) implies {
    Session.anchor' = Session.cursor
    Session.snapshot' = Session.runtime
  } else {
    Session.anchor' = Session.anchor
    Session.snapshot' = Session.snapshot
  }
}
pred hardFailure {
  Session.phase = Running
  Session.checkpoint = Live
  Session.phase' = Failed
  stableGraphAndEffects
  Session.checkpoint' = Session.checkpoint
}
pred settle {
  Session.phase = Running
  Session.checkpoint = Exhausted
  Session.phase' = (some Session.anchor => Failed else Completed)
  stableGraphAndEffects
  Session.checkpoint' = Session.checkpoint
}
pred interrupt {
  Session.phase = Running
  Session.phase' = Suspended
  stableGraphAndEffects
  Session.checkpoint' = Session.checkpoint
}
pred loseCheckpoint {
  Session.phase in Failed + Suspended
  Session.checkpoint != Unstarted
  Session.phase' = Session.phase
  Session.checkpoint' = Unreliable
  stableGraphAndEffects
}
pred dispose {
  Session.phase = Completed
  Session.phase' = Removed
  stableGraphAndEffects
  Session.checkpoint' = Session.checkpoint
}
pred explicitDelete {
  Session.phase = Failed
  Session.phase' = Removed
  stableGraphAndEffects
  Session.checkpoint' = Session.checkpoint
}
// Clean has no transition for Failed; the opt-in list is separate from inventory
// used by default selection and batch runs.
abstract sig Inventory {}
one sig OptedInCliList, DefaultList, RunPicker, BatchRun extends Inventory {}
pred listed[inventory: Inventory] {
  Session.phase = Failed and inventory = OptedInCliList
}

fact Behavior {
  init
  always (observe or setupFailure or resume or execute[0] or execute[1] or
    hardFailure or settle or interrupt or loseCheckpoint or dispose or explicitDelete)
}

assert FailureRetainedUntilExplicitAction {
  always (Session.phase = Failed and not (resume or explicitDelete) implies
    Session.phase' = Failed)
}
assert FailedInventoryIsOptIn {
  always (Session.phase = Failed implies { i: Inventory | listed[i] } = OptedInCliList)
}
assert EarliestFailureSnapshotPreserved {
  always ((execute[0] or execute[1]) and some Session.anchor implies
    Session.anchor' = Session.anchor and Session.snapshot' = Session.snapshot)
  always (some Session.anchor implies Session.snapshot = prefix[Session.anchor])
}
assert FailedRewindRestoresPrefixBeforeEffects {
  always (resume and some Session.anchor and (Session.phase = Failed or Session.checkpoint = Exhausted) implies
    Session.cursor' = Session.anchor and Session.runtime' = Session.snapshot and
    no Session.anchor' and Session.effects' = Session.effects)
}
assert HardErrorUsesLiveCheckpoint {
  always (resume and Session.phase = Failed and Session.checkpoint = Live and no Session.anchor implies
    Session.cursor' = Session.cursor and Session.runtime' = Session.runtime)
}
assert LiveSuspensionKeepsUnresolvedFailure {
  always (resume and Session.phase = Suspended and Session.checkpoint = Live implies
    Session.cursor' = Session.cursor and Session.anchor' = Session.anchor and Session.snapshot' = Session.snapshot)
}
assert UnreliableCheckpointPreventsReplay {
  always (Session.checkpoint = Unreliable implies Session.phase' != Running and Session.effects' = Session.effects)
}
assert CompletedWithFailureCannotSucceed {
  always (Session.phase = Running and Session.checkpoint = Exhausted and some Session.anchor implies
    Session.phase' != Completed and Session.phase' != Removed)
}
assert AutomaticDisposalRequiresCompleted {
  always (Session.phase != Removed and Session.phase' = Removed and not explicitDelete implies Session.phase = Completed)
}

pred ContinuedFailureThenRepair {
  resume; execute[0]; execute[1]; execute[1]; settle;
  (Session.phase = Failed and Session.anchor = Middle and Tail in Session.effects and resume);
  execute[0]; execute[0]; settle; dispose
}
pred HardErrorAfterSoftFailure {
  resume; execute[0]; execute[1]; hardFailure;
  (Session.phase = Failed and Session.cursor = Tail and Session.anchor = Middle and resume);
  Session.cursor = Middle
}
pred ExhaustedThenInterrupted {
  resume; execute[0]; execute[1]; execute[0]; interrupt;
  (Session.phase = Suspended and Session.checkpoint = Exhausted and resume);
  Session.cursor = Middle
}
pred SetupFailureThenEntry {
  setupFailure; (Session.phase = Failed and Session.checkpoint = Unstarted and resume);
  Session.cursor = Entry
}
pred CorruptCheckpointRetained {
  resume; hardFailure; loseCheckpoint; (Session.phase = Failed and Session.checkpoint = Unreliable and observe)
}
pred LiveSuspensionThenContinuation {
  resume; execute[0]; execute[1]; interrupt;
  (Session.phase = Suspended and Session.cursor = Tail and Session.anchor = Middle and resume);
  (Session.cursor = Tail and execute[0]); settle; Session.phase = Failed
}
pred HardFailureWithoutEarlierFailure {
  resume; execute[0]; hardFailure;
  (Session.phase = Failed and Session.cursor = Middle and no Session.anchor and resume);
  Session.cursor = Middle
}
pred RepeatedFailureThenDelete {
  resume; execute[1]; execute[0]; execute[0]; settle;
  resume; execute[1]; execute[0]; execute[0]; settle; explicitDelete
}

check FailureRetainedUntilExplicitAction for 6 but 12 steps expect 0
check FailedInventoryIsOptIn for 6 but 12 steps expect 0
check EarliestFailureSnapshotPreserved for 6 but 12 steps expect 0
check FailedRewindRestoresPrefixBeforeEffects for 6 but 12 steps expect 0
check HardErrorUsesLiveCheckpoint for 6 but 12 steps expect 0
check LiveSuspensionKeepsUnresolvedFailure for 6 but 12 steps expect 0
check UnreliableCheckpointPreventsReplay for 6 but 12 steps expect 0
check CompletedWithFailureCannotSucceed for 6 but 12 steps expect 0
check AutomaticDisposalRequiresCompleted for 6 but 12 steps expect 0
run ContinuedFailureThenRepair for 6 but 12 steps expect 1
run HardErrorAfterSoftFailure for 6 but 12 steps expect 1
run ExhaustedThenInterrupted for 6 but 12 steps expect 1
run SetupFailureThenEntry for 6 but 12 steps expect 1
run CorruptCheckpointRetained for 6 but 12 steps expect 1
run RepeatedFailureThenDelete for 6 but 12 steps expect 1
run LiveSuspensionThenContinuation for 6 but 12 steps expect 1
run HardFailureWithoutEarlierFailure for 6 but 12 steps expect 1
