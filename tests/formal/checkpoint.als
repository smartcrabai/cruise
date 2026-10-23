// Paired with checkpoint.qnt. Reconstructed CP IDs map to tests/graph_state.rs.
// Two fixed nodes (current 0 work, 1 finish, 2 done), one retry pair,
// one forward pair and one group. Counts/limits 0..2, effects/runtime 0..4.
// 13 ordered states = 12 transitions, 4-bit signed Int prevents overflow here.
// Successful saves are atomic. Resume uses only successful checkpoints.
// External effects and the following save are NOT crash-atomic/exactly-once.
// Validation is an abstract rejection guard, tested concretely by Cucumber.
// Only automatic/node-ID resume is modeled. Manual reposition and usize overflow
// have concrete BDD coverage, not an unbounded claim from this scalar model.
// gate authorizes one effect after a successful save; saved marks resumable state.
// No fairness, unbounded proof, arbitrary topology or JSON-decoder claim.
module checkpoint
open util/ordering[State]
open util/integer
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Event {}
one sig Init, Save, CheckpointFailure, Execute, Accept, RejectBudget,
  Resume, Policy, RejectInvalid, Idle extends Event {}
sig State {
  current, pending, total, budget, forward, forwardBudget,
    groups, effects, runtime, limit: one Int,
  gate, saved: one Bool,
  event: one Event
}

pred sameCounts[a, b: State] {
  b.total = a.total and b.budget = a.budget
  b.forward = a.forward and b.forwardBudget = a.forwardBudget
  b.groups = a.groups
}
pred samePosition[a, b: State] { b.current = a.current and b.pending = a.pending }
pred sameEffects[a, b: State] { b.effects = a.effects and b.runtime = a.runtime }
pred sameData[a, b: State] {
  sameCounts[a,b] and samePosition[a,b] and sameEffects[a,b] and b.limit = a.limit
}
pred identical[a, b: State] {
  sameData[a,b] and b.gate = a.gate and b.saved = a.saved
}
pred initial[a: State] {
  a.current = 0 and a.pending = 0 and a.total = 0 and a.budget = 0
  a.forward = 0 and a.forwardBudget = 0 and a.groups = 0
  a.effects = 0 and a.runtime = 0 and a.limit = 2
  a.gate = No and a.saved = Yes and a.event = Init
}
pred authorize[a, b: State] {
  a.current < 2 and sameData[a,b]
  b.gate = Yes and b.saved = Yes and b.event = Save
}
pred checkpointFailure[a, b: State] {
  sameData[a,b] and b.saved = a.saved
  b.gate = No and b.event = CheckpointFailure
}
pred execute[a, b: State] {
  a.current < 2 and a.pending = 0 and a.gate = Yes
  b.current = a.current and sameCounts[a,b] and b.limit = a.limit
  ((a.current = 0 and b.pending >= 1 and b.pending <= 4)
    or (a.current = 1 and b.pending = 5))
  // Exhausted group invocation is skipped before its body executes.
  ((a.current = 0 and a.groups = 2) implies b.pending = 3)
  (b.pending != 3 implies a.effects < 4)
  b.effects = (b.pending = 3 => a.effects else a.effects.add[1])
  b.runtime = b.effects and b.gate = No and b.saved = No and b.event = Execute
}
pred accept[a, b: State] {
  a.pending > 0 and b.pending = 0 and sameEffects[a,b] and b.limit = a.limit
  b.gate = No and b.saved = Yes and b.event = Accept
  (a.pending <= 2 and a.budget < a.limit and a.total < 2
    and b.current = a.current and b.total = a.total.add[1]
    and b.budget = a.budget.add[1]
    and b.groups = (a.pending = 2 => a.groups.add[1] else a.groups)
    and b.forward = a.forward and b.forwardBudget = a.forwardBudget)
  or (a.pending >= 3 and a.pending <= 4
    and (a.pending = 3 or a.forwardBudget < a.limit)
    and b.current = 1 and b.total = a.total and b.budget = a.budget
    and b.groups = a.groups and b.forward = a.forward.add[1]
    and b.forwardBudget = (a.pending = 4 => a.forwardBudget.add[1] else a.forwardBudget))
  or (a.pending = 5 and b.current = 2 and sameCounts[a,b])
}
pred rejectBudget[a, b: State] {
  ((a.pending > 0 and a.pending <= 2 and a.budget >= a.limit)
    or (a.pending = 4 and a.forwardBudget >= a.limit))
  sameData[a,b] and b.saved = Yes and b.gate = No and b.event = RejectBudget
}
pred resume[a, b: State] { a.saved = Yes and identical[a,b] and b.event = Resume }
pred policy[a, b: State] {
  sameCounts[a,b] and samePosition[a,b] and sameEffects[a,b]
  b.gate = a.gate and b.saved = a.saved
  b.limit >= 0 and b.limit <= 2 and b.event = Policy
}
pred rejectInvalid[a, b: State] { identical[a,b] and b.event = RejectInvalid }
pred idle[a, b: State] { identical[a,b] and b.event = Idle }
pred step[a, b: State] {
  authorize[a,b] or checkpointFailure[a,b] or execute[a,b] or accept[a,b]
  or rejectBudget[a,b] or resume[a,b] or policy[a,b] or rejectInvalid[a,b] or idle[a,b]
}
// This trace constraint is essential: assertions range over reachable behavior,
// not unconstrained records. Domains are consequences, not assumed safety facts.
fact Trace { initial[first] and all a: State - last | step[a, next[a]] }

