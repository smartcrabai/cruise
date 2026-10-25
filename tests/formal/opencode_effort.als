// Same observable contract as opencode_effort.qnt, not an HTTP implementation.
// cooled is the abandoned-and-cooled identity observer, not the full process map.
// Exactly 13 states = 12 transitions. All counters are 0..2 in signed 4-bit Int.
module opencode_effort
open util/ordering[State]
open util/integer
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Kind {}
one sig Plain, Effort, NativeRequest, Invalid extends Kind {}
abstract sig Profile {}
one sig Unlisted, Empty, CanonicalAlias, UniqueAlias, Ambiguous,
  ConflictAlias, ApiFailure extends Profile {}
abstract sig Variant {}
one sig Canonical, AliasA, AliasB, Native extends Variant {}
abstract sig Model { profile: one Profile }
one sig Primary, Spare extends Model {}
one sig Config { kind: one Kind, resume, policy, budget, enabled, candidate: one Bool }
abstract sig Phase {}
one sig Setup, Ready, Resolved, Selected, Executing, Missing,
  Success, Rejected, Permanent extends Phase {}
abstract sig Event {}
one sig Init, SetReady, Resolve, RejectSyntax, RejectCatalog, Create,
  ResumeUpdate, Submit, Barrier, NoRoute, AuthFailure, Succeed, Fallback, Idle extends Event {}
