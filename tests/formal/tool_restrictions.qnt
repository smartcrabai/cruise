// Plan sections 2.2/2.3, R3/R4/R6: observable resolve -> authorize -> effect.
// Restriction tokens: 0 web, 1 subagent, 2 all workflow MCP, 3 MCP A,
// 4 explicit publish prefix, 5 explicit/implicit git mutation prefix.
// Requests: 0 web, 1 subagent, 2 MCP A, 3 MCP B, 4 publish, 5 git mutation,
// 6 harmless shell (status/diff). Shell syntax is quotiented by its match result,
// not claimed verified here. Concrete normalization is exercised by production BDD.
// Config starts after existing permission validation: command backends require full.
module tool_restrictions {
  type Config = { defaults: Set[int], workflow: Set[int], present: bool,
    child: Set[int], full: bool, commit: bool, sdk: bool }
  type State = { phase: str, denied: Set[int], mcp: Set[int], request: int, effect: bool }
  var cfg: Config
  var s: State
  pure val tokens = Set(0, 1, 2, 3, 4, 5)
  pure val requests = Set(0, 1, 2, 3, 4, 5, 6)
  pure def workflow(c: Config): Set[int] = if (c.present) c.workflow else c.defaults
  pure def explicit(c: Config): Set[int] = workflow(c).union(c.child)
  pure def prohibited(c: Config, r: int): bool =
    if (r == 0 or r == 1) explicit(c).contains(r)
    else if (r == 2) explicit(c).contains(2) or explicit(c).contains(3)
    else if (r == 3) explicit(c).contains(2)
    else if (r == 4) not(c.full) or explicit(c).contains(4)
    else if (r == 5) not(c.full) or not(c.commit) or explicit(c).contains(5)
    else not(c.full)
  pure val ordinary = { defaults: Set(0), workflow: Set(), present: false,
    child: Set(1, 3, 4), full: true, commit: false, sdk: true }
  action initWith(c: Config): bool = all {
    cfg' = c, s' = { phase: "configured", denied: Set(), mcp: Set(), request: -1, effect: false } }
  action init = {
    nondet defaults = tokens.powerset().oneOf()
    nondet top = tokens.powerset().oneOf()
    nondet child = tokens.powerset().oneOf()
    nondet present = Set(false, true).oneOf()
    nondet full = Set(false, true).oneOf()
    nondet commit = Set(false, true).oneOf()
    nondet sdk = Set(false, true).oneOf()
    all { sdk or full, initWith({ defaults: defaults, workflow: top, present: present,
      child: child, full: full, commit: commit, sdk: sdk }) }
  }
  action resolve = all {
    s.phase == "configured", cfg.sdk or explicit(cfg) == Set(),
    cfg' = cfg,
    s' = { ...s, phase: "ready",
      denied: if (cfg.sdk) requests.filter(r => prohibited(cfg, r)) else Set(),
      mcp: if (cfg.sdk) Set(2, 3).filter(r => not(prohibited(cfg, r))) else Set(2, 3) } }
  action reject = all { s.phase == "configured", not(cfg.sdk), explicit(cfg) != Set(),
    cfg' = cfg, s' = { ...s, phase: "rejected" } }
  action attempt(r: int): bool = all { s.phase == "ready", requests.contains(r),
    cfg' = cfg, s' = { ...s, phase: if (s.denied.contains(r)) "blocked" else "allowed",
      request: r, effect: not(s.denied.contains(r)) } }
  action choose = { nondet r = requests.oneOf(); attempt(r) }
  action idle = all { cfg' = cfg, s' = s }
  action step = any { resolve, reject, choose, idle }

  // Separate obligations: no loosening, no accidental broadening, admission,
  // exact MCP subset, and never perform a denied operation.
  val resolved = Set("ready", "blocked", "allowed").contains(s.phase)
  val denyOnly = resolved and cfg.sdk implies all {
    explicit(cfg).contains(0) implies s.denied.contains(0),
    explicit(cfg).contains(1) implies s.denied.contains(1),
    explicit(cfg).contains(4) implies s.denied.contains(4),
    explicit(cfg).contains(5) implies s.denied.contains(5),
    not(cfg.full) implies Set(4, 5, 6).subseteq(s.denied),
    not(cfg.commit) implies s.denied.contains(5) }
  val exactDecisions = resolved implies requests.forall(r =>
    s.denied.contains(r) iff (cfg.sdk and prohibited(cfg, r)))
  val mcpScope = resolved implies s.mcp == Set(2, 3).exclude(s.denied)
  val commandAdmission = all {
    resolved and not(cfg.sdk) implies explicit(cfg) == Set() and s.denied == Set(),
    s.phase == "rejected" implies not(cfg.sdk) and explicit(cfg) != Set() and not(s.effect) }
  val effectGate = all {
    s.effect iff s.phase == "allowed",
    Set("blocked", "allowed").contains(s.phase) implies
      (s.effect iff not(s.denied.contains(s.request))),
    s.effect and cfg.sdk implies not(prohibited(cfg, s.request)) }
  val safety = denyOnly and exactDecisions and mcpScope and commandAdmission and effectGate

  run unionTest = initWith(ordinary).then(resolve)
    .expect(s.denied == Set(0, 1, 2, 4, 5) and s.mcp == Set(3) and safety)
    .then(attempt(5)).expect(s.phase == "blocked" and not(s.effect) and safety)
  run safeShellTest = initWith(ordinary).then(resolve).then(attempt(6))
    .expect(s.effect and safety)
  run emptyOverrideTest = initWith({ ...ordinary, present: true, child: Set(), commit: true })
    .then(resolve).then(attempt(0)).expect(s.effect and s.denied == Set() and safety)
  run explicitSurvivesCommitTest = initWith({ ...ordinary, commit: true, child: Set(5) })
    .then(resolve).then(attempt(5)).expect(not(s.effect) and safety)
  run restrictedTest = initWith({ ...ordinary, full: false, commit: true, child: Set() })
    .then(resolve).then(attempt(6)).expect(not(s.effect) and safety)
  run allMcpTest = initWith({ ...ordinary, child: Set(2) }).then(resolve)
    .expect(s.mcp == Set() and safety)
  run commandRejectTest = initWith({ ...ordinary, sdk: false }).then(reject)
    .expect(s.phase == "rejected" and safety)
  run commandImplicitIgnoredTest = initWith({ ...ordinary, sdk: false, defaults: Set(), child: Set() })
    .then(resolve).then(attempt(5)).expect(s.effect and safety)
}
