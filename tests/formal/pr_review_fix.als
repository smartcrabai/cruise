// Review loop with a fixed permission classification and three thread IDs.
module pr_review_fix
open util/ordering[State]
abstract sig Thread {}
one sig T0, T1, T2 extends Thread {}
abstract sig Status {}
one sig Running, Success, NoProgress, Limit extends Status {}
abstract sig Event {}
one sig Init, Empty, Iterate, Idle extends Event {}
one sig Permissions { eligible: set Thread }
sig State { unresolved, prompted, prior: set Thread, iteration: one Int,
  status: one Status, event: one Event }
pred initial[a: State] {
  no a.prompted and no a.prior and a.iteration = 0 and a.status = Running and a.event = Init
}
pred idle[a,b: State] {
  b.unresolved = a.unresolved and b.prompted = a.prompted and b.prior = a.prior
  and b.iteration = a.iteration and b.status = a.status and b.event = Idle
}
pred emptySuccess[a,b: State] {
  a.status = Running and no a.unresolved & Permissions.eligible
  b.unresolved = a.unresolved and b.prompted = a.prompted and b.prior = a.prior
  and b.iteration = a.iteration and b.status = Success and b.event = Empty
}
pred iterate[a,b: State] {
  a.status = Running and some a.unresolved & Permissions.eligible and a.iteration < 2
  b.prior = a.unresolved & Permissions.eligible and b.prompted = b.prior
  b.unresolved - Permissions.eligible = a.unresolved - Permissions.eligible
  b.iteration = a.iteration.plus[1] and b.event = Iterate
  let result = b.unresolved & Permissions.eligible |
    b.status = (no result => Success else (result = b.prior => NoProgress else
      (b.iteration = 2 => Limit else Running)))
}
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] | idle[a,b] or emptySuccess[a,b] or iterate[a,b]
}
assert PermissionFilterBeforePrompt {
  all a: State - last | let b = next[a] | b.event = Iterate implies
    b.prompted = a.unresolved & Permissions.eligible
}
assert ExcludedNeverPrompted { all a: State | no a.prompted - Permissions.eligible }
assert SuccessHasNoEligible { all a: State | a.status = Success implies no a.unresolved & Permissions.eligible }
assert TerminalAbsorbing { all a: State - last | a.status != Running implies idle[a,next[a]] }
assert NoProgressIffSameSet {
  all a: State | a.event = Iterate implies
    (a.status = NoProgress iff (some a.unresolved & Permissions.eligible and a.unresolved & Permissions.eligible = a.prior))
}
assert IterationsBounded { all a: State | a.iteration >= 0 and a.iteration <= 2 }
pred SuccessWitness { some a: State | a.status = Success and some a.unresolved - Permissions.eligible }
pred NoProgressWitness { some a: State | a.status = NoProgress }
pred LimitWitness { some a: State | a.status = Limit }
check PermissionFilterBeforePrompt for 5 but exactly 5 State, 4 Int expect 0
check ExcludedNeverPrompted for 5 but exactly 5 State, 4 Int expect 0
check SuccessHasNoEligible for 5 but exactly 5 State, 4 Int expect 0
check TerminalAbsorbing for 5 but exactly 5 State, 4 Int expect 0
check NoProgressIffSameSet for 5 but exactly 5 State, 4 Int expect 0
check IterationsBounded for 5 but exactly 5 State, 4 Int expect 0
run SuccessWitness for 5 but exactly 5 State, 4 Int expect 1
run NoProgressWitness for 5 but exactly 5 State, 4 Int expect 1
run LimitWitness for 5 but exactly 5 State, 4 Int expect 1
