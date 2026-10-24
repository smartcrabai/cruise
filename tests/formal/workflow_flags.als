// Same run-boundary abstraction as workflow_flags.qnt. Exactly 9 ordered states
// give 8 transitions. Explicit off is represented by present minus enabled.
module workflow_flags
open util/ordering[State]
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Flag {}
one sig E2E, Docs, Unknown extends Flag {}
abstract sig Phase {}
one sig Preparing, Prompting, Ready, Running, Cancelled, Rejected, Dry extends Phase {}
abstract sig Event {}
one sig Init, Prepare, Reject, Select, Cancel, Start, Idle extends Event {}
one sig Input {
  dry, single, fresh, tty, valid, exec: one Bool,
  declared, cli, cliOn: set Flag
}
fact Inputs { Input.declared in E2E + Docs and Input.cli in Input.declared and Input.cliOn in Input.cli }
sig State {
  phase: one Phase, enabled, present, saved, savedPresent, engine, selected: set Flag,
  exists: one Bool, prompts: one Int, event: one Event
}
pred eligible {
  Input.dry = No and no Input.cli and Input.single = Yes and Input.fresh = Yes
  and some Input.declared and Input.tty = Yes
}
pred assign[b: State, p: Phase, on, keys, disk, diskKeys: set Flag,
            alive: Bool, count: Int, engineOn, chosen: set Flag] {
  b.phase = p and b.enabled = on and b.present = keys and b.saved = disk
  and b.savedPresent = diskKeys and b.exists = alive and b.prompts = count
  and b.engine = engineOn and b.selected = chosen
}
pred initial[a: State] {
  a.phase = Preparing and a.enabled in a.present and a.saved = a.enabled
  and a.savedPresent = a.present and a.exists = Yes and a.prompts = 0
  and no a.engine and no a.selected and a.event = Init
}
pred prepare[a,b: State] {
  a.phase = Preparing and Input.valid = Yes
  assign[b, (eligible => Prompting else (Input.dry = Yes => Dry else Ready)),
    (a.enabled - Input.cli) + Input.cliOn, a.present + Input.cli, a.saved,
    a.savedPresent, a.exists, (eligible => a.prompts.plus[1] else a.prompts), a.engine, a.selected]
  b.event = Prepare
}
pred reject[a,b: State] {
  a.phase = Preparing and Input.valid = No
  assign[b,Rejected,a.enabled,a.present,a.saved,a.savedPresent,
    (Input.exec = Yes => No else Yes),a.prompts,a.engine,a.selected]
  b.event = Reject
}
pred selectFlags[a,b: State] {
  a.phase = Prompting and b.selected in Input.declared
  assign[b,Ready,(a.enabled - Input.declared) + b.selected,
    a.present + Input.declared,a.saved,a.savedPresent,a.exists,a.prompts,a.engine,b.selected]
  b.event = Select
}
pred cancel[a,b: State] {
  a.phase = Prompting
  assign[b,Cancelled,a.enabled,a.present,a.saved,a.savedPresent,
    (Input.exec = Yes => No else Yes),a.prompts,a.engine,a.selected]
  b.event = Cancel
}
pred start[a,b: State] {
  a.phase = Ready
  assign[b,Running,a.enabled,a.present,a.enabled,a.present,a.exists,a.prompts,a.enabled,a.selected]
  b.event = Start
}
pred idle[a,b: State] {
  assign[b,a.phase,a.enabled,a.present,a.saved,a.savedPresent,a.exists,a.prompts,a.engine,a.selected]
  b.event = Idle
}
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] |
    prepare[a,b] or reject[a,b] or selectFlags[a,b] or cancel[a,b] or start[a,b] or idle[a,b]
}
assert PromptGate {
  all a: State | a.prompts >= 0 and a.prompts <= 1
    and (a.phase = Prompting implies eligible and Input.valid = Yes and a.prompts = 1)
    and (a.event in Select + Cancel implies eligible and Input.valid = Yes)
    and (not eligible implies a.prompts = 0)
}
assert ExplicitSelection {
  all a: State - last | let b = next[a] | b.event = Select implies
    (b.enabled & Input.declared = b.selected and Input.declared in b.present
     and b.enabled - Input.declared = a.enabled - Input.declared
     and b.present - Input.declared = a.present - Input.declared)
}
assert CliAuthority {
  all a: State - last | let b = next[a] | b.event = Prepare implies
    (b.enabled & Input.cli = Input.cliOn and Input.cli in b.present
     and b.enabled - Input.cli = a.enabled - Input.cli)
}
assert Persistence {
  all a: State | (a.phase = Running implies a.saved = a.engine and a.enabled = a.engine)
    and (a.phase = Dry implies a.prompts = 0 and no a.engine)
  all a: State - last | let b = next[a] |
    (b.event != Start implies b.saved = a.saved and b.savedPresent = a.savedPresent)
    and (b.phase = Cancelled implies b.saved = a.saved and b.exists = (Input.exec = Yes => No else Yes))
}
assert Domains {
  all a: State | a.enabled in a.present and a.saved in a.savedPresent
}
pred SelectionWitness {
  Input.declared = E2E + Docs and Input.exec = No and first.enabled = Docs + Unknown
  some a: State | a.event = Select and a.selected = E2E
  some a: State | a.phase = Running and a.saved = E2E + Unknown and a.present = Flag
}
pred EmptyWitness {
  first.enabled = Flag and Input.declared = E2E + Docs
  some a: State | a.event = Select and no a.selected
  some a: State | a.phase = Running and a.saved = Unknown
}
pred ExplicitOffWitness {
  Input.cli = E2E and no Input.cliOn and first.enabled = E2E + Docs
  Input.valid = Yes and Input.dry = No and Input.tty = Yes
  some a: State | a.phase = Running and a.saved = Docs and a.prompts = 0
}
pred CancellationWitness {
  Input.exec = No and first.enabled = Docs
  some a: State | a.phase = Cancelled and a.exists = Yes and a.saved = Docs
}
pred ExecCancellationWitness {
  Input.exec = Yes
  some a: State | a.phase = Cancelled and a.exists = No and no a.engine
}
pred PreflightWitness {
  Input.valid = No and Input.exec = No
  some a: State | a.phase = Rejected and a.prompts = 0 and a.exists = Yes
}
pred SuppressedWitness {
  Input.valid = Yes and Input.dry = No and no Input.cli and some Input.declared
  (Input.tty = No or Input.single = No or Input.fresh = No)
  first.enabled = Docs and Unknown in first.present - first.enabled
  some a: State | a.phase = Running and a.saved = Docs and a.prompts = 0
}
pred DryWitness {
  Input.valid = Yes and Input.dry = Yes
  some a: State | a.phase = Dry and a.prompts = 0 and no a.engine
}
check PromptGate for 9 but exactly 9 State, 5 Int expect 0
check ExplicitSelection for 9 but exactly 9 State, 5 Int expect 0
check CliAuthority for 9 but exactly 9 State, 5 Int expect 0
check Persistence for 9 but exactly 9 State, 5 Int expect 0
check Domains for 9 but exactly 9 State, 5 Int expect 0
run SelectionWitness for 9 but exactly 9 State, 5 Int expect 1
run EmptyWitness for 9 but exactly 9 State, 5 Int expect 1
run ExplicitOffWitness for 9 but exactly 9 State, 5 Int expect 1
run CancellationWitness for 9 but exactly 9 State, 5 Int expect 1
run ExecCancellationWitness for 9 but exactly 9 State, 5 Int expect 1
run PreflightWitness for 9 but exactly 9 State, 5 Int expect 1
run SuppressedWitness for 9 but exactly 9 State, 5 Int expect 1
run DryWitness for 9 but exactly 9 State, 5 Int expect 1
