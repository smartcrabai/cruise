// UI/approval boundary, separate from workflow_flags' run-boundary model.
// Seven explicitly ordered states, six transitions. Sparse sets encode true:
// absence and explicit false both evaluate off. Unknown flag keys may persist.
module flag_skip_selection
open util/ordering[State]
abstract sig Flag {}
one sig A, B, Legacy extends Flag {}
abstract sig Id {}
one sig Plain, Single, Either, Group, Child1, Child2, AfterPR, UnknownId extends Id {}
abstract sig Phase {}
one sig Menu, FlagPrompt, SkipPrompt, Chosen, Committed, Running, Cancelled extends Phase {}
abstract sig Event {}
one sig Init, Open, ChooseFlags, ChangeFlags, ChooseSkips, Commit, Create, Execute, Cancel, Idle extends Event {}
fun declared: set Flag { A + B }
fun known: set Id { Id - UnknownId }
// Resume positions are main-workflow expanded IDs, never call-site parents or after-pr.
fun mainExpanded: set Id { Plain + Single + Either + Child1 + Child2 }
// Fixed nonempty catalog exercises candidate filtering, not empty-config fallback.
one sig CreateRequest { values: set Flag, skips: set Id }
// Group and expanded child IDs inherit the call-site gate. AfterPR uses B.
fun gates: Id -> Flag {
  Single->A + Either->A + Either->B + Group->B + Child1->B + Child2->B + AfterPR->B
}
fun visible[on: set Flag]: set Id {
  { i: known | no i.gates or some (i.gates & on) }
}
fun prune[ids: set Id, on: set Flag]: set Id { ids - (known - visible[on]) }
sig State {
  phase: one Phase, event: one Event,
  flags, savedFlags, forwardedNames, forwardedOn: set Flag,
  skips, savedSkips, history, candidates, resume: set Id
}
// Every transition specifies all observables. There are no safety facts.
pred put[b: State, p: Phase, e: Event, f, sf: set Flag,
         s, ss, h: set Id, names, on: set Flag] {
  b.phase = p and b.event = e and b.flags = f and b.savedFlags = sf
  and b.skips = s and b.savedSkips = ss and b.history = h
  and b.candidates = visible[f] and b.resume = mainExpanded
  and b.forwardedNames = names and b.forwardedOn = on
}
pred initial[a: State] {
  a.flags in Flag and a.skips in Id and a.history in Id
  put[a,Menu,Init,a.flags,a.flags,a.skips,a.skips,a.history,none,none]
}
pred openPrompt[a,b: State] {
  a.phase = Menu
  put[b,FlagPrompt,Open,a.flags,a.savedFlags,a.skips,a.savedSkips,a.history,none,none]
}
pred flags[a,b: State] {
  a.phase in FlagPrompt + SkipPrompt
  b.flags - declared = a.flags - declared
  put[b,SkipPrompt,(a.phase = FlagPrompt => ChooseFlags else ChangeFlags),
    b.flags,a.savedFlags,prune[a.skips,b.flags],a.savedSkips,a.history,none,none]
}
pred selectSkips[a,b: State] {
  a.phase = SkipPrompt
  // Known choices must be displayed. Unknown text-input IDs survive pruning.
  b.skips in a.candidates + (a.skips - known)
  put[b,Chosen,ChooseSkips,a.flags,a.savedFlags,b.skips,a.savedSkips,a.history,none,none]
}
pred commit[a,b: State] {
  a.phase = Chosen
  put[b,Committed,Commit,a.flags,a.flags,prune[a.skips,a.flags],
    prune[a.skips,a.flags],prune[a.skips,a.flags],none,none]
}
pred create[a,b: State] {
  a.phase = Menu
  // Application validates request names before pruning or persisting anything.
  CreateRequest.values in declared
  put[b,Committed,Create,CreateRequest.values,CreateRequest.values,
    prune[CreateRequest.skips,CreateRequest.values],
    prune[CreateRequest.skips,CreateRequest.values],
    prune[CreateRequest.skips,CreateRequest.values],none,none]
}
pred execute[a,b: State] {
  a.phase = Committed
  // Explicit assignments include false declared names, excluding legacy keys.
  put[b,Running,Execute,a.flags,a.savedFlags,a.skips,a.savedSkips,a.history,
    declared,a.flags & declared]
}
pred cancel[a,b: State] {
  a.phase in FlagPrompt + SkipPrompt
  put[b,Cancelled,Cancel,a.savedFlags,a.savedFlags,a.savedSkips,a.savedSkips,a.history,none,none]
}
pred idle[a,b: State] {
  put[b,a.phase,Idle,a.flags,a.savedFlags,a.skips,a.savedSkips,a.history,a.forwardedNames,a.forwardedOn]
}
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] |
    openPrompt[a,b] or flags[a,b] or selectSkips[a,b] or commit[a,b]
    or create[a,b] or execute[a,b] or cancel[a,b] or idle[a,b]
}
// should_prompt_flags is false when explicit declared assignments are present.
pred nextRunPrompts[a: State] { no a.forwardedNames and some declared }
assert CandidateSemantics {
  all a: State |
    Plain in a.candidates and UnknownId not in a.candidates
    and (Single in a.candidates iff A in a.flags)
    and (Either in a.candidates iff some (a.flags & (A+B)))
    and (AfterPR in a.candidates iff B in a.flags)
    and (Group in a.candidates iff B in a.flags)
    and (Child1 in a.candidates iff Group in a.candidates)
    and (Child2 in a.candidates iff Group in a.candidates)
}
assert OrderAndPruning {
  all a: State - last | let b = next[a] |
    (b.event = ChooseSkips implies a.phase = SkipPrompt)
    and (b.event in ChooseFlags + ChangeFlags implies
      b.skips = a.skips - (known - b.candidates)
      and b.skips - known = a.skips - known)
    and (b.phase in Chosen + Committed + Running implies b.skips & known in b.candidates)
}
assert AtomicPersistence {
  all a: State - last | let b = next[a] |
    (b.event not in Commit + Create implies b.savedFlags = a.savedFlags
      and b.savedSkips = a.savedSkips and b.history = a.history)
    and (b.event = Cancel implies b.savedFlags = first.savedFlags
      and b.savedSkips = first.savedSkips and b.history = first.history)
}
assert CommittedSelection {
  all a: State | a.phase in Committed + Running implies
    a.savedFlags = a.flags and a.savedSkips = a.skips
    and a.history = a.skips and a.skips = prune[a.skips,a.flags]
}
assert ExecuteForwarding {
  all a: State | a.phase = Running implies
    a.forwardedNames = declared and a.forwardedOn = a.flags & declared
    and Legacy not in a.forwardedNames and not nextRunPrompts[a]
}
assert ResumeUnfiltered { all a: State | a.resume = mainExpanded }
assert ApplicationCreate {
  all a: State | a.event = Create implies
    a.flags = CreateRequest.values and a.flags in declared
    and a.savedSkips = CreateRequest.skips - (known - a.candidates)
    and a.savedSkips - known = CreateRequest.skips - known
    and a.history = a.savedSkips
  (some (CreateRequest.values - declared)) implies no a: State | a.event = Create
}
pred ApplicationCreateWitness {
  CreateRequest.values = A
  CreateRequest.skips = Single + Group + Child1 + Child2 + AfterPR + UnknownId
  some a: State | a.event = Create and a.skips = Single + UnknownId
    and a.history = Single + UnknownId and a.resume = mainExpanded
}
pred InvalidCreateWitness {
  CreateRequest.values = Legacy
  some a: State | a.phase = Committed
  no a: State | a.event = Create
}
pred AbsentOffWitness {
  no first.flags
  first.candidates = Plain and first.resume = mainExpanded
  some a: State | a.event = Commit and a.skips = Plain
}
pred OrAndAfterPRWitness {
  some a: State | a.event = ChooseFlags and a.flags = B
    and a.candidates = Plain + Either + Group + Child1 + Child2 + AfterPR
  some a: State | a.phase = Committed and a.skips = Either + AfterPR
}
pred GroupPruningWitness {
  first.flags = B + Legacy
  first.skips = Group + Child1 + Child2 + UnknownId
  some a: State | a.event = ChooseFlags and B in a.flags
  some a: State | a.event = ChangeFlags and a.flags = Legacy and a.skips = UnknownId
  some a: State | a.phase = Running and a.history = UnknownId and no a.forwardedOn
}
pred SkipCancellationWitness {
  first.flags = B and first.skips = Group and first.history = AfterPR
  some a: State | a.event = ChooseFlags and a.flags = A and no a.skips
  some a: State | a.phase = Cancelled and a.savedFlags = B
    and a.savedSkips = Group and a.history = AfterPR
}
pred FlagCancellationWitness {
  some a: State | a.event = Cancel
  no a: State | a.event in ChooseFlags + ChooseSkips + Commit
}
pred ExecuteWitness {
  first.flags = Legacy
  some a: State | a.phase = Running and a.flags = B + Legacy
    and a.forwardedNames = A+B and a.forwardedOn = B
    and a.history = Group + Child1 + Child2
}
check ApplicationCreate for 10 but exactly 7 State, 1 Int expect 0
run ApplicationCreateWitness for 10 but exactly 7 State, 1 Int expect 1
run InvalidCreateWitness for 10 but exactly 7 State, 1 Int expect 1
check CandidateSemantics for 10 but exactly 7 State, 1 Int expect 0
check OrderAndPruning for 10 but exactly 7 State, 1 Int expect 0
check AtomicPersistence for 10 but exactly 7 State, 1 Int expect 0
check CommittedSelection for 10 but exactly 7 State, 1 Int expect 0
check ExecuteForwarding for 10 but exactly 7 State, 1 Int expect 0
check ResumeUnfiltered for 10 but exactly 7 State, 1 Int expect 0
run AbsentOffWitness for 10 but exactly 7 State, 1 Int expect 1
run OrAndAfterPRWitness for 10 but exactly 7 State, 1 Int expect 1
run GroupPruningWitness for 10 but exactly 7 State, 1 Int expect 1
run SkipCancellationWitness for 10 but exactly 7 State, 1 Int expect 1
run FlagCancellationWitness for 10 but exactly 7 State, 1 Int expect 1
run ExecuteWitness for 10 but exactly 7 State, 1 Int expect 1
