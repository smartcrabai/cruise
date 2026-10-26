// Approval and new-session skip-selection contract from plan.md (2026-10-25).
// IDs: build=0, e2e=1, either=2, group parent=3, children=4/5,
// after-pr=6, unknown free-text skip=7. Flags 0/1 declared, 2 legacy.
// Finite observable abstraction, not a copy of UI/Rust data structures.
module flag_skip_selection {
  type State = { phase: int, catalog: Set[int], textMode: bool, on: Set[int], candidates: Set[int],
    skips: Set[int], savedOn: Set[int], savedSkips: Set[int], history: Set[int],
    forwardedKeys: Set[int], forwardedOn: Set[int], cliKeys: Set[int], flagPrompts: int,
    runPrompts: int, resume: Set[int] }
  var s: State
  var before: State
  var event: str
  pure val declared = Set(0, 1)
  pure val known = Set(0, 1, 2, 3, 4, 5, 6)
  pure val allIds = known.union(Set(7))
  pure val mainIds = Set(0, 1, 2, 4, 5)
  pure def available(on: Set[int], catalog: Set[int]): Set[int] = (Set(0)
    .union(if (on.contains(0)) Set(1) else Set())
    .union(if (on.intersect(declared) != Set()) Set(2) else Set())
    .union(if (on.contains(1)) Set(3, 4, 5, 6) else Set())).intersect(catalog)
  pure def prune(skips: Set[int], on: Set[int], catalog: Set[int]): Set[int] =
    skips.exclude(catalog.exclude(available(on, catalog)))
  pure def initial(on: Set[int], skips: Set[int], catalog: Set[int]): State = {
    phase: 0, catalog: catalog, textMode: catalog == Set(), on: on,
    candidates: available(on, catalog), skips: skips,
    savedOn: on, savedSkips: skips, history: skips, forwardedKeys: Set(),
    forwardedOn: Set(), cliKeys: Set(), flagPrompts: 0, runPrompts: 0, resume: mainIds.intersect(catalog) }
  action initCatalog(on: Set[int], skips: Set[int], catalog: Set[int]): bool = all {
    s' = initial(on, skips, catalog), before' = initial(on, skips, catalog), event' = "init" }
  action initWith(on: Set[int], skips: Set[int]): bool = initCatalog(on, skips, known)
  action init = {
    nondet on = Set(0, 1, 2).powerset().oneOf()
    nondet skips = allIds.powerset().oneOf()
    nondet catalog = Set(known, known.exclude(Set(0)), Set()).oneOf()
    initCatalog(on, skips, catalog)
  }
  action record(updated: State, name: str): bool = all {
    s' = updated, before' = s, event' = name }
  // The choice is staged, not saved, while the skip chooser is open.
  // Explicit -f takes the same path without a flag prompt.
  // Explicit CLI values were validated and saved before approval begins.
  // This boundary does not reapply or roll back those earlier assignments.
  action beginInput(chosen: Set[int], cli: Set[int]): bool = all {
    s.phase == 0, cli.subseteq(declared),
    chosen.subseteq(if (cli == Set()) declared else cli),
    cli != Set() implies chosen == s.savedOn.intersect(cli),
    val on = if (cli == Set()) s.savedOn.exclude(declared).union(chosen) else s.savedOn
    record({ ...s, phase: 1, on: on, candidates: available(on, s.catalog),
      skips: prune(s.savedSkips, on, s.catalog), cliKeys: cli,
      flagPrompts: if (cli == Set()) 1 else 0 }, if (cli == Set()) "flags" else "explicit") }
  action beginWith(chosen: Set[int], explicit: bool): bool =
    beginInput(chosen, if (explicit) declared else Set())
  action begin = {
    nondet cli = declared.powerset().oneOf()
    nondet chosen = (if (cli == Set()) declared else cli).powerset().oneOf()
    beginInput(chosen, cli)
  }
  // TUI/WebUI can change flags while editing. Hidden skips do not reappear.
  action changeTo(chosen: Set[int]): bool = all {
    s.phase == 1, s.cliKeys == Set(), chosen.subseteq(declared),
    val on = s.on.exclude(declared).union(chosen)
    record({ ...s, on: on, candidates: available(on, s.catalog), skips: prune(s.skips, on, s.catalog) }, "change") }
  action change = { nondet chosen = declared.powerset().oneOf(); changeTo(chosen) }
  action cancel = all { s.phase == 1,
    record({ ...s, phase: 0, on: s.savedOn, candidates: available(s.savedOn, s.catalog),
      skips: s.savedSkips, cliKeys: Set() }, "cancel") }
  action confirmWith(chosen: Set[int]): bool = all {
    s.phase == 1, chosen.subseteq(s.candidates),
    record({ ...s, phase: 2, skips: chosen, savedOn: s.on,
      savedSkips: chosen, history: chosen }, "confirm") }
  action confirm = { nondet chosen = s.candidates.powerset().oneOf(); confirmWith(chosen) }
  // Shared application boundary also sanitizes API/free-text new sessions.
  action create = all { s.phase == 0, s.on.subseteq(declared),
    val clean = prune(s.skips, s.on, s.catalog)
    record({ ...s, phase: 2, skips: clean, savedSkips: clean, history: clean }, "create") }
  action executeNow = all { s.phase == 2,
    val names = if (s.cliKeys == Set()) declared else s.cliKeys
    record({ ...s, phase: 3, forwardedKeys: names,
      forwardedOn: s.savedOn.intersect(names) }, "forward") }
  action enterRun = all { s.phase == 3,
    record({ ...s, phase: 4, runPrompts: if (s.forwardedKeys == Set()) 1 else 0 }, "run") }
  action idle = record(s, "idle")
  action step = any { begin, change, cancel, confirm, create, executeNow, enterRun, idle }

  val eligibility = all {
    s.candidates.contains(0) == s.catalog.contains(0),
    s.candidates.contains(1) == (s.on.contains(0) and s.catalog.contains(1)),
    s.candidates.contains(2) == ((s.on.contains(0) or s.on.contains(1)) and s.catalog.contains(2)),
    Set(3, 4, 5, 6).forall(id => s.candidates.contains(id) == (s.on.contains(1) and s.catalog.contains(id))),
    not(s.candidates.contains(7)), s.resume == mainIds.intersect(s.catalog),
    s.textMode == (s.catalog == Set()), s.catalog == before.catalog }
  val noHiddenSkips = s.phase != 0 implies s.skips.intersect(s.catalog).subseteq(s.candidates)
  val atomicity = all {
    not(Set("confirm", "create").contains(event)) implies
      s.savedOn == before.savedOn and s.savedSkips == before.savedSkips and s.history == before.history,
    event == "confirm" implies before.phase == 1 and s.savedOn == before.on,
    event == "cancel" implies s.phase == 0 and s.on == before.savedOn and s.skips == before.savedSkips,
    s.phase >= 2 implies s.history == s.savedSkips and s.skips == s.savedSkips }
  val pruning = Set("flags", "explicit", "change", "create").contains(event) implies all {
    s.skips.exclude(s.catalog) == before.skips.exclude(s.catalog),
    s.skips == before.skips.exclude(s.catalog.exclude(s.candidates)) }
  val forwarding = s.phase >= 3 implies all {
    s.forwardedKeys == (if (s.cliKeys == Set()) declared else s.cliKeys),
    s.forwardedOn == s.savedOn.intersect(s.forwardedKeys),
    s.forwardedKeys != Set(), s.forwardedKeys.subseteq(declared),
    not(s.forwardedKeys.contains(2)), s.runPrompts == 0 }
  val promptOrder = all {
    event == "flags" implies s.flagPrompts == 1 and before.phase == 0,
    event == "explicit" implies s.flagPrompts == 0,
    event == "confirm" implies before.phase == 1,
    s.flagPrompts >= 0 and s.flagPrompts <= 1 }
  val cliAuthority = event == "explicit" implies all {
    s.cliKeys != Set(), s.cliKeys.subseteq(declared),
    s.on == before.savedOn }
  val domains = all { s.on.subseteq(Set(0, 1, 2)), s.skips.subseteq(allIds),
    s.candidates.subseteq(known), s.cliKeys.subseteq(declared), 0.to(4).contains(s.phase) }
  val safety = eligibility and noHiddenSkips and atomicity and pruning and forwarding and promptOrder and cliAuthority and domains

  run offByDefaultTest = initWith(Set(), Set(1, 3, 4, 5, 6, 7)).then(beginWith(Set(), false))
    .expect(s.candidates == Set(0) and s.skips == Set(7) and safety)
  run orFlagTest = initWith(Set(), Set()).then(beginWith(Set(0), false))
    .expect(s.candidates == Set(0, 1, 2) and safety)
  run groupAndAfterPrTest = initWith(Set(), Set()).then(beginWith(Set(1), false))
    .expect(s.candidates == Set(0, 2, 3, 4, 5, 6) and safety)
  run cancellationTest = initWith(Set(1), Set(4, 7)).then(beginWith(Set(0), false))
    .then(cancel).expect(s.savedOn == Set(1) and s.savedSkips == Set(4, 7) and s.history == Set(4, 7) and safety)
  run hiddenSelectionDoesNotReturnTest = initWith(Set(1), Set(3, 4, 5, 6, 7))
    .then(beginWith(Set(1), false)).then(changeTo(Set())).then(changeTo(Set(1)))
    .expect(s.skips == Set(7) and safety)
  run createSanitizesTest = initWith(Set(), allIds).then(create)
    .expect(s.savedSkips == Set(0, 7) and s.history == Set(0, 7) and safety)
  run executeNowTest = initWith(Set(2), Set()).then(beginWith(Set(0), false))
    .then(confirmWith(Set(1))).then(executeNow).then(enterRun)
    .expect(s.savedOn == Set(0, 2) and s.forwardedKeys == declared and s.forwardedOn == Set(0) and s.runPrompts == 0 and safety)
  run explicitOffTest = initWith(Set(), Set(1)).then(beginWith(Set(), true))
    .then(confirmWith(Set())).then(executeNow).then(enterRun)
    .expect(s.flagPrompts == 0 and s.runPrompts == 0 and s.savedSkips == Set() and safety)
  run partialCliTest = initWith(Set(0), Set()).then(beginInput(Set(0), Set(0)))
    .then(confirmWith(Set(1))).then(executeNow).then(enterRun)
    .expect(s.savedOn == Set(0) and s.forwardedKeys == Set(0) and s.forwardedOn == Set(0)
      and s.flagPrompts == 0 and s.runPrompts == 0 and safety)
  run explicitCancellationTest = initWith(Set(0), Set(1, 7))
    .then(beginInput(Set(0), Set(0))).then(cancel)
    .expect(s.savedOn == Set(0) and s.on == Set(0) and s.savedSkips == Set(1, 7)
      and s.history == Set(1, 7) and safety)
  run allGatedOffStaysListTest = initCatalog(Set(), Set(1, 4), known.exclude(Set(0)))
    .then(beginWith(Set(), false)).expect(s.candidates == Set() and not(s.textMode) and s.skips == Set() and safety)
  run unknownCatalogUsesTextTest = initCatalog(Set(), Set(1, 7), Set()).then(create)
    .expect(s.textMode and s.savedSkips == Set(1, 7) and safety)
  run resumeUnaffectedTest = initWith(Set(), Set()).then(beginWith(Set(), false))
    .expect(s.resume.contains(1) and not(s.candidates.contains(1)) and safety)
}
