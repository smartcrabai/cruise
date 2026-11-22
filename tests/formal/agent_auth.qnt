// Workflow-start jcode provider authentication preflight.
// Access expiry: 0 unknown, 1 <= now, 2 > now. Equality is expired.
// Target 0 is Anthropic; target 1 is OpenAI; target 2 is unsupported/bare/API.
// Two account slots represent a provider pool; token bytes are excluded.
module agent_auth {
  type Account = { access: int, refresh: bool }
  type Config = { planning: bool, sdk: bool, skip: bool, dry: bool,
    otherAuth: bool, plan: Set[int], execution: Set[int],
    anthropicFirst: Account, anthropicSecond: Account,
    openaiFirst: Account, openaiSecond: Account }
  type State = { phase: str, read: Set[int], errors: Set[int], workspace: bool }
  var cfg: Config
  var s: State

  pure val unknown = { access: 0, refresh: false }
  pure val dead = { access: 1, refresh: false }
  pure val renewable = { access: 1, refresh: true }
  pure val fresh = { access: 2, refresh: false }
  pure val ordinary = { planning: false, sdk: true, skip: false, dry: false,
    otherAuth: false, plan: Set(0), execution: Set(1),
    anthropicFirst: dead, anthropicSecond: dead,
    openaiFirst: dead, openaiSecond: dead }
  pure def expired(a: Account): bool = a.access == 1 and not(a.refresh)
  pure def bypass(c: Config): bool = c.skip or not(c.sdk) or c.dry
  pure def selected(c: Config): Set[int] =
    (if (c.planning) c.plan else c.execution).intersect(Set(0, 1))
      .exclude(if (c.otherAuth) Set(0) else Set())
  pure def required(c: Config): Set[int] = if (bypass(c)) Set() else selected(c)
  pure def unavailable(c: Config): Set[int] = Set(0, 1).filter(t =>
    if (t == 0) expired(c.anthropicFirst) and expired(c.anthropicSecond)
    else expired(c.openaiFirst) and expired(c.openaiSecond))
  action initWith(c: Config): bool = all {
    cfg' = c, s' = { phase: "checking", read: Set(), errors: Set(), workspace: false } }
  action init = {
    nondet access0 = 0.to(2).oneOf()
    nondet refresh0 = Set(false, true).oneOf()
    nondet access1 = 0.to(2).oneOf()
    nondet refresh1 = Set(false, true).oneOf()
    nondet access2 = 0.to(2).oneOf()
    nondet refresh2 = Set(false, true).oneOf()
    nondet access3 = 0.to(2).oneOf()
    nondet refresh3 = Set(false, true).oneOf()
    nondet plan = Set(0, 1, 2).powerset().oneOf()
    nondet execution = Set(0, 1, 2).powerset().oneOf()
    nondet planning = Set(false, true).oneOf()
    nondet sdk = Set(false, true).oneOf()
    nondet skip = Set(false, true).oneOf()
    nondet dry = Set(false, true).oneOf()
    nondet otherAuth = Set(false, true).oneOf()
    initWith({ planning: planning, sdk: sdk, skip: skip, dry: dry,
      otherAuth: otherAuth, plan: plan, execution: execution,
      anthropicFirst: { access: access0, refresh: refresh0 },
      anthropicSecond: { access: access1, refresh: refresh1 },
      openaiFirst: { access: access2, refresh: refresh2 },
      openaiSecond: { access: access3, refresh: refresh3 } })
  }
  action inspect(t: int): bool = all {
    s.phase == "checking", required(cfg).contains(t), not(s.read.contains(t)),
    cfg' = cfg,
    s' = { ...s, read: s.read.union(Set(t)), errors:
      if (unavailable(cfg).contains(t)) s.errors.union(Set(t)) else s.errors } }
  action inspectNext = {
    nondet t = Set(0, 1).oneOf()
    inspect(t)
  }
  action decide = all {
    s.phase == "checking", s.read == required(cfg), cfg' = cfg,
    s' = { ...s, phase: if (s.errors.size() > 0) "rejected"
      else if (cfg.dry) "dry" else "authorized" } }
  action launch = all {
    s.phase == "authorized", cfg' = cfg,
    s' = { ...s, phase: "launched", workspace: not(cfg.planning) } }
  action idle = all { cfg' = cfg, s' = s }
  action step = any { inspectNext, decide, launch, idle }

  val scopeSafety = s.read.subseteq(required(cfg))
  val exactDiagnostics = s.errors == s.read.intersect(unavailable(cfg))
  val completeBeforeDecision = s.phase != "checking" implies s.read == required(cfg)
  val admission = all {
    Set("authorized", "launched", "dry").contains(s.phase) implies
      required(cfg).intersect(unavailable(cfg)).size() == 0,
    s.phase == "rejected" iff
      (s.phase != "checking" and required(cfg).intersect(unavailable(cfg)).size() > 0),
  }
  val effectGate = all {
    s.workspace implies s.phase == "launched" and not(cfg.planning),
    cfg.dry implies s.phase != "launched" and not(s.workspace),
    s.phase == "rejected" implies not(s.workspace),
  }
  val domains = Set("checking", "authorized", "launched", "rejected", "dry").contains(s.phase)
  val safety = scopeSafety and exactDiagnostics and completeBeforeDecision and admission and effectGate and domains

  run rejectBeforeEffectTest = initWith(ordinary).then(inspect(1)).then(decide)
    .expect(s.phase == "rejected" and s.errors == Set(1) and not(s.workspace) and safety)
  run renewableAccountTest = initWith({ ...ordinary, openaiSecond: renewable })
    .then(inspect(1)).then(decide).then(launch)
    .expect(s.phase == "launched" and s.errors == Set() and safety)
  run unknownAccountTest = initWith({ ...ordinary, openaiSecond: unknown })
    .then(inspect(1)).then(decide).then(launch).expect(s.phase == "launched" and safety)
  run allTargetsReportedTest = initWith({ ...ordinary, execution: Set(0, 1, 2) })
    .then(inspect(1)).then(inspect(0)).then(decide)
    .expect(s.errors == Set(0, 1) and s.phase == "rejected" and safety)
  run planningScopeTest = initWith({ ...ordinary, planning: true, anthropicFirst: fresh })
    .then(inspect(0)).then(decide).then(launch)
    .expect(s.read == Set(0) and not(s.workspace) and safety)
  run dryRunTest = initWith(ordinary).then(inspect(1)).then(decide)
    .expect(s.phase == "dry" and s.read == Set() and safety)
  run planningDryRunTest = initWith({ ...ordinary, planning: true, dry: true }).then(decide)
    .expect(s.phase == "dry" and s.read == Set() and safety)
  run optOutTest = initWith({ ...ordinary, skip: true }).then(decide).then(launch)
    .expect(s.phase == "launched" and s.read == Set() and safety)
  run commandTest = initWith({ ...ordinary, sdk: false }).then(decide).then(launch)
    .expect(s.phase == "launched" and s.read == Set() and safety)
  run unsupportedProviderTest = initWith({ ...ordinary, execution: Set(2) }).then(decide)
    .then(launch).expect(s.read == Set() and s.phase == "launched" and safety)
  run alternativeAuthTest = initWith({ ...ordinary, execution: Set(0, 2), otherAuth: true })
    .then(decide).then(launch).expect(s.read == Set() and safety)
  run expiryBoundaryTest = initWith({ ...ordinary, execution: Set(0) })
    .then(inspect(0)).then(decide).expect(s.phase == "rejected" and safety)
  run freshAccessTest = initWith({ ...ordinary, execution: Set(0), anthropicFirst: fresh })
    .then(inspect(0)).then(decide).then(launch)
    .expect(s.phase == "launched" and safety)
}
