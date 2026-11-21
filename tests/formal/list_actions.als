module list_actions

// Bounded API/selector model, not a model of prompt rendering or daemon jobs.
abstract sig Bit {}
one sig Yes, No extends Bit {}
abstract sig Phase {}
one sig Draft, Planned, Suspended, Other extends Phase {}
sig Label {}
sig Session { label: one Label }
sig Snapshot {
  rows: seq Session,
  phase: Session -> lone Phase,
  active: one Bit
} {
  #rows = #rows.elems
  phase.Phase = rows.elems
}

abstract sig Global {}
one sig NewSession, Clean, RunAll, Quit extends Global {}
abstract sig Route {}
one sig JsonOutput, InteractivePicker, LegacyPicker, EmptyExit extends Route {}
sig Invocation {
  jsonFlag, inputTty, outputTty: one Bit,
  snapshot: one Snapshot,
  route: one Route
}
fun globals[i: Invocation]: set Global {
  i.route = InteractivePicker => Global else none
}
fun selectedRows[i: Invocation]: seq Session {
  i.route in InteractivePicker + LegacyPicker => i.snapshot.rows else none -> none
}
fun displayed[i: Invocation]: Int -> (Global + Session) {
  i.route = InteractivePicker =>
    (0 -> NewSession + 1 -> Clean + 2 -> RunAll +
      { pos: Int, s: Session | some j: Int | s = i.snapshot.rows[j] and pos = add[j, 3] } +
      add[#i.snapshot.rows, 3] -> Quit)
  else selectedRows[i]
}
fun initialChoice[i: Invocation]: lone (Global + Session) {
  i.route = InteractivePicker and some selectedRows[i] => displayed[i][3] else displayed[i][0]
}
fact RouteInvocation {
  all i: Invocation |
    i.route = (i.jsonFlag = Yes => JsonOutput else
      (i.inputTty = Yes and i.outputTty = Yes => InteractivePicker else
        (some i.snapshot.rows => LegacyPicker else EmptyExit)))
}

abstract sig Mode {}
one sig PlanInteractive, PlanBackground, SaveDraft extends Mode {}
abstract sig Reply {}
one sig ConfirmYes, ConfirmNo, DefaultNo, Dismiss extends Reply {}
abstract sig Input {}
one sig Submitted, Cancelled extends Input {}
abstract sig Effect {}
one sig DelegatePlan, DelegateBackground, CreateDraft, CleanEligible,
  StartBatch, AttachBatch, NoActiveBatch extends Effect {}
abstract sig BatchPrompt {}
one sig PromptStart, PromptAttach extends BatchPrompt {}
fun candidates[s: Snapshot]: set Session { s.phase.(Planned + Suspended) }
fun runPrompt[s: Snapshot]: lone BatchPrompt {
  s.active = Yes => PromptAttach else (some candidates[s] => PromptStart else none)
}

sig GlobalVisit {
  invocation: one Invocation,
  choice: lone Global,
  mode: lone Mode,
  input: lone Input,
  reply: lone Reply,
  availability, atDispatch: lone Snapshot,
  effect: lone Effect,
  returned: lone Snapshot
} {
  choice in globals[invocation]
  invocation.route = InteractivePicker
  (some input iff choice = NewSession and some mode)
  (some mode implies choice = NewSession)
  (some availability iff choice = RunAll)
  (some reply iff choice = Clean or (some availability and some runPrompt[availability]))
  (some atDispatch iff choice = RunAll and reply = ConfirmYes)
  // Return to selector after the action completes, fails, declines or is cancelled.
  // Quit and selector dismissal exit. The refreshed snapshot can be unchanged.
  (some returned iff some choice and choice != Quit)
  effect = (
    choice = NewSession and input = Submitted =>
      (mode = PlanInteractive => DelegatePlan else
        (mode = PlanBackground => DelegateBackground else CreateDraft)) else
    (choice = Clean and reply = ConfirmYes => CleanEligible else
      (some atDispatch => (atDispatch.active = Yes => AttachBatch else
        (runPrompt[availability] = PromptAttach => NoActiveBatch else StartBatch)) else none)))
}

// A row returns its stored session ID, not a lookup by the display label or
// index from a newer fetch. A visit returning from Back uses live rows again.
sig SessionVisit {
  invocation: one Invocation,
  chosenIndex: one Int,
  selected, detailTarget, actionTarget: one Session,
  returned: one Snapshot
} {
  selected = selectedRows[invocation][chosenIndex]
  detailTarget = selected
  actionTarget = selected
}

assert JsonWins {
  all i: Invocation | i.jsonFlag = Yes implies
    i.route = JsonOutput and no globals[i] and no selectedRows[i]
}
assert HeadlessHasNoGlobals {
  all i: Invocation | i.jsonFlag = No and No in i.inputTty + i.outputTty implies
    no globals[i] and (some i.snapshot.rows =>
      (i.route = LegacyPicker and selectedRows[i] = i.snapshot.rows) else i.route = EmptyExit)
}
assert InteractiveRetainsRowsAndCursor {
  all i: Invocation | i.route = InteractivePicker implies
    globals[i] = NewSession + Clean + RunAll + Quit and selectedRows[i] = i.snapshot.rows and
    displayed[i][0] = NewSession and displayed[i][1] = Clean and displayed[i][2] = RunAll and
    displayed[i][add[#i.snapshot.rows, 3]] = Quit and
    (all j: i.snapshot.rows.inds | displayed[i][add[j, 3]] = i.snapshot.rows[j]) and
    (some i.snapshot.rows => initialChoice[i] = i.snapshot.rows[0] else initialChoice[i] = NewSession)
}
assert CreationNeedsModeAndSubmission {
  all v: GlobalVisit | v.choice = NewSession implies
    (no v.mode or v.input = Cancelled => no v.effect else
      ((v.mode = PlanInteractive implies v.effect = DelegatePlan) and
       (v.mode = PlanBackground implies v.effect = DelegateBackground) and
       (v.mode = SaveDraft implies v.effect = CreateDraft)))
}
assert ConfirmationGatesCleanAndBatch {
  all v: GlobalVisit | v.choice in Clean + RunAll implies
    (v.choice = RunAll implies (some v.atDispatch iff v.reply = ConfirmYes)) and
    (some v.effect implies v.reply = ConfirmYes) and
    (v.effect = StartBatch implies runPrompt[v.availability] = PromptStart) and
    (v.choice = Clean and v.reply = ConfirmYes implies v.effect = CleanEligible)
}
assert BatchPromptAndExecutionUseSeparateReads {
  all v: GlobalVisit | v.choice = RunAll implies
    let observed = v.availability |
      (observed.active = Yes implies runPrompt[observed] = PromptAttach) and
      (observed.active = No and some candidates[observed] implies runPrompt[observed] = PromptStart) and
      (observed.active = No and no candidates[observed] implies no v.reply and no v.effect) and
      (some v.atDispatch implies
        (v.atDispatch.active = Yes => v.effect = AttachBatch else
          (runPrompt[observed] = PromptAttach => v.effect = NoActiveBatch else v.effect = StartBatch)))
}
assert StableIdAndRefresh {
  (all v: SessionVisit |
    let chosen = v.invocation.snapshot.rows[v.chosenIndex] |
      v.detailTarget = chosen and v.actionTarget = chosen) and
  (all v: GlobalVisit | (no v.choice or v.choice = Quit) => no v.returned else one v.returned)
}

pred EmptyInteractive { some i: Invocation | i.route = InteractivePicker and no i.snapshot.rows }
pred JsonWithTtys { some i: Invocation | i.route = JsonOutput and i.inputTty = Yes and i.outputTty = Yes and some i.snapshot.rows }
pred HeadlessEmpty { some i: Invocation | i.route = EmptyExit and i.inputTty = No and i.outputTty = Yes }
pred HeadlessRows { some i: Invocation | i.route = LegacyPicker and i.inputTty = Yes and i.outputTty = No and some selectedRows[i] }
pred CreateInteractive { some v: GlobalVisit | v.effect = DelegatePlan }
pred CreateBackground { some v: GlobalVisit | v.effect = DelegateBackground }
pred CreateDraftOnly { some v: GlobalVisit | v.effect = CreateDraft }
pred CancelCreation {
  some m, i: GlobalVisit | m.choice = NewSession and no m.mode and no m.effect and
    i.mode = PlanBackground and i.input = Cancelled and no i.effect and some i.returned
}
pred CleanAndReturn {
  some v: GlobalVisit | v.effect = CleanEligible and
    v.returned.rows.elems in v.invocation.snapshot.rows.elems and
    v.returned.rows.elems != v.invocation.snapshot.rows.elems
}
pred DeclineAndDismiss {
  some clean, batch: GlobalVisit | clean.choice = Clean and clean.reply = ConfirmNo and
    batch.choice = RunAll and batch.reply = Dismiss and no clean.effect + batch.effect
}
pred DefaultDeclines {
  some v: GlobalVisit | v.reply = DefaultNo and no v.effect and some v.returned
}
pred StartConfirmedBatch {
  some v: GlobalVisit | v.effect = StartBatch and runPrompt[v.availability] = PromptStart and
    no candidates[v.invocation.snapshot] and some candidates[v.availability] and
    some candidates[v.atDispatch] and candidates[v.atDispatch] in v.atDispatch.phase.Suspended
}
pred AttachWithoutCandidates {
  some v: GlobalVisit | v.effect = AttachBatch and runPrompt[v.availability] = PromptAttach and
    no candidates[v.availability] + candidates[v.atDispatch]
}
pred BatchRace {
  some v: GlobalVisit | runPrompt[v.availability] = PromptStart and v.effect = AttachBatch
}
pred StaleAttach {
  some v: GlobalVisit | runPrompt[v.availability] = PromptAttach and v.reply = ConfirmYes and
    v.atDispatch.active = No and some candidates[v.atDispatch] and
    v.effect = NoActiveBatch and some v.returned
}
pred NoBatchWork {
  some v: GlobalVisit | v.choice = RunAll and v.availability.active = No and
    some v.availability.rows and no candidates[v.availability] and no v.reply and no v.effect
}
pred SameLabelRefresh {
  some v: SessionVisit, next: Invocation |
    v.invocation.route = InteractivePicker and next.route = InteractivePicker and next.snapshot = v.returned and
    let first = v.invocation.snapshot.rows[v.chosenIndex], second = selectedRows[next][v.chosenIndex] |
      some second and first != second and first.label = second.label and v.actionTarget = first
}
pred QuitWithoutRefresh { some v: GlobalVisit | v.choice = Quit and no v.returned and no v.effect }
pred DismissSelector { some v: GlobalVisit | no v.choice and no v.effect and no v.returned }

check JsonWins for 3 but 3 seq expect 0
check HeadlessHasNoGlobals for 3 but 3 seq expect 0
check InteractiveRetainsRowsAndCursor for 3 but 3 seq expect 0
check CreationNeedsModeAndSubmission for 3 but 3 seq expect 0
check ConfirmationGatesCleanAndBatch for 3 but 3 seq expect 0
check BatchPromptAndExecutionUseSeparateReads for 3 but 3 seq expect 0
check StableIdAndRefresh for 3 but 3 seq expect 0
run EmptyInteractive for 3 but 3 seq expect 1
run JsonWithTtys for 3 but 3 seq expect 1
run HeadlessEmpty for 3 but 3 seq expect 1
run HeadlessRows for 3 but 3 seq expect 1
run CreateInteractive for 3 but 3 seq expect 1
run CreateBackground for 3 but 3 seq expect 1
run CreateDraftOnly for 3 but 3 seq expect 1
run CancelCreation for 3 but 3 seq expect 1
run CleanAndReturn for 3 but 3 seq expect 1
run DeclineAndDismiss for 3 but 3 seq expect 1
run DefaultDeclines for 3 but 3 seq expect 1
run StartConfirmedBatch for 3 but 3 seq expect 1
run AttachWithoutCandidates for 3 but 3 seq expect 1
run BatchRace for 3 but 3 seq expect 1
run StaleAttach for 3 but 3 seq expect 1
run NoBatchWork for 3 but 3 seq expect 1
run SameLabelRefresh for 3 but 3 seq expect 1
run QuitWithoutRefresh for 3 but 3 seq expect 1
run DismissSelector for 3 but 3 seq expect 1