sig State {
  phase: one Phase, model: one Model, chosen: lone Variant,
  ready, barrier: one Bool, cooled: set Model,
  catalogs, selections, prompts: one Int, event: one Event
}
fun compatible[p: Profile]: set Variant {
  p = CanonicalAlias => Canonical + AliasA else
  (p in UniqueAlias + ConflictAlias => AliasA else
  (p = Ambiguous => AliasA + AliasB else none))
}
fun allowed[p: Profile]: set Variant {
  Canonical in compatible[p] => Canonical else
  (#compatible[p] = 1 => compatible[p] else none)
}
pred assign[b: State, ph: Phase, m: Model, v: set Variant, r, bar: Bool,
  cool: set Model, c, w, n: Int] {
  b.phase = ph and b.model = m and b.chosen = v and b.ready = r
  b.barrier = bar and b.cooled = cool
  b.catalogs = c and b.selections = w and b.prompts = n
}
pred identical[a,b: State] {
  assign[b,a.phase,a.model,a.chosen,a.ready,a.barrier,a.cooled,
    a.catalogs,a.selections,a.prompts]
}
pred initial[a: State] {
  assign[a,Setup,Primary,none,No,No,none,0,0,0] and a.event = Init
}
pred ready[a,b: State] {
  a.phase = Setup and Config.kind != Invalid
  assign[b,Ready,a.model,a.chosen,Yes,a.barrier,a.cooled,a.catalogs,a.selections,a.prompts]
  b.event = SetReady
}
pred rejectSyntax[a,b: State] {
  a.phase = Setup and Config.kind = Invalid
  assign[b,Rejected,a.model,a.chosen,a.ready,Yes,a.cooled,a.catalogs,a.selections,a.prompts]
  b.event = RejectSyntax
}
pred resolve[a,b: State] {
  a.phase = Ready
  (Config.kind = Effort and some allowed[a.model.profile] and b.chosen in allowed[a.model.profile] and one b.chosen)
    or (Config.kind = NativeRequest and b.chosen = Native)
    or (Config.kind = Plain and no b.chosen)
  assign[b,Resolved,a.model,b.chosen,a.ready,a.barrier,a.cooled,
    (Config.kind = Effort => a.catalogs.add[1] else a.catalogs),a.selections,a.prompts]
  b.event = Resolve
}
pred rejectCatalog[a,b: State] {
  a.phase = Ready and Config.kind = Effort and no allowed[a.model.profile]
  assign[b,Rejected,a.model,a.chosen,a.ready,Yes,a.cooled,a.catalogs.add[1],a.selections,a.prompts]
  b.event = RejectCatalog
}
pred selectModel[a,b: State] {
  a.phase = Resolved
  assign[b,Selected,a.model,a.chosen,a.ready,a.barrier,a.cooled,a.catalogs,a.selections.add[1],a.prompts]
  b.event = (Config.resume = Yes => ResumeUpdate else Create)
}
pred submit[a,b: State] {
  a.phase = Selected
  assign[b,Executing,a.model,a.chosen,a.ready,a.barrier,a.cooled,a.catalogs,a.selections,a.prompts.add[1]]
  b.event = Submit
}
pred barrier[a,b: State] {
  a.phase = Executing
  assign[b,Executing,a.model,a.chosen,a.ready,Yes,a.cooled,a.catalogs,a.selections,a.prompts]
  b.event = Barrier
}
pred finish[a,b: State] {
  a.phase = Executing
  (b.phase = Missing and b.event = NoRoute)
    or (b.phase = Permanent and b.event = AuthFailure)
    or (b.phase = Success and b.event = Succeed)
  assign[b,b.phase,a.model,a.chosen,a.ready,a.barrier,a.cooled,a.catalogs,a.selections,a.prompts]
}
pred fallback[a,b: State] {
  a.phase = Missing and a.model = Primary and a.barrier = No
  Config.policy = Yes and Config.budget = Yes and Config.enabled = Yes and Config.candidate = Yes
  assign[b,Setup,Spare,none,No,a.barrier,a.cooled + Primary,a.catalogs,a.selections,a.prompts]
  b.event = Fallback
}
pred idle[a,b: State] { identical[a,b] and b.event = Idle }
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] |
    ready[a,b] or rejectSyntax[a,b] or resolve[a,b] or rejectCatalog[a,b]
    or selectModel[a,b] or submit[a,b] or barrier[a,b] or finish[a,b]
    or fallback[a,b] or idle[a,b]
}
assert CapabilitySafety {
  all a: State |
    (Config.kind = Effort and a.phase in Resolved + Selected + Executing + Missing + Success
      implies a.chosen in allowed[a.model.profile] and one a.chosen)
    and (Config.kind != Effort implies a.catalogs = 0)
    and a.prompts <= a.selections
  all a: State - last | let b = next[a] |
    (b.event in Resolve + RejectCatalog implies a.ready = Yes)
    and (b.event in RejectCatalog + RejectSyntax implies b.prompts = a.prompts and b.selections = a.selections)
    and (b.event in Create + ResumeUpdate implies a.phase = Resolved and a.ready = Yes)
    and (b.event = Submit implies a.phase = Selected and a.ready = Yes)
}
assert ReplayAndFallbackSafety {
  all a: State | a.model not in a.cooled and a.prompts >= 0 and a.prompts <= 2
    and a.selections >= 0 and a.selections <= 2 and a.catalogs >= 0 and a.catalogs <= 2
  all a: State - last | let b = next[a] |
    (b.event = Fallback implies a.phase = Missing and a.model = Primary and a.barrier = No
      and Config.policy = Yes and Config.budget = Yes and Config.enabled = Yes and Config.candidate = Yes
      and b.model = Spare and Primary in b.cooled and no b.chosen and b.ready = No)
    and (a.barrier = Yes implies b.barrier = Yes and b.prompts = a.prompts)
    and a.cooled in b.cooled
    and (b.event != Submit implies b.prompts = a.prompts)
    and (b.event not in Create + ResumeUpdate implies b.selections = a.selections)
    and (a.phase in Success + Rejected + Permanent implies identical[a,b])
}
pred CanonicalPriorityWitness {
  Config.kind = Effort and Primary.profile = CanonicalAlias
  last.phase = Success and last.model = Primary and last.chosen = Canonical and last.prompts = 1
}
pred ResumeAliasWitness {
  Config.kind = Effort and Config.resume = Yes and Primary.profile = ConflictAlias
  last.phase = Success and last.chosen = AliasA and last.prompts = 1
  some a: State | a.event = ResumeUpdate
}
pred RejectedResumeWitness {
  Config.kind = Effort and Config.resume = Yes and Primary.profile = Ambiguous
  last.phase = Rejected and last.prompts = 0 and last.selections = 0 and last.catalogs = 1
}
pred ApiFailureWitness {
  Config.kind = Effort and Primary.profile = ApiFailure
  last.phase = Rejected and last.prompts = 0 and last.selections = 0
}
pred NativeBypassWitness {
  Config.kind = NativeRequest and Primary.profile = ApiFailure
  last.phase = Success and last.chosen = Native and last.catalogs = 0
}
pred FallbackRechecksWitness {
  Config.kind = Effort and Primary.profile = CanonicalAlias and Spare.profile = UniqueAlias
  last.phase = Success and last.model = Spare and last.chosen = AliasA
  last.prompts = 2 and last.catalogs = 2 and last.cooled = Primary
}
pred BarrierWitness {
  Config.kind = Effort and Primary.profile = CanonicalAlias
  Config.policy = Yes and Config.budget = Yes and Config.enabled = Yes and Config.candidate = Yes
  last.phase = Missing and last.model = Primary and last.prompts = 1 and last.barrier = Yes
}
pred AuthWitness {
  Config.kind = Effort and Primary.profile = CanonicalAlias
  last.phase = Permanent and last.model = Primary and last.prompts = 1
}
check CapabilitySafety for 13 State, 4 Int expect 0
check ReplayAndFallbackSafety for 13 State, 4 Int expect 0
run CanonicalPriorityWitness for 13 State, 4 Int expect 1
run ResumeAliasWitness for 13 State, 4 Int expect 1
run RejectedResumeWitness for 13 State, 4 Int expect 1
run ApiFailureWitness for 13 State, 4 Int expect 1
run NativeBypassWitness for 13 State, 4 Int expect 1
run FallbackRechecksWitness for 13 State, 4 Int expect 1
run BarrierWitness for 13 State, 4 Int expect 1
run AuthWitness for 13 State, 4 Int expect 1
