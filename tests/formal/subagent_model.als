// Same abstraction and limits as subagent_model.qnt. Thirteen ordered states
// give twelve transitions. Five-bit signed Int includes legacy env token 9.
module subagent_model
open util/ordering[State]
open util/integer
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Event {}
one sig Init, Validate, Attempt, Configure, AgentsError, Send, Fallback, Finish, Idle extends Event {}
one sig Input {
  base, global, local, purpose: one Int,
  wellformed, command: one Bool,
  candidates, workflowCandidates: set Int
}
sig State {
  phase, attempt, target, applied, envModel, envForce, swarm: one Int,
  sent, failed: one Bool,
  event: one Event
}
fact Inputs {
  Input.base in 0 + 1 + 2
  Input.global in -1 + 0 + 1 + 2 + 3
  Input.local in -1 + 0 + 1 + 2 + 3
  Input.purpose in 0 + 1 + 2
  some Input.candidates
  Input.candidates in 0 + 1 + 2
  Input.candidates in Input.workflowCandidates
  Input.workflowCandidates in 0 + 1 + 2
}
fun owner[e: Int]: one Int { e = 3 => Input.base else e }
pred allowed[e: Int, candidates: set Int] {
  e = -1 or (Input.wellformed = Yes and Input.command = No
    and owner[e] in 0 + 1 and owner[e] in candidates)
}
pred accepted {
  allowed[Input.global, Input.workflowCandidates] and allowed[Input.local, Input.candidates]
  and (Input.local = -1 or Input.purpose = 0)
}
fun chosen: one Int {
  Input.purpose = 2 => -1 else
  (Input.purpose = 0 and Input.local != -1) => Input.local else Input.global
}
fun token: one Int { (Input.purpose = 0 and Input.local != -1) => 2 else 1 }
fun appliedTo[t: Int]: one Int { (chosen != -1 and owner[chosen] = t) => token else 0 }
pred assign[b: State, p,n,t,m,em,ef,sw: Int, se,fa: Bool] {
  b.phase = p and b.attempt = n and b.target = t and b.applied = m
  b.envModel = em and b.envForce = ef and b.swarm = sw
  b.sent = se and b.failed = fa
}
pred initial[a: State] { assign[a,0,0,-1,0,7,9,0,No,No] and a.event = Init }
pred validate[a,b: State] {
  a.phase = 0 and b.event = Validate
  assign[b,(accepted => 1 else 5),0,-1,0,7,9,0,No,No]
}
pred attempt[a,b: State] {
  a.phase = 1 and a.attempt < 2 and b.event = Attempt
  b.target in Input.candidates
  assign[b,2,a.attempt.add[1],b.target,0,7,9,0,No,No]
}
pred configure[a,b: State] {
  a.phase = 2 and b.event = Configure
  let m = appliedTo[a.target] |
    assign[b,3,a.attempt,a.target,m,
      ((m != 0 and a.target = 1) => m else 7),
      ((m != 0 and a.target = 1) => 1 else 9),
      ((m != 0 and a.target = 0) => m else 0),a.sent,a.failed]
}
pred progress[a,b: State] {
  (a.phase = 3 and a.swarm != 0 and b.event = AgentsError
    and assign[b,5,a.attempt,a.target,a.applied,a.envModel,a.envForce,a.swarm,a.sent,Yes])
  or (a.phase = 3 and b.event = Send
    and assign[b,4,a.attempt,a.target,a.applied,a.envModel,a.envForce,a.swarm,Yes,a.failed])
  or (a.phase = 4 and a.attempt < 2 and b.event = Fallback
    and assign[b,1,a.attempt,a.target,a.applied,a.envModel,a.envForce,a.swarm,a.sent,a.failed])
  or (a.phase = 4 and b.event = Finish
    and assign[b,5,a.attempt,a.target,a.applied,a.envModel,a.envForce,a.swarm,a.sent,a.failed])
  or (b.event = Idle
    and assign[b,a.phase,a.attempt,a.target,a.applied,a.envModel,a.envForce,a.swarm,a.sent,a.failed])
}
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] |
    validate[a,b] or attempt[a,b] or configure[a,b] or progress[a,b]
}
assert RejectionBeforeDispatch {
  not accepted implies all a: State | a.attempt = 0 and a.sent = No
}
assert PrecedenceAndSdkIsolation {
  all a: State | a.applied != 0 implies
    (chosen != -1 and a.target = owner[chosen] and a.applied = token and a.target in 0 + 1)
  all a: State | a.phase in 3 + 4 implies a.applied = appliedTo[a.target]
}
assert ExcludedMetadata {
  Input.purpose = 2 implies all a: State | a.applied = 0
}
assert BackendContract {
  all a: State |
    ((a.applied != 0 and a.target = 1) implies (a.envModel = a.applied and a.envForce = 1))
    and ((a.applied = 0 or a.target != 1) implies (a.envModel = 7 and a.envForce = 9))
    and (a.swarm != 0 iff (a.applied != 0 and a.target = 0))
    and (a.swarm != 0 implies a.swarm = a.applied)
}
assert ConfigureBeforeSend {
  all a: State | a.failed = Yes implies (a.sent = No and a.phase = 5)
  all a: State | a.attempt >= 0 and a.attempt <= 2
  all a: State - last | let b = next[a] |
    (b.event = Send implies (a.phase = 3 and a.failed = No))
    and (b.event = Attempt implies (accepted and a.phase = 1))
}
pred PrecedenceFallbackWitness {
  Input.base = 0 and Input.global = 0 and Input.local = 1 and Input.purpose = 0
  some a,b: State | lt[a,b] and a.event = Send and a.target = 0 and a.applied = 0
    and b.event = Send and b.target = 1 and b.applied = 2 and b.envForce = 1
}
pred BarePlanningWitness {
  Input.base = 0 and Input.global = 3 and Input.local = -1 and Input.purpose = 1
  some a: State | a.event = Send and a.swarm = 1
}
pred MetadataWitness {
  Input.global = 1 and Input.local = -1 and Input.purpose = 2
  some a: State | a.event = Send and a.target = 1 and a.applied = 0 and a.envModel = 7
}
pred LegacyEnvironmentWitness {
  Input.global = -1 and Input.local = -1 and Input.purpose = 0
  some a: State | a.event = Send and a.target = 1 and a.envModel = 7 and a.envForce = 9
}
pred AgentsFailureWitness {
  Input.global = 0 and Input.local = -1 and Input.purpose = 0
  some a: State | a.event = AgentsError and a.swarm = 1 and a.failed = Yes and a.sent = No
}
pred RejectedWitness {
  Input.global = 2 and Input.local = -1
  some a: State | a.event = Validate and a.phase = 5 and a.attempt = 0
}
pred OtherPhaseApplicabilityWitness {
  Input.global = 1 and Input.local = -1 and Input.purpose = 0
  Input.candidates = 0 and 1 in Input.workflowCandidates
  some a: State | a.event = Send and a.target = 0 and a.applied = 0
}
pred LocalApplicabilityRejectionWitness {
  Input.global = -1 and Input.local = 1 and Input.purpose = 0
  Input.candidates = 0 and 1 in Input.workflowCandidates
  some a: State | a.event = Validate and a.phase = 5 and a.attempt = 0
}
check RejectionBeforeDispatch for 13 State, 5 Int expect 0
check PrecedenceAndSdkIsolation for 13 State, 5 Int expect 0
check ExcludedMetadata for 13 State, 5 Int expect 0
check BackendContract for 13 State, 5 Int expect 0
check ConfigureBeforeSend for 13 State, 5 Int expect 0
run PrecedenceFallbackWitness for 13 State, 5 Int expect 1
run BarePlanningWitness for 13 State, 5 Int expect 1
run MetadataWitness for 13 State, 5 Int expect 1
run LegacyEnvironmentWitness for 13 State, 5 Int expect 1
run AgentsFailureWitness for 13 State, 5 Int expect 1
run RejectedWitness for 13 State, 5 Int expect 1
run OtherPhaseApplicabilityWitness for 13 State, 5 Int expect 1
run LocalApplicabilityRejectionWitness for 13 State, 5 Int expect 1
