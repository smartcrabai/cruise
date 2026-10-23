// Paired with opencode.qnt. 15 ordered states = 14 transitions, 5-bit Int.
// Small observable contract, NOT a full HTTP/SSE/process implementation model.
module opencode
open util/ordering[State]
open util/integer
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Event {}
one sig Init, Start, Checkpoint, Subscribe, Submit, Effect, Succeed,
  ProviderFailure, Uncertain, Cancel, Stop, Publish, Irrelevant extends Event {}
sig State {
  phase, attempts, prompts, effects, result: one Int,
  alive, checkpoint, subscribed, barrier, evidence: one Bool,
  event: one Event
}
pred assign[b: State, p, a, n, e, r: Int, live, cp, sub, bar, ev: Bool] {
  b.phase = p and b.attempts = a and b.prompts = n and b.effects = e and b.result = r
  b.alive = live and b.checkpoint = cp and b.subscribed = sub
  b.barrier = bar and b.evidence = ev
}
pred identical[a,b: State] {
  assign[b,a.phase,a.attempts,a.prompts,a.effects,a.result,
    a.alive,a.checkpoint,a.subscribed,a.barrier,a.evidence]
}
pred initial[a: State] {
  assign[a,0,0,0,0,0,No,No,No,No,No] and a.event = Init
}
pred start[a,b: State] {
  (a.phase = 0 or (a.phase = 6 and a.result = 2 and a.barrier = No))
  a.attempts < 2
  assign[b,1,a.attempts.add[1],a.prompts,a.effects,0,Yes,No,No,a.barrier,No]
  b.event = Start
}
pred checkpoint[a,b: State] {
  a.phase = 1
  assign[b,2,a.attempts,a.prompts,a.effects,a.result,a.alive,Yes,a.subscribed,a.barrier,a.evidence]
  b.event = Checkpoint
}
pred subscribe[a,b: State] {
  a.phase = 2
  assign[b,3,a.attempts,a.prompts,a.effects,a.result,a.alive,a.checkpoint,Yes,a.barrier,a.evidence]
  b.event = Subscribe
}
pred submit[a,b: State] {
  a.phase = 3
  assign[b,4,a.attempts,a.prompts.add[1],a.effects,a.result,a.alive,a.checkpoint,a.subscribed,a.barrier,a.evidence]
  b.event = Submit
}
pred effect[a,b: State] {
  a.phase = 4 and a.effects = 0
  assign[b,4,a.attempts,a.prompts,1,a.result,a.alive,a.checkpoint,a.subscribed,Yes,a.evidence]
  b.event = Effect
}
pred resolve[a,b: State] {
  (a.phase = 4 and b.event = Succeed and b.result = 1 and b.evidence = Yes and b.barrier = a.barrier)
  or (a.phase = 4 and b.event = ProviderFailure
    and b.result = (a.barrier = Yes => 3 else 2) and b.evidence = a.evidence and b.barrier = a.barrier)
  or (a.phase >= 1 and a.phase <= 4 and b.event = Uncertain
    and b.result = 3 and b.evidence = a.evidence and b.barrier = Yes)
  or (a.phase >= 1 and a.phase <= 4 and b.event = Cancel
    and b.result = 4 and b.evidence = a.evidence and b.barrier = Yes)
  b.phase = 5 and b.attempts = a.attempts and b.prompts = a.prompts
  b.effects = a.effects and b.alive = a.alive and b.checkpoint = a.checkpoint and b.subscribed = a.subscribed
}
pred stop[a,b: State] {
  a.phase = 5
  assign[b,6,a.attempts,a.prompts,a.effects,a.result,No,a.checkpoint,a.subscribed,a.barrier,a.evidence]
  b.event = Stop
}
pred publish[a,b: State] {
  a.phase = 6
  assign[b,7,a.attempts,a.prompts,a.effects,a.result,a.alive,a.checkpoint,a.subscribed,a.barrier,a.evidence]
  b.event = Publish
}
pred irrelevant[a,b: State] { identical[a,b] and b.event = Irrelevant }
pred step[a,b: State] {
  start[a,b] or checkpoint[a,b] or subscribe[a,b] or submit[a,b]
    or effect[a,b] or resolve[a,b] or stop[a,b] or publish[a,b] or irrelevant[a,b]
}
fact Trace { initial[first] and all a: State - last | step[a,next[a]] }
assert Admission {
  all a: State | a.prompts >= 0 and a.prompts <= 2 and a.prompts <= a.attempts
  all a: State - last | let b = next[a] |
    (b.event = Submit implies (a.checkpoint = Yes and a.subscribed = Yes and a.phase = 3 and b.prompts = a.prompts.add[1]))
    and (b.event != Submit implies b.prompts = a.prompts)
}
assert RetrySafety {
  all a: State | a.effects > 0 implies a.barrier = Yes
  all a: State - last | let b = next[a] |
    (a.barrier = Yes implies (b.barrier = Yes and b.prompts = a.prompts))
    and ((b.event = Start and a.attempts > 0) implies
      (a.phase = 6 and a.alive = No and a.result = 2 and a.barrier = No))
}
assert Completion {
  all a: State | (a.result = 1 implies a.evidence = Yes)
    and (a.phase = 7 implies (a.alive = No and a.result != 0))
  all a: State - last | let b = next[a] |
    (b.event = Publish implies (a.phase = 6 and a.alive = No))
    and (b.event = Irrelevant implies identical[a,b])
    and (a.phase = 7 implies identical[a,b])
}
assert Domains {
  all a: State | a.phase >= 0 and a.phase <= 7 and a.attempts >= 0 and a.attempts <= 2
    and a.effects >= 0 and a.effects <= 1 and a.result >= 0 and a.result <= 4
    and ((a.phase >= 1 and a.phase <= 5) implies a.alive = Yes)
}
pred SuccessWitness {
  last.phase = 7 and last.result = 1 and last.prompts = 1
  some a: State - last | a.phase = 4 and next[a].event = Irrelevant
}
pred SilentEffectFailureWitness {
  last.phase = 7 and last.result = 3 and last.effects = 1 and last.prompts = 1
  some a: State | a.event = ProviderFailure
}
pred SafeFallbackWitness {
  last.phase = 7 and last.result = 1 and last.attempts = 2 and last.prompts = 2
}
// Require the named boundary, not merely a compatible final result. In
// particular, a second attempt failing during setup is not a witness of lost
// acceptance or cancellation of an active input.
pred UncertainWitness {
  last.phase = 7 and last.result = 3 and last.prompts = 1 and last.effects = 0
  last.attempts = 1
  some a: State - last | a.phase = 4 and next[a].event = Uncertain
}
pred RejectedCheckpointWitness {
  last.phase = 7 and last.result = 3 and last.prompts = 0
  last.attempts = 1 and last.checkpoint = No
  some a: State - last | a.phase = 1 and next[a].event = Uncertain
}
pred CancellationWitness {
  last.phase = 7 and last.result = 4 and last.prompts = 1
  last.attempts = 1
  some a: State - last | a.phase = 4 and next[a].event = Cancel
}
check Admission for 15 State, 5 Int expect 0
check RetrySafety for 15 State, 5 Int expect 0
check Completion for 15 State, 5 Int expect 0
check Domains for 15 State, 5 Int expect 0
run SuccessWitness for 15 State, 5 Int expect 1
run SilentEffectFailureWitness for 15 State, 5 Int expect 1
run SafeFallbackWitness for 15 State, 5 Int expect 1
run UncertainWitness for 15 State, 5 Int expect 1
run RejectedCheckpointWitness for 15 State, 5 Int expect 1
run CancellationWitness for 15 State, 5 Int expect 1
