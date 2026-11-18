// Planning-action offers for idle sessions; production behavior is exercised by BDD.
module plan_revision

abstract sig Phase {}
one sig Draft, AwaitingInput, AwaitingApproval, Planned, Running extends Phase {}
abstract sig Bit {}
one sig Yes, No extends Bit {}
abstract sig Operation {}
one sig Fix, Ask, Verify, Replan extends Operation {}

sig State {
  phase: one Phase,
  planAvailable: one Bit,
  planError: one Bit,
  pendingPrompt: one Bit,
  webui, cli: set Operation
}

fun usablePlan[available, error: Bit]: Bit {
  (available = Yes and error = No) => Yes else No
}
fun webuiOffer[p: Phase, available, error, pending: Bit]: set Operation {
  (p = AwaitingApproval and available = Yes) =>
    Fix + (error = No => Ask else none)
      + (error = No and pending = No => Verify else none)
  else (p = Planned and available = Yes and error = No) =>
    Fix + Ask + (pending = No => Verify else none)
  else none
}
fun cliOffer[p: Phase]: set Operation {
  p = Planned => Replan else none
}

fact StateMeaning {
  all s: State |
      s.webui = webuiOffer[s.phase, s.planAvailable, s.planError, s.pendingPrompt]
      and s.cli = cliOffer[s.phase]
}

assert PlanningOffersMatchContract {
  all s: State |
    s.webui =
      ((s.phase = AwaitingApproval and s.planAvailable = Yes) =>
        Fix + (s.planError = No => Ask else none)
          + (s.planError = No and s.pendingPrompt = No => Verify else none)
      else (s.phase = Planned and s.planAvailable = Yes and s.planError = No) =>
        Fix + Ask + (s.pendingPrompt = No => Verify else none)
      else none)
    and s.cli = (s.phase = Planned => Replan else none)
}

pred AwaitingApprovalErrorFixWitness {
  some s: State |
    s.phase = AwaitingApproval and s.planAvailable = Yes
    and s.planError = Yes and s.pendingPrompt = No
    and s.webui = Fix and s.cli = none
}
pred AwaitingApprovalMissingPlanWitness {
  some s: State |
    s.phase = AwaitingApproval and s.planAvailable = No and s.planError = No
    and s.webui = none and s.cli = none
}
pred AwaitingApprovalUsablePlanWitness {
  some s: State |
    s.phase = AwaitingApproval and s.planAvailable = Yes
    and s.planError = No and s.pendingPrompt = No
    and s.webui = Fix + Ask + Verify
}
pred PlannedUsablePlanWitness {
  some s: State |
    s.phase = Planned and s.planAvailable = Yes
    and s.planError = No and s.pendingPrompt = No
    and s.webui = Fix + Ask + Verify
    and s.cli = Replan
}
pred PlannedPendingPromptWitness {
  some s: State |
    s.phase = Planned and s.planAvailable = Yes
    and s.planError = No and s.pendingPrompt = Yes
    and s.webui = Fix + Ask and s.cli = Replan
}

check PlanningOffersMatchContract for 3 expect 0
run AwaitingApprovalErrorFixWitness for 3 expect 1
run AwaitingApprovalMissingPlanWitness for 3 expect 1
run AwaitingApprovalUsablePlanWitness for 3 expect 1
run PlannedUsablePlanWitness for 3 expect 1
run PlannedPendingPromptWitness for 3 expect 1
