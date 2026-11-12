// Planning-action offers for idle sessions; production behavior is exercised by BDD.
module plan_revision {
  type State = { phase: str, planAvailable: bool, planError: bool,
    pendingPrompt: bool, webuiActions: Set[str], tuiActions: Set[str],
    cliActions: Set[str] }
  var s: State

  val phases = Set("Draft", "AwaitingInput", "AwaitingApproval", "Planned", "Running")

  pure def usablePlan(planAvailable: bool, planError: bool): bool =
    planAvailable and not(planError)

  pure def tuiOffer(p: str, planAvailable: bool, planError: bool): Set[str] =
    if (p == "AwaitingApproval") {
      if (usablePlan(planAvailable, planError)) Set("Fix", "Ask", "Verify")
      else Set("Fix")
    } else if (p == "Planned" and usablePlan(planAvailable, planError))
      Set("Fix", "Ask", "Verify")
    else Set()

  pure def webuiOffer(p: str, planAvailable: bool, planError: bool,
    pendingPrompt: bool): Set[str] =
    if (p == "AwaitingApproval" and planAvailable) {
      Set("Fix")
        .union(if (planError) Set() else Set("Ask"))
        .union(if (planError or pendingPrompt) Set() else Set("Verify"))
    } else if (p == "Planned" and usablePlan(planAvailable, planError)) {
      Set("Fix", "Ask").union(if (pendingPrompt) Set() else Set("Verify"))
    } else Set()

  pure def cliOffer(p: str): Set[str] =
    if (p == "Planned") Set("Replan") else Set()

  action initAt(p: str, available: bool, error: bool, pending: bool): bool = all {
    phases.contains(p),
    s' = { phase: p, planAvailable: available, planError: error,
      pendingPrompt: pending,
      webuiActions: webuiOffer(p, available, error, pending),
      tuiActions: tuiOffer(p, available, error), cliActions: cliOffer(p) }
  }
  action init = {
    nondet p = phases.oneOf()
    nondet available = Set(false, true).oneOf()
    nondet error = Set(false, true).oneOf()
    nondet pending = Set(false, true).oneOf()
    initAt(p, available, error, pending)
  }
  action idle = s' = s
  action step = idle

  val safety = all {
    s.tuiActions == (if (s.phase == "AwaitingApproval")
      if (s.planAvailable and not(s.planError)) Set("Fix", "Ask", "Verify")
      else Set("Fix")
      else if (s.phase == "Planned" and s.planAvailable and not(s.planError))
        Set("Fix", "Ask", "Verify")
      else Set()),
    s.webuiActions == (if (s.phase == "AwaitingApproval" and s.planAvailable)
      Set("Fix")
        .union(if (s.planError) Set() else Set("Ask"))
        .union(if (s.planError or s.pendingPrompt) Set() else Set("Verify"))
      else if (s.phase == "Planned" and s.planAvailable and not(s.planError))
        Set("Fix", "Ask").union(if (s.pendingPrompt) Set() else Set("Verify"))
      else Set()),
    s.cliActions == (if (s.phase == "Planned") Set("Replan") else Set())
  }

  run awaitingApprovalErrorFix = initAt("AwaitingApproval", true, true, false)
    .expect(s.tuiActions == Set("Fix") and s.webuiActions == Set("Fix")
      and s.cliActions == Set() and safety)
  run awaitingApprovalMissingPlan = initAt("AwaitingApproval", false, false, false)
    .expect(s.tuiActions == Set("Fix") and s.webuiActions == Set()
      and s.cliActions == Set() and safety)
  run awaitingApprovalUsablePlan = initAt("AwaitingApproval", true, false, false)
    .expect(s.tuiActions == Set("Fix", "Ask", "Verify")
      and s.webuiActions == Set("Fix", "Ask", "Verify") and safety)
  run plannedUsablePlan = initAt("Planned", true, false, false)
    .expect(s.tuiActions == Set("Fix", "Ask", "Verify")
      and s.webuiActions == Set("Fix", "Ask", "Verify")
      and s.cliActions == Set("Replan") and safety)
  run plannedPendingPrompt = initAt("Planned", true, false, true)
    .expect(s.tuiActions == Set("Fix", "Ask", "Verify")
      and s.webuiActions == Set("Fix", "Ask") and safety)
}
