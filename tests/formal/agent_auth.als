// Same finite abstraction as agent_auth.qnt, 9 ordered states = 8 transitions.
// Expiry compares to one observation instant, including equality in Past.
module agent_auth
open util/ordering[State]
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Time {}
one sig Unknown, Past, Future extends Time {}
abstract sig Target {}
one sig Claude, Jcode, Unsupported extends Target {}
abstract sig Phase {}
one sig Checking, Authorized, Rejected, Dry, Launched extends Phase {}
abstract sig Account { access, refreshExpiry: one Time, refresh: one Bool }
one sig ClaudeAccount, FirstAccount, SecondAccount extends Account {}
// Neither jcode OAuth file supplies a refresh-expiry timestamp.
fact CredentialSchema { (FirstAccount + SecondAccount).refreshExpiry = Unknown }
one sig Config {
  planning, sdk, skip, dry, otherAuth: one Bool,
  planCandidates, executionCandidates: set Target
}
sig State { phase: one Phase, read, errors: set Target, workspace: one Bool }
pred expired[a: Account] { a.access = Past and (a.refresh = No or a.refreshExpiry = Past) }
pred bypass { Config.skip = Yes or Config.sdk = No or Config.dry = Yes }
fun selected: set Target {
  ((Config.planning = Yes => Config.planCandidates else Config.executionCandidates)
    & (Claude + Jcode)) - (Config.otherAuth = Yes => Claude else none)
}
fun required: set Target { bypass => none else selected }
fun unavailable: set Target {
  { t: Claude + Jcode | (t = Claude and expired[ClaudeAccount])
    or (t = Jcode and expired[FirstAccount] and expired[SecondAccount]) }
}
pred initial[a: State] { a.phase = Checking and no a.read and no a.errors and a.workspace = No }
pred inspect[a,b: State] {
  a.phase = Checking
  some t: required - a.read | {
    b.read = a.read + t
    b.errors = a.errors + (t & unavailable)
  }
  b.phase = Checking and b.workspace = a.workspace
}
pred decide[a,b: State] {
  a.phase = Checking and a.read = required
  b.phase = (some a.errors => Rejected else
    (Config.dry = Yes => Dry else Authorized))
  b.read = a.read and b.errors = a.errors and b.workspace = a.workspace
}
pred launch[a,b: State] {
  a.phase = Authorized and b.phase = Launched
  b.read = a.read and b.errors = a.errors
  b.workspace = (Config.planning = Yes => No else Yes)
}
pred idle[a,b: State] {
  b.phase = a.phase and b.read = a.read and b.errors = a.errors and b.workspace = a.workspace
}
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] | inspect[a,b] or decide[a,b] or launch[a,b] or idle[a,b]
}
assert ScopeSafety { all a: State | a.read in required }
assert ExactDiagnostics { all a: State | a.errors = a.read & unavailable }
assert CompleteBeforeDecision { all a: State | a.phase != Checking implies a.read = required }
assert Admission {
  all a: State | {
    a.phase in Authorized + Launched + Dry implies no (required & unavailable)
    a.phase = Rejected iff (a.phase != Checking and some (required & unavailable))
  }
}
assert EffectGate {
  all a: State | {
    a.workspace = Yes implies (a.phase = Launched and Config.planning = No)
    Config.dry = Yes implies (a.phase != Launched and a.workspace = No)
    a.phase = Rejected implies a.workspace = No
  }
}
// A denied trace must actually read both expired targets and aggregate both.
pred AllTargetsRejected {
  Config.planning = No and Config.sdk = Yes and Config.skip = No and Config.dry = No
  Config.otherAuth = No and Config.executionCandidates = Target
  expired[ClaudeAccount] and expired[FirstAccount] and expired[SecondAccount]
  last.phase = Rejected and last.read = Claude + Jcode and last.errors = Claude + Jcode
}
// One renewable account suffices, despite another account being dead.
pred RenewableAccountLaunch {
  required = Jcode and expired[FirstAccount]
  SecondAccount.access = Past and SecondAccount.refresh = Yes and SecondAccount.refreshExpiry = Unknown
  last.phase = Launched and last.read = Jcode and no last.errors
}
pred UnknownAccountLaunch {
  required = Jcode and expired[FirstAccount] and SecondAccount.access = Unknown
  last.phase = Launched and no last.errors
}
pred PlanningIsolation {
  Config.planning = Yes and Config.sdk = Yes and Config.skip = No and Config.otherAuth = No
  Config.planCandidates = Claude and Config.executionCandidates = Jcode
  ClaudeAccount.access = Future and expired[FirstAccount] and expired[SecondAccount]
  last.phase = Launched and last.read = Claude and last.workspace = No
}
pred DryWithoutReads {
  Config.planning = No and Config.dry = Yes and Config.executionCandidates = Target
  expired[ClaudeAccount] and expired[FirstAccount] and expired[SecondAccount]
  last.phase = Dry and no last.read and last.workspace = No
}
pred PlanningDryWithoutReads {
  Config.planning = Yes and Config.dry = Yes and Config.planCandidates = Target
  expired[ClaudeAccount] and expired[FirstAccount] and expired[SecondAccount]
  last.phase = Dry and no last.read and last.workspace = No
}
pred OptOutLaunch {
  Config.skip = Yes and Config.dry = No and Config.executionCandidates = Target
  expired[ClaudeAccount] and expired[FirstAccount] and expired[SecondAccount]
  last.phase = Launched and no last.read
}
pred CommandLaunch {
  Config.sdk = No and Config.skip = No and Config.dry = No
  Config.planning = No and Config.executionCandidates = Target
  expired[ClaudeAccount] and expired[FirstAccount] and expired[SecondAccount]
  last.phase = Launched and no last.read
}
pred UnsupportedLaunch {
  Config.sdk = Yes and Config.skip = No and Config.dry = No and Config.otherAuth = No
  Config.planning = No and Config.executionCandidates = Unsupported
  expired[ClaudeAccount] and expired[FirstAccount] and expired[SecondAccount]
  last.phase = Launched and no last.read
}
pred AlternativeAuthLaunch {
  Config.planning = No and Config.sdk = Yes and Config.skip = No and Config.dry = No
  Config.otherAuth = Yes and Config.executionCandidates = Claude + Unsupported
  expired[ClaudeAccount] and last.phase = Launched and no last.read
}
pred RefreshExpiryRejected {
  required = Claude and ClaudeAccount.access = Past
  ClaudeAccount.refresh = Yes and ClaudeAccount.refreshExpiry = Past
  last.phase = Rejected and last.errors = Claude
}
pred FreshAccessLaunch {
  required = Claude and ClaudeAccount.access = Future
  ClaudeAccount.refresh = Yes and ClaudeAccount.refreshExpiry = Past
  last.phase = Launched and no last.errors
}
check ScopeSafety for 9 State expect 0
check ExactDiagnostics for 9 State expect 0
check CompleteBeforeDecision for 9 State expect 0
check Admission for 9 State expect 0
check EffectGate for 9 State expect 0
run AllTargetsRejected for 9 State expect 1
run RenewableAccountLaunch for 9 State expect 1
run UnknownAccountLaunch for 9 State expect 1
run PlanningIsolation for 9 State expect 1
run DryWithoutReads for 9 State expect 1
run PlanningDryWithoutReads for 9 State expect 1
run OptOutLaunch for 9 State expect 1
run CommandLaunch for 9 State expect 1
run UnsupportedLaunch for 9 State expect 1
run AlternativeAuthLaunch for 9 State expect 1
run RefreshExpiryRejected for 9 State expect 1
run FreshAccessLaunch for 9 State expect 1
