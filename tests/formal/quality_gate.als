// Paired with quality_gate.qnt. Eight ordered states, 4-bit signed integers.
// Atomic successful checkpoints only. External effect crash windows are excluded.
module quality_gate
open util/ordering[State]
open util/integer
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Event {}
one sig Init, Pass, Fail, Retry, Exhaust, ExhaustGoto, Resume, Idle extends Event {}
sig State { phase, count, limit, effects: one Int, feedback: one Bool, event: one Event }
pred same[a,b: State] {
  b.phase = a.phase and b.count = a.count and b.limit = a.limit
  b.effects = a.effects and b.feedback = a.feedback
}
pred initial[a: State] {
  a.phase = 0 and a.count = 0 and a.limit = 2 and a.effects = 0
  a.feedback = No and a.event = Init
}
pred step[a,b: State] {
  b.limit = a.limit and (
    (a.phase = 0 and b.event = Pass and b.phase = 2 and b.feedback = No
      and b.count = a.count and b.effects = a.effects.add[1])
    or (a.phase = 0 and b.event = Fail and b.phase = 1 and b.feedback = Yes
      and b.count = a.count and b.effects = a.effects.add[1])
    or (a.phase = 1 and a.count < a.limit and b.event = Retry and b.phase = 0
      and b.count = a.count.add[1] and b.effects = a.effects and b.feedback = a.feedback)
    or (a.phase = 1 and a.count >= a.limit and b.event = Exhaust and b.phase = 3
      and b.count = a.count and b.effects = a.effects and b.feedback = No)
    or (a.phase = 1 and a.count >= a.limit and b.event = ExhaustGoto and b.phase = 4
      and b.count = a.count and b.effects = a.effects and b.feedback = No)
    or (b.event in Resume + Idle and same[a,b]))
}
fact Trace { initial[first] and all a: State - last | step[a,next[a]] }
assert GateBehavior {
  all a: State | (a.event = Fail implies (a.phase = 1 and a.feedback = Yes))
    and (a.event = Pass implies (a.phase = 2 and a.feedback = No))
    and (a.phase = 1 implies a.feedback = Yes)
}
assert BudgetBehavior {
  all a: State | a.count >= 0 and a.count <= a.limit
  all a: State - last | let b = next[a] |
    (b.event = Retry implies (a.count < a.limit and b.count = a.count.add[1]))
    and (b.event != Retry implies b.count = a.count)
    and (a.phase >= 2 implies (b.phase = a.phase and b.effects = a.effects))
}
assert ResumeBehavior {
  all a: State - last | let b = next[a] |
    (b.event = Resume implies same[a,b])
    and (b.event in Retry + Exhaust + ExhaustGoto implies b.effects = a.effects)
    and (b.event in Exhaust + ExhaustGoto implies b.feedback = No)
}
pred RetryPassWitness {
  some a: State - last | a.event = Fail and next[a].event = Resume and next[next[a]].event = Retry
  last.phase = 2 and last.count = 1 and last.effects = 2 and last.feedback = No
}
pred ExhaustionWitness {
  last.phase = 3 and last.count = 2 and last.effects = 3 and last.feedback = No and last.event = Resume
}
pred ExhaustGotoWitness { last.phase = 4 and last.count = 2 and last.effects = 3 and last.feedback = No }
check GateBehavior for 8 State, 4 Int expect 0
check BudgetBehavior for 8 State, 4 Int expect 0
check ResumeBehavior for 8 State, 4 Int expect 0
run RetryPassWitness for 8 State, 4 Int expect 1
run ExhaustionWitness for 8 State, 4 Int expect 1
run ExhaustGotoWitness for 8 State, 4 Int expect 1
