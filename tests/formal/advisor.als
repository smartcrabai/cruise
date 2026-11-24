// Advisor configuration placement and model selection (empty lists represented as absent).
module advisor

abstract sig Role {}
one sig Step, Advisor, Plan, Normal extends Role {}
abstract sig Placement {}
one sig TopPrompt, InlinedPrompt, CommandStep, OptionStep, ParallelWrapper,
  ParallelChild, GroupCall, GroupChild, AfterPR, WorkflowCall, SquashMerge,
  GithubReview extends Placement {}
abstract sig Permission {}
one sig Unspecified, ReadOnly, Editable, Full extends Permission {}

one sig Input {
  available: set Role,
  advisorStep: lone Advisor,
  placement: one Placement,
  outputFile: lone Placement,
  commitAllowed: lone Placement,
  permission: one Permission
}

pred allowed {
  no Input.advisorStep or (
    Input.placement in TopPrompt + InlinedPrompt
    and some Input.outputFile and no Input.commitAllowed
    and Input.permission in Unspecified + ReadOnly
  )
}

fun selected: lone Role {
  let a = Input.available |
    Step in a => Step else
    (some Input.advisorStep and Advisor in a) => Advisor else
    (some Input.advisorStep and Plan in a) => Plan else
    Normal & a
}

assert AdvisorSelectionUsesHighestEligibleRole {
  some Input.advisorStep implies {
    let higher = Step->Advisor + Step->Plan + Step->Normal + Advisor->Plan + Advisor->Normal + Plan->Normal |
      selected = Input.available - Input.available.higher
  }
}
assert OrdinaryStepIgnoresAdvisorAndPlan {
  no Input.advisorStep implies selected =
    (Step in Input.available => Step else Normal & Input.available)
}
assert ValidAdvisorRequirements {
  some Input.advisorStep implies (
    allowed iff not (
      Input.placement in CommandStep + OptionStep + ParallelWrapper + ParallelChild + GroupCall
         + GroupChild + AfterPR + WorkflowCall + SquashMerge + GithubReview
      or some Input.commitAllowed
      or no Input.outputFile
      or Input.permission in Editable + Full
    )
  )
}
assert OrdinaryPlacementUnaffected { no Input.advisorStep implies allowed }
pred InlinedAdvisorWitness {
  allowed and some Input.advisorStep and Input.placement = InlinedPrompt
  Input.available = Role and selected = Step
}
pred PlanningFallbackWitness {
  allowed and some Input.advisorStep and Input.placement = TopPrompt
  Input.available = Plan + Normal and selected = Plan
}
pred InvalidParallelWitness {
  some Input.advisorStep and Input.placement = ParallelChild
  some Input.outputFile and no Input.commitAllowed and Input.permission = ReadOnly
  not allowed
}

check AdvisorSelectionUsesHighestEligibleRole for 12 expect 0
check OrdinaryStepIgnoresAdvisorAndPlan for 12 expect 0
check ValidAdvisorRequirements for 12 expect 0
check OrdinaryPlacementUnaffected for 12 expect 0
run InlinedAdvisorWitness for 12 expect 1
run PlanningFallbackWitness for 12 expect 1
run InvalidParallelWitness for 12 expect 1
