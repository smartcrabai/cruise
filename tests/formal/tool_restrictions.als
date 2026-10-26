// Same finite observable policy contract as tool_restrictions.qnt.
// One git request abstracts the agreed implicit prefix list, not all git writes.
module tool_restrictions
open util/ordering[State]
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Restriction {}
one sig Web, Subagent, AllMcp, McpA, Publish, Git extends Restriction {}
abstract sig Request {}
one sig WebCall, AgentCall, CallA, CallB, PublishCall, GitCall, SafeShell extends Request {}
abstract sig Phase {}
one sig Configured, Ready, Rejected, Blocked, Allowed extends Phase {}
one sig Config {
  defaults, workflow, child: set Restriction,
  present, full, commit, sdk: one Bool
}
// Existing permission validation precedes this contract.
fact ValidPermission { Config.sdk = No implies Config.full = Yes }
sig State {
  phase: one Phase, denied: set Request, mcp: set Request,
  request: lone Request, effect: one Bool
}
fun explicit: set Restriction {
  (Config.present = Yes => Config.workflow else Config.defaults) + Config.child
}
fun prohibited: set Request {
  {r: Request |
    (r = WebCall and Web in explicit) or
    (r = AgentCall and Subagent in explicit) or
    (r = CallA and some (AllMcp + McpA) & explicit) or
    (r = CallB and AllMcp in explicit) or
    (r = PublishCall and (Config.full = No or Publish in explicit)) or
    (r = GitCall and (Config.full = No or Config.commit = No or Git in explicit)) or
    (r = SafeShell and Config.full = No)}
}
pred initial[a: State] {
  a.phase = Configured and no a.denied + a.mcp + a.request and a.effect = No
}
pred resolve[a,b: State] {
  a.phase = Configured and (Config.sdk = Yes or no explicit)
  b.phase = Ready and no b.request and b.effect = No
  b.denied = (Config.sdk = Yes => prohibited else none)
  b.mcp = CallA + CallB - b.denied
}
pred reject[a,b: State] {
  a.phase = Configured and Config.sdk = No and some explicit
  b.phase = Rejected and b.denied = a.denied and b.mcp = a.mcp
  b.request = a.request and b.effect = No
}
pred attempt[a,b: State] {
  a.phase = Ready and one b.request
  b.denied = a.denied and b.mcp = a.mcp
  (b.request in a.denied implies b.phase = Blocked and b.effect = No)
  (b.request not in a.denied implies b.phase = Allowed and b.effect = Yes)
}
pred idle[a,b: State] {
  b.phase = a.phase and b.denied = a.denied and b.mcp = a.mcp
  b.request = a.request and b.effect = a.effect
}
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] |
    resolve[a,b] or reject[a,b] or attempt[a,b] or idle[a,b]
}
assert DenyOnly {
  all a: State | a.phase in Ready + Blocked + Allowed and Config.sdk = Yes implies
    (Web in explicit implies WebCall in a.denied) and
    (Subagent in explicit implies AgentCall in a.denied) and
    (Publish in explicit implies PublishCall in a.denied) and
    (Git in explicit implies GitCall in a.denied) and
    (Config.full = No implies PublishCall + GitCall + SafeShell in a.denied) and
    (Config.commit = No implies GitCall in a.denied)
}
assert ExactDecisions {
  all a: State | a.phase in Ready + Blocked + Allowed implies
    a.denied = (Config.sdk = Yes => prohibited else none)
}
assert McpScope {
  all a: State | a.phase in Ready + Blocked + Allowed implies
    a.mcp = CallA + CallB - a.denied
}
assert CommandAdmission {
  all a: State |
    (a.phase in Ready + Blocked + Allowed and Config.sdk = No implies no explicit and no a.denied) and
    (a.phase = Rejected implies Config.sdk = No and some explicit and a.effect = No)
}
assert EffectGate {
  all a: State |
    (a.effect = Yes iff a.phase = Allowed) and
    (a.phase in Blocked + Allowed implies (a.effect = Yes iff a.request not in a.denied)) and
    (a.effect = Yes and Config.sdk = Yes implies a.request not in prohibited)
}
pred UnionBlockedWitness {
  Config.sdk = Yes and Config.present = No and Config.defaults = Web
  Config.child = Subagent + McpA + Publish and Config.full = Yes and Config.commit = No
  some a: State | a.phase = Blocked and a.request = GitCall and a.mcp = CallB
    and a.denied = WebCall + AgentCall + CallA + PublishCall + GitCall
}
pred SafeShellWitness {
  Config.sdk = Yes and Config.full = Yes and Config.commit = No
  some a: State | a.phase = Allowed and a.request = SafeShell
}
pred EmptyOverrideWitness {
  Config.sdk = Yes and Config.present = Yes and no Config.workflow + Config.child
  Config.defaults = Web and Config.full = Yes and Config.commit = Yes
  some a: State | a.phase = Allowed and a.request = WebCall and no a.denied
}
pred ExplicitSurvivesCommitWitness {
  Config.sdk = Yes and Config.commit = Yes and Config.full = Yes and Git in Config.child
  some a: State | a.phase = Blocked and a.request = GitCall
}
pred RestrictedWitness {
  Config.sdk = Yes and Config.commit = Yes and Config.full = No
  some a: State | a.phase = Blocked and a.request = SafeShell
}
pred AllMcpWitness {
  Config.sdk = Yes and AllMcp in Config.child
  some a: State | a.phase = Ready and no a.mcp
}
pred CommandRejectWitness {
  Config.sdk = No and Web in Config.child
  some a: State | a.phase = Rejected
}
pred CommandImplicitIgnoredWitness {
  Config.sdk = No and no Config.defaults + Config.workflow + Config.child
  Config.commit = No and Config.full = Yes
  some a: State | a.phase = Allowed and a.request = GitCall
}
check DenyOnly for 7 but exactly 5 State expect 0
check ExactDecisions for 7 but exactly 5 State expect 0
check McpScope for 7 but exactly 5 State expect 0
check CommandAdmission for 7 but exactly 5 State expect 0
check EffectGate for 7 but exactly 5 State expect 0
run UnionBlockedWitness for 7 but exactly 5 State expect 1
run SafeShellWitness for 7 but exactly 5 State expect 1
run EmptyOverrideWitness for 7 but exactly 5 State expect 1
run ExplicitSurvivesCommitWitness for 7 but exactly 5 State expect 1
run RestrictedWitness for 7 but exactly 5 State expect 1
run AllMcpWitness for 7 but exactly 5 State expect 1
run CommandRejectWitness for 7 but exactly 5 State expect 1
run CommandImplicitIgnoredWitness for 7 but exactly 5 State expect 1
