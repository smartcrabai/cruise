// Abstract workflow-start base CI admission, plan sections 1 and 2.
// Forge/API parsing and concurrent changes to remote CI are abstracted into
// one observation. A configured branch wins over the forge default.
module base_ci {
  type Config = { exec: bool, resume: bool, afterPr: bool, dry: bool,
    enabled: bool, forge: bool, configured: bool, outcome: str }
  type State = { phase: str, queried: bool, branch: str, warning: bool, workspace: bool }
  var cfg: Config
  var s: State
  pure val normal = { exec: false, resume: false, afterPr: false, dry: false,
    enabled: true, forge: true, configured: false, outcome: "passing" }
  pure def bypass(c: Config): bool = c.exec or c.resume or c.afterPr or c.dry or not(c.enabled) or not(c.forge)
  pure def blocked(c: Config): bool = not(bypass(c)) and Set("failing", "error").contains(c.outcome)
  pure def warned(c: Config): bool = not(bypass(c)) and Set("pending", "none").contains(c.outcome)
  action initWith(c: Config): bool = all {
    cfg' = c, s' = { phase: "checking", queried: false, branch: "none", warning: false, workspace: false }
  }
  action init = {
    nondet exec = Set(false, true).oneOf()
    nondet resume = Set(false, true).oneOf()
    nondet afterPr = Set(false, true).oneOf()
    nondet dry = Set(false, true).oneOf()
    nondet isEnabled = Set(false, true).oneOf()
    nondet forge = Set(false, true).oneOf()
    nondet configured = Set(false, true).oneOf()
    nondet outcome = Set("passing", "pending", "none", "failing", "error").oneOf()
    initWith({ exec: exec, resume: resume, afterPr: afterPr, dry: dry,
      enabled: isEnabled, forge: forge, configured: configured, outcome: outcome })
  }
  action decide = all {
    s.phase == "checking", cfg' = cfg,
    s' = { ...s, phase: if (blocked(cfg)) "rejected" else if (cfg.dry) "dry" else "authorized",
      queried: not(bypass(cfg)), branch: if (bypass(cfg)) "none" else if (cfg.configured) "configured" else "default",
      warning: warned(cfg) }
  }
  action launch = all { s.phase == "authorized", cfg' = cfg,
    s' = { ...s, phase: "launched", workspace: true } }
  action idle = all { cfg' = cfg, s' = s }
  action step = any { decide, launch, idle }
  val scope = s.phase != "checking" implies s.queried == not(bypass(cfg))
  val branchChoice = s.phase != "checking" implies
    s.branch == (if (bypass(cfg)) "none" else if (cfg.configured) "configured" else "default")
  val decision = s.phase != "checking" implies all {
    (s.phase == "rejected") iff blocked(cfg),
    s.warning == warned(cfg),
    (s.phase == "dry") iff cfg.dry,
  }
  val noPrematureEffect = s.workspace iff s.phase == "launched"
  val safety = scope and branchChoice and decision and noPrematureEffect
  run failureTest = initWith({ ...normal, outcome: "failing" }).then(decide)
    .expect(s.phase == "rejected" and not(s.workspace) and s.queried and safety)
  run errorTest = initWith({ ...normal, outcome: "error" }).then(decide)
    .expect(s.phase == "rejected" and safety)
  run pendingTest = initWith({ ...normal, outcome: "pending" }).then(decide).then(launch)
    .expect(s.warning and s.workspace and safety)
  run noChecksTest = initWith({ ...normal, outcome: "none" }).then(decide)
    .expect(s.warning and s.phase == "authorized" and safety)
  run passingTest = initWith(normal).then(decide).then(launch)
    .expect(s.workspace and not(s.warning) and s.branch == "default" and safety)
  run configuredTest = initWith({ ...normal, configured: true }).then(decide)
    .expect(s.branch == "configured" and safety)
  run optOutTest = initWith({ ...normal, enabled: false, outcome: "failing" }).then(decide)
    .expect(not(s.queried) and s.phase == "authorized" and safety)
  run resumeTest = initWith({ ...normal, resume: true, outcome: "error" }).then(decide)
    .expect(not(s.queried) and s.phase == "authorized" and safety)
  run afterPrTest = initWith({ ...normal, afterPr: true, outcome: "failing" }).then(decide)
    .expect(not(s.queried) and s.phase == "authorized" and safety)
  run execTest = initWith({ ...normal, exec: true, outcome: "failing" }).then(decide)
    .expect(not(s.queried) and s.phase == "authorized" and safety)
  run noForgeTest = initWith({ ...normal, forge: false, outcome: "error" }).then(decide)
    .expect(not(s.queried) and s.phase == "authorized" and safety)
  run dryTest = initWith({ ...normal, dry: true, outcome: "failing" }).then(decide)
    .expect(not(s.queried) and s.phase == "dry" and safety)
}
