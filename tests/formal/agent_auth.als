// Jcode provider preflight abstraction. Nine ordered states = eight transitions.
module agent_auth
open util/ordering[State]
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Time {}
one sig Unknown, Past, Future extends Time {}
abstract sig Target {}
one sig Anthropic, OpenAI, Unsupported extends Target {}
abstract sig Phase {}
one sig Checking, Authorized, Rejected, Dry, Launched extends Phase {}
abstract sig Account { access: one Time, refresh: one Bool }
one sig AnthropicFirst, AnthropicSecond, OpenAIFirst, OpenAISecond extends Account {}
one sig Config {
  planning, sdk, skip, dry, otherAuth: one Bool,
  planCandidates, executionCandidates: set Target
}
sig State { phase: one Phase, read, errors: set Target, workspace: one Bool }
pred expired[a: Account] { a.access = Past and a.refresh = No }
pred bypass { Config.skip = Yes or Config.sdk = No or Config.dry = Yes }
fun selected: set Target {
  ((Config.planning = Yes => Config.planCandidates else Config.executionCandidates)
    & (Anthropic + OpenAI)) - (Config.otherAuth = Yes => Anthropic else none)
}
fun required: set Target { bypass => none else selected }
fun unavailable: set Target {
  { t: Anthropic + OpenAI |
    (t = Anthropic and expired[AnthropicFirst] and expired[AnthropicSecond])
    or (t = OpenAI and expired[OpenAIFirst] and expired[OpenAISecond]) }
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
pred AllTargetsRejected {
  Config.planning = No and Config.sdk = Yes and Config.skip = No and Config.dry = No
  Config.otherAuth = No and Config.executionCandidates = Anthropic + OpenAI + Unsupported
  expired[AnthropicFirst] and expired[AnthropicSecond]
  expired[OpenAIFirst] and expired[OpenAISecond]
  last.phase = Rejected and last.read = Anthropic + OpenAI and last.errors = Anthropic + OpenAI
}
pred RenewableAccountLaunch {
  required = OpenAI and expired[OpenAIFirst]
  OpenAISecond.access = Past and OpenAISecond.refresh = Yes
  last.phase = Launched and last.read = OpenAI and no last.errors
}
pred UnknownAccountLaunch {
  required = OpenAI and expired[OpenAIFirst] and OpenAISecond.access = Unknown
  last.phase = Launched and no last.errors
}
pred PlanningIsolation {
  Config.planning = Yes and Config.sdk = Yes and Config.skip = No and Config.otherAuth = No
  Config.planCandidates = Anthropic and Config.executionCandidates = OpenAI
  AnthropicFirst.access = Future and expired[AnthropicSecond]
  last.phase = Launched and last.read = Anthropic and last.workspace = No
}
pred DryWithoutReads {
  Config.planning = No and Config.dry = Yes and Config.executionCandidates = Anthropic + OpenAI + Unsupported
  expired[AnthropicFirst] and expired[AnthropicSecond]
  expired[OpenAIFirst] and expired[OpenAISecond]
  last.phase = Dry and no last.read and last.workspace = No
}
pred PlanningDryWithoutReads {
  Config.planning = Yes and Config.dry = Yes
  Config.planCandidates = Anthropic + OpenAI + Unsupported
  expired[AnthropicFirst] and expired[AnthropicSecond]
  expired[OpenAIFirst] and expired[OpenAISecond]
  last.phase = Dry and no last.read and last.workspace = No
}
pred OptOutLaunch {
  Config.skip = Yes and Config.dry = No
  Config.executionCandidates = Anthropic + OpenAI + Unsupported
  expired[AnthropicFirst] and expired[AnthropicSecond]
  expired[OpenAIFirst] and expired[OpenAISecond]
  last.phase = Launched and no last.read
}
pred CommandLaunch {
  Config.sdk = No and Config.skip = No and Config.dry = No
  Config.planning = No and Config.executionCandidates = Anthropic + OpenAI + Unsupported
  expired[AnthropicFirst] and expired[AnthropicSecond]
  expired[OpenAIFirst] and expired[OpenAISecond]
  last.phase = Launched and no last.read
}
pred UnsupportedLaunch {
  Config.sdk = Yes and Config.skip = No and Config.dry = No and Config.otherAuth = No
  Config.planning = No and Config.executionCandidates = Unsupported
  expired[AnthropicFirst] and expired[AnthropicSecond]
  expired[OpenAIFirst] and expired[OpenAISecond]
  last.phase = Launched and no last.read
}
pred AlternativeAuthLaunch {
  Config.planning = No and Config.sdk = Yes and Config.skip = No and Config.dry = No
  Config.otherAuth = Yes and Config.executionCandidates = Anthropic + Unsupported
  expired[AnthropicFirst] and expired[AnthropicSecond]
  last.phase = Launched and no last.read
}
pred ExpiryBoundaryRejected {
  required = Anthropic
  AnthropicFirst.access = Past and AnthropicFirst.refresh = No
  AnthropicSecond.access = Past and AnthropicSecond.refresh = No
  last.phase = Rejected and last.errors = Anthropic
}
pred FreshAccessLaunch {
  required = Anthropic and AnthropicFirst.access = Future
  expired[AnthropicSecond]
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
run ExpiryBoundaryRejected for 9 State expect 1
run FreshAccessLaunch for 9 State expect 1