// CP-01 engine::accept_transition: accepted counts monotone, denial consumes none.
// Historical counts may exceed a newly lowered ceiling, never reset to fit it.
assert CP01Accounting {
  all a: State | a.total >= 0 and a.total <= 2 and a.budget >= 0 and a.budget <= a.total
    and a.forward >= 0 and a.forward <= 1 and a.forwardBudget >= 0 and a.forwardBudget <= a.forward
  all a: State - last | let b = next[a] |
    b.total >= a.total and b.budget >= a.budget
    and ((b.event = Accept and a.pending <= 2) implies
      (a.budget < a.limit and b.budget = a.budget.add[1] and b.total = a.total.add[1]
        and b.forward = a.forward and b.forwardBudget = a.forwardBudget))
    and ((b.event = Accept and a.pending >= 3 and a.pending <= 4) implies
      (b.forward = a.forward.add[1]
        and b.forwardBudget = (a.pending = 4 => a.forwardBudget.add[1] else a.forwardBudget)
        and b.total = a.total and b.budget = a.budget))
    and ((b.event != Accept or a.pending = 5) implies
      (b.total = a.total and b.budget = a.budget and b.forward = a.forward and b.forwardBudget = a.forwardBudget))
    and (b.event = RejectBudget implies
      (b.total = a.total and b.budget = a.budget and b.forward = a.forward and b.forwardBudget = a.forwardBudget))
}
// CP-02 engine pending outcome path never reexecutes its source side effect.
assert CP02Pending {
  all a: State | a.pending > 0 implies a.current < 2
  all a: State - last | let b = next[a] |
    (b.event = Execute implies a.pending = 0)
    and (b.event in Accept + RejectBudget implies b.effects = a.effects)
    and (b.event = RejectBudget implies b.pending = a.pending)
}
// CP-03 save_checkpoint failures cannot authorize the next command.
assert CP03CheckpointGate {
  all a: State - last | let b = next[a] |
    (b.event = Execute implies a.gate = Yes)
    and (b.event = CheckpointFailure implies (b.gate = No and b.effects = a.effects))
}
// CP-04 skip/group-exhaustion traversals do not spend budget or run commands.
assert CP04Skip {
  all a: State - last | let b = next[a] |
    ((b.event = Execute and b.pending = 3) implies b.effects = a.effects)
    and ((b.event = Accept and a.pending = 3) implies
      (b.forward = a.forward.add[1] and b.forwardBudget = a.forwardBudget))
}
// CP-05 persistence::prepare_resume restores authoritative state, not stale display.
assert CP05Resume { all a: State - last | let b = next[a] | b.event = Resume implies identical[a,b] }
// CP-06 persistence validation rejects incompatible inputs without changing bytes.
assert CP06Reject { all a: State - last | let b = next[a] | b.event = RejectInvalid implies identical[a,b] }
// CP-07 group acceptance is atomic with edge accounting, opaque snapshots survive.
assert CP07Group {
  all a: State | a.groups >= 0 and a.groups <= a.budget
  all a: State - last | let b = next[a] |
    ((b.event = Accept and a.pending = 2) implies b.groups = a.groups.add[1])
    and ((b.event != Accept or a.pending != 2) implies b.groups = a.groups)
    and ((b.event = Execute and a.current = 0 and a.groups = 2) implies
      (b.pending = 3 and b.effects = a.effects))
    and (b.event != Execute implies b.runtime = a.runtime)
}
// CP-08 build_graph fixed identities and completed execution has no more effects.
assert CP08Completed {
  all a: State | a.current >= 0 and a.current <= 2 and a.pending >= 0 and a.pending <= 5
    and a.effects >= 0 and a.effects <= 4 and a.runtime = a.effects
    and (a.current = 2 implies a.pending = 0)
  all a: State - last | let b = next[a] |
    a.current = 2 implies (b.current = 2 and b.effects = a.effects)
}
// Non-vacuous witnesses mirror Quint's named executable tests, including lowering
// the ceiling, denied pending resume, then raising it and accepting exactly once.
pred RetryResumeWitness {
  first.next.event = Save
  first.next.next.event = Execute and first.next.next.pending = 1
  first.next.next.next.event = Accept
  some a: State - last | a.event = RejectBudget and a.pending = 1 and a.total = 1
    and next[a].event = Resume
  last.total = 2 and last.effects = 2 and last.pending = 0
}
pred SkipCompletionWitness {
  some a: State | a.event = CheckpointFailure
  some a: State | a.event = Execute and a.pending = 3
  last.current = 2 and last.effects = 1 and last.forward = 1 and last.forwardBudget = 0
  last.event = Resume
}
pred GroupExhaustionWitness {
  some a: State | a.event = Execute and a.pending = 3 and a.groups = 2
  last.groups = 2 and last.current = 2 and last.effects = 3
  last.forward = 1 and last.forwardBudget = 0
}
pred GroupRuntimeWitness {
  some a: State | a.event = Accept and a.groups = 1
  some a: State | a.event = Resume and a.runtime = 1
  last.event = RejectInvalid and last.groups = 1 and last.runtime = 1
}
check CP01Accounting for 13 State, 4 Int expect 0
check CP02Pending for 13 State, 4 Int expect 0
check CP03CheckpointGate for 13 State, 4 Int expect 0
check CP04Skip for 13 State, 4 Int expect 0
check CP05Resume for 13 State, 4 Int expect 0
check CP06Reject for 13 State, 4 Int expect 0
check CP07Group for 13 State, 4 Int expect 0
check CP08Completed for 13 State, 4 Int expect 0
run RetryResumeWitness for 13 State, 4 Int expect 1
run SkipCompletionWitness for 13 State, 4 Int expect 1
run GroupRuntimeWitness for 13 State, 4 Int expect 1
run GroupExhaustionWitness for 13 State, 4 Int expect 1
