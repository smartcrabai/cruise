// Single-session bounded batch abstraction, with persisted count possibly above
// a lowered limit. Nine ordered states represent eight transitions.
module auto_requeue
open util/ordering[State]
abstract sig Phase {}
one sig Planned, Suspended, Running, Failed, Completed extends Phase {}
abstract sig Error {}
one sig None, Eligible, Ineligible, Cancelled extends Error {}
abstract sig Event {}
one sig Init, Start, Finish, Auto, Manual, Reset, Idle extends Event {}
one sig Config { limit: one Int }
abstract sig Mode {}
one sig Batch, ManualRun extends Mode {}
sig State { phase: one Phase, count: one Int, err: one Error, event: one Event, mode: one Mode }
pred canAuto[a: State] { a.mode = Batch and a.phase = Failed and a.err = Eligible and a.count < Config.limit }
pred initial[a: State] {
  a.phase in Planned + Suspended and a.err = None and a.event = Init and a.mode = ManualRun
  a.count >= 0 and a.count <= 2 and Config.limit >= 0 and Config.limit <= 2
}
pred start[a,b: State] {
  a.phase in Planned + Suspended and b.phase = Running and b.err = None
  b.count = a.count and b.event = Start and b.mode = Batch
}
pred finish[a,b: State] {
  a.phase = Running and b.event = Finish and b.count = a.count and b.mode = a.mode
  ((b.phase = Completed and b.err = None) or
    (b.phase = Failed and b.err in Eligible + Ineligible + Cancelled))
}
pred auto[a,b: State] {
  canAuto[a] and b.phase = Running and b.count = add[a.count, 1]
  b.err = None and b.event = Auto and b.mode = a.mode
}
pred manual[a,b: State] {
  a.phase = Failed and b.phase = Running and b.count = a.count
  b.err = None and b.event = Manual and b.mode = ManualRun
}
pred reset[a,b: State] {
  a.phase in Failed + Completed + Suspended and b.phase = Planned
  b.count = 0 and b.err = None and b.event = Reset and b.mode = ManualRun
}
pred idle[a,b: State] {
  b.phase = a.phase and b.count = a.count and b.err = a.err and b.event = Idle and b.mode = a.mode
}
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] |
    start[a,b] or finish[a,b] or auto[a,b] or manual[a,b] or reset[a,b] or idle[a,b]
}
assert AutoEligibility {
  all a: State - last | let b = next[a] | b.event = Auto implies
    canAuto[a] and b.count = add[a.count, 1] and b.phase = Running
}
assert CounterDiscipline {
  all a: State - last | let b = next[a] |
    (b.event != Auto implies b.count <= a.count) and
    (b.event = Manual implies b.count = a.count) and
    (b.event = Reset implies b.count = 0 and b.phase = Planned)
}
assert Budget {
  all s: State | s.count >= 0 and s.count <= 2 and
    (first.count <= Config.limit implies s.count <= Config.limit)
}
assert StartEligibility {
  all a: State - last | next[a].event = Start implies a.phase in Planned + Suspended
}
assert NoRetryAtOrAboveLimit {
  all a: State | a.count >= Config.limit implies not canAuto[a]
}
assert ManualFailureNeverAutoRequeues {
  all a: State | a.mode = ManualRun and a.phase = Failed implies
    not canAuto[a] and (a != last implies next[a].event != Auto)
}
pred ManualEligibleFailure {
  some a: State - last | a.event = Manual and next[a].event = Finish and
    next[a].phase = Failed and next[a].err = Eligible and
    next[a].count < Config.limit and not canAuto[next[a]]
}
pred RetrySuccess { some a, b: State | a.event = Auto and b in a.^next and b.phase = Completed }
pred Exhausted { some s: State | s.phase = Failed and s.err = Eligible and s.count = Config.limit and not canAuto[s] and s.count > 0 }
pred Disabled { Config.limit = 0 and some s: State | s.phase = Failed and s.err = Eligible and not canAuto[s] }
pred IneligibleFailure { some s: State | s.phase = Failed and s.err = Ineligible and not canAuto[s] }
pred CancelledFailure { some s: State | s.phase = Failed and s.err = Cancelled and not canAuto[s] }
pred ManualAndReset { some a, b: State | a.event = Manual and b in a.^next and b.event = Reset }
pred LoweredLimit { first.count > Config.limit and some s: State | s.phase = Failed and s.err = Eligible and not canAuto[s] }
check ManualFailureNeverAutoRequeues for exactly 9 State, 4 Int expect 0
run ManualEligibleFailure for exactly 9 State, 4 Int expect 1
check AutoEligibility for exactly 9 State, 4 Int expect 0
check CounterDiscipline for exactly 9 State, 4 Int expect 0
check Budget for exactly 9 State, 4 Int expect 0
check StartEligibility for exactly 9 State, 4 Int expect 0
check NoRetryAtOrAboveLimit for exactly 9 State, 4 Int expect 0
run RetrySuccess for exactly 9 State, 4 Int expect 1
run Exhausted for exactly 9 State, 4 Int expect 1
run Disabled for exactly 9 State, 4 Int expect 1
run IneligibleFailure for exactly 9 State, 4 Int expect 1
run CancelledFailure for exactly 9 State, 4 Int expect 1
run ManualAndReset for exactly 9 State, 4 Int expect 1
run LoweredLimit for exactly 9 State, 4 Int expect 1
