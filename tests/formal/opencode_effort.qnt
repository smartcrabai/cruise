// OpenCode common effort and safe no-route fallback, plan requirements 1-10.
// Observable abstraction: two ordered base-model identities, one requested tier.
// Variant 1 has the canonical ID, 2/3 are opaque IDs, 4 is a native variant.
// cooled observes cooldowns of identities abandoned by this request, not the
// entire process cooldown map. No-switch cooldown updates are Rust unit checks.
// Profiles: 0 unlisted, 1 empty, 2 canonical + alias, 3 unique alias,
// 4 ambiguous aliases, 5 conflicting canonical + unique alias, 6 API failure.
// Unknown suffixes/normalization and diagnostic text are concrete BDD/unit tests,
// not string parsing reimplemented here. No liveness or Rust refinement claim.
module opencode_effort {
  type Config = { kind: int, primary: int, spare: int, resume: bool,
    policy: bool, budget: bool, fallbackEnabled: bool, candidate: bool }
  type State = { phase: int, model: int, chosen: int, ready: bool,
    catalogs: int, selections: int, prompts: int, barrier: bool,
    cooled: Set[int] }
  var cfg: Config
  var s: State
  var previous: State
  var event: str
  // kind 0 plain/unknown suffix/no model, 1 common effort, 2 native, 3 invalid.
  // phase 0 setup, 1 ready, 2 resolved, 3 selected, 4 executing,
  // 5 genuine model missing, 6 success, 7 local rejection, 8 permanent error.
  pure val initial = { phase: 0, model: 0, chosen: 0, ready: false,
    catalogs: 0, selections: 0, prompts: 0, barrier: false, cooled: Set() }
  action initWith(c: Config): bool = all {
    cfg' = c, s' = initial, previous' = initial, event' = "init",
  }
  action init = {
    nondet kind = 0.to(3).oneOf()
    nondet primary = 0.to(6).oneOf()
    nondet spare = 0.to(6).oneOf()
    nondet resume = Set(false, true).oneOf()
    nondet policy = Set(false, true).oneOf()
    nondet budget = Set(false, true).oneOf()
    nondet fallbackEnabled = Set(false, true).oneOf()
    nondet candidate = Set(false, true).oneOf()
    initWith({ kind: kind, primary: primary, spare: spare, resume: resume,
      policy: policy, budget: budget, fallbackEnabled: fallbackEnabled, candidate: candidate })
  }
  pure def compatible(profile: int): Set[int] =
    if (profile == 2) Set(1, 2)
    else if (profile == 3 or profile == 5) Set(2)
    else if (profile == 4) Set(2, 3) else Set()
  pure def permitted(profile: int, chosen: int): bool = {
    val matches = compatible(profile)
    if (matches.contains(1)) chosen == 1
    else matches.size() == 1 and matches.contains(chosen)
  }
  val profile = if (s.model == 0) cfg.primary else cfg.spare
  action record(updated: State, name: str): bool = all {
    cfg' = cfg, previous' = s, s' = updated, event' = name,
  }
  action rejectSyntax = all { s.phase == 0, cfg.kind == 3,
    record({ ...s, phase: 7, barrier: true }, "rejectSyntax") }
  action ready = all { s.phase == 0, cfg.kind != 3,
    record({ ...s, phase: 1, ready: true }, "ready") }
  action resolveTo(chosen: int): bool = all {
    s.phase == 1,
    if (cfg.kind == 1) permitted(profile, chosen)
    else chosen == (if (cfg.kind == 2) 4 else 0),
    record({ ...s, phase: 2, chosen: chosen,
      catalogs: s.catalogs + (if (cfg.kind == 1) 1 else 0) }, "resolve"),
  }
  action rejectCatalog = all { s.phase == 1, cfg.kind == 1,
    not(0.to(4).exists(v => permitted(profile, v))),
    record({ ...s, phase: 7, catalogs: s.catalogs + 1, barrier: true }, "rejectCatalog") }
  // admitSelection denotes either session creation or resume model update. Both must
  // wait for resolution and must be absent after a local capability failure.
  action admitSelection = all { s.phase == 2,
    record({ ...s, phase: 3, selections: s.selections + 1 },
      if (cfg.resume) "resumeUpdate" else "create") }
  action submit = all { s.phase == 3,
    record({ ...s, phase: 4, prompts: s.prompts + 1 }, "submit") }
  // Visible output, tool effects and uncertain acceptance are replay barriers.
  action barrier = all { s.phase == 4,
    record({ ...s, barrier: true }, "barrier") }
  action noRoute = all { s.phase == 4,
    record({ ...s, phase: 5 }, "noRoute") }
  // Auth/permission or invalid-request diagnostics never authorize switching,
  // even if mixed with a no-route header or generic model-missing words.
  action permanent = all { s.phase == 4,
    record({ ...s, phase: 8 }, "permanent") }
  action succeed = all { s.phase == 4,
    record({ ...s, phase: 6 }, "succeed") }
  action fallback = all { s.phase == 5, s.model == 0,
    cfg.policy, cfg.budget, cfg.fallbackEnabled, cfg.candidate, not(s.barrier),
    record({ ...s, phase: 0, model: 1, chosen: 0, ready: false,
      cooled: s.cooled.union(Set(0)) }, "fallback") }
  action idle = record(s, "idle")
  action step = {
    nondet chosen = 0.to(4).oneOf()
    any { rejectSyntax, ready, resolveTo(chosen), rejectCatalog, admitSelection,
      submit, barrier, noRoute, permanent, succeed, fallback, idle }
  }
  val capabilitySafety = all {
    cfg.kind == 1 and 2.to(6).contains(s.phase) implies permitted(profile, s.chosen),
    event == "resolve" or event == "rejectCatalog" implies previous.ready,
    cfg.kind != 1 implies s.catalogs == 0,
    event == "rejectCatalog" or event == "rejectSyntax" implies
      s.prompts == previous.prompts and s.selections == previous.selections,
    Set("resumeUpdate", "create").contains(event) implies previous.phase == 2 and previous.ready,
    event == "submit" implies previous.phase == 3 and previous.ready,
    s.prompts <= s.selections,
  }
  val fallbackSafety = all {
    event == "fallback" implies all { previous.phase == 5, previous.model == 0,
      cfg.policy, cfg.budget, cfg.fallbackEnabled, cfg.candidate, not(previous.barrier),
      s.model == 1, s.cooled.contains(0), s.chosen == 0, not(s.ready) },
    previous.barrier implies s.barrier and s.prompts == previous.prompts,
    previous.cooled.subseteq(s.cooled),
    s.cooled.contains(s.model) == false,
    event != "submit" implies s.prompts == previous.prompts,
    not(Set("resumeUpdate", "create").contains(event)) implies s.selections == previous.selections,
    Set(6, 7, 8).contains(previous.phase) implies s == previous,
    s.prompts <= 2 and s.catalogs <= 2 and s.selections <= 2,
  }
  val safety = capabilitySafety and fallbackSafety
  pure val common = { kind: 1, primary: 2, spare: 3, resume: false,
    policy: true, budget: true, fallbackEnabled: true, candidate: true }
  run canonicalPriorityTest = initWith(common).then(ready).then(resolveTo(1))
    .then(admitSelection).then(submit).then(succeed)
    .expect(s.chosen == 1 and s.catalogs == 1 and s.prompts == 1 and safety)
  run resumeAliasTest = initWith({ ...common, primary: 5, resume: true })
    .then(ready).then(resolveTo(2)).then(admitSelection).then(submit).then(succeed)
    .expect(s.chosen == 2 and s.selections == 1 and safety)
  run ambiguousRejectTest = initWith({ ...common, primary: 4, resume: true })
    .then(ready).then(rejectCatalog)
    .expect(s.prompts == 0 and s.selections == 0 and s.phase == 7 and safety)
  run catalogFailureTest = initWith({ ...common, primary: 6 })
    .then(ready).then(rejectCatalog).expect(s.prompts == 0 and safety)
  run nativeBypassTest = initWith({ ...common, kind: 2, primary: 6 })
    .then(ready).then(resolveTo(4)).then(admitSelection).then(submit).then(succeed)
    .expect(s.chosen == 4 and s.catalogs == 0 and safety)
  run fallbackRechecksTest = initWith(common).then(ready).then(resolveTo(1))
    .then(admitSelection).then(submit).then(noRoute).then(fallback).then(ready)
    .then(resolveTo(2)).then(admitSelection).then(submit).then(succeed)
    .expect(s.model == 1 and s.catalogs == 2 and s.prompts == 2
      and s.cooled == Set(0) and s.chosen == 2 and safety)
  run barrierTest = initWith(common).then(ready).then(resolveTo(1))
    .then(admitSelection).then(submit).then(barrier).then(noRoute).then(idle)
    .expect(s.model == 0 and s.prompts == 1 and s.barrier and safety)
  run authTest = initWith(common).then(ready).then(resolveTo(1))
    .then(admitSelection).then(submit).then(permanent).then(idle)
    .expect(s.model == 0 and s.phase == 8 and safety)
}
