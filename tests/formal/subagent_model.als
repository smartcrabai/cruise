// Jcode-only subagent route configuration and prompt-dispatch ordering.
module subagent_model
open util/ordering[State]
open util/integer
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Phase {}
one sig Initial, Validated, Configured, Sent, Failed extends Phase {}
abstract sig Event {}
one sig Init, Validate, Configure, Send, Idle extends Event {}
one sig Input {
  global, local, purpose: one Int,
  wellformed, command, settingFails: one Bool
}
sig State {
  phase: one Phase, route, agentsCalls: one Int,
  configured, sent, failed: one Bool,
  event: one Event
}
fact Inputs {
  Input.global in -1 + 0
  Input.local in -1 + 0 + 1
  Input.purpose in 0 + 1 + 2
}
pred accepted { Input.wellformed = Yes and Input.command = No }
fun chosen: one Int {
  Input.purpose = 2 => -1 else
  (Input.purpose = 0 and Input.local != -1) => Input.local else Input.global
}
pred assign[b: State, ph: Phase, r, calls: Int, conf, sentValue, failedValue: Bool, e: Event] {
  b.phase = ph and b.route = r and b.agentsCalls = calls
  b.configured = conf and b.sent = sentValue and b.failed = failedValue and b.event = e
}
pred initial[a: State] { assign[a,Initial,-1,0,No,No,No,Init] }
pred validate[a,b: State] {
  a.phase = Initial
  assign[b,(accepted => Validated else Failed),-1,0,No,No,(accepted => No else Yes),Validate]
}
pred configure[a,b: State] {
  a.phase = Validated
  let r = chosen |
    assign[b,((r != -1 and Input.settingFails = Yes) => Failed else Configured),r,
      a.agentsCalls.add[(r != -1 => 1 else 0)],
      ((r != -1 and Input.settingFails = No) => Yes else No),No,
      ((r != -1 and Input.settingFails = Yes) => Yes else No),Configure]
}
pred send[a,b: State] {
  a.phase = Configured
  assign[b,Sent,a.route,a.agentsCalls,a.configured,Yes,No,Send]
}
pred idle[a,b: State] {
  assign[b,a.phase,a.route,a.agentsCalls,a.configured,a.sent,a.failed,Idle]
}
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] | validate[a,b] or configure[a,b] or send[a,b] or idle[a,b]
}
assert RejectionBeforeSend {
  not accepted implies all a: State | a.sent = No and a.phase in Initial + Failed
}
assert Selection {
  all a: State | a.configured = Yes implies a.route = chosen and a.route != -1
}
assert ConfigureBeforeSend {
  all a: State - last | let b = next[a] |
    (b.event = Send implies
      (a.phase = Configured and (a.route = -1 or a.configured = Yes)))
    and (b.event = Configure and chosen != -1 implies b.agentsCalls = a.agentsCalls.add[1])
    and (a.failed = Yes implies b.sent = No)
}
assert Scope {
  Input.purpose = 2 implies all a: State | a.configured = No and a.agentsCalls = 0
}
pred StepPrecedenceWitness {
  Input.global = 0 and Input.local = 1 and Input.purpose = 0
  Input.wellformed = Yes and Input.command = No and Input.settingFails = No
  last.phase = Sent and last.route = 1 and last.agentsCalls = 1
}
pred WorkflowPlanningWitness {
  Input.global = 0 and Input.local = 1 and Input.purpose = 1
  Input.wellformed = Yes and Input.command = No and Input.settingFails = No
  last.phase = Sent and last.route = 0
}
pred TitleExclusionWitness {
  Input.global = 0 and Input.local = 1 and Input.purpose = 2
  Input.wellformed = Yes and Input.command = No
  last.phase = Configured and last.route = -1 and last.agentsCalls = 0
}
pred NoOverrideWitness {
  Input.global = -1 and Input.local = -1 and Input.purpose = 0
  Input.wellformed = Yes and Input.command = No
  last.phase = Sent and last.route = -1 and last.agentsCalls = 0
}
pred FailedSettingWitness {
  Input.global = 0 and Input.local = -1 and Input.purpose = 0
  Input.wellformed = Yes and Input.command = No and Input.settingFails = Yes
  last.phase = Failed and last.failed = Yes and last.agentsCalls = 1 and last.sent = No
}
pred MalformedRejectWitness {
  Input.global = 0 and Input.local = -1 and Input.purpose = 0
  Input.wellformed = No and Input.command = No
  last.phase = Failed and last.agentsCalls = 0 and last.sent = No
}
pred CommandRejectWitness {
  Input.global = 0 and Input.local = -1 and Input.purpose = 0
  Input.wellformed = Yes and Input.command = Yes
  last.phase = Failed and last.agentsCalls = 0 and last.sent = No
}
check RejectionBeforeSend for 8 State, 5 Int expect 0
check Selection for 8 State, 5 Int expect 0
check ConfigureBeforeSend for 8 State, 5 Int expect 0
check Scope for 8 State, 5 Int expect 0
run StepPrecedenceWitness for 8 State, 5 Int expect 1
run WorkflowPlanningWitness for 8 State, 5 Int expect 1
run TitleExclusionWitness for 8 State, 5 Int expect 1
run NoOverrideWitness for 8 State, 5 Int expect 1
run FailedSettingWitness for 8 State, 5 Int expect 1
run MalformedRejectWitness for 8 State, 5 Int expect 1
run CommandRejectWitness for 8 State, 5 Int expect 1
