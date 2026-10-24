// Run-boundary contract from plan.md sections 1, 2.1, 2.2, 2.4 and 2.6.
// Flags 0/1 are declared, 2 is an unknown saved key. Absence means off.
// Preflight abstracts config/plan/retry/compile/resume validation, not flag-aware
// graph validation. No fairness, provider calls or general workflow model claimed.
module workflow_flags {
  type Input = { dry: bool, cli: Set[int], cliOn: Set[int], single: bool,
    fresh: bool, declared: Set[int], tty: bool, valid: bool, exec: bool }
  type State = { phase: int, enabled: Set[int], present: Set[int],
    saved: Set[int], savedPresent: Set[int], exists: bool, prompts: int,
    engine: Set[int], selected: Set[int] }
  var input: Input
  var s: State
  var before: State
  var event: str
  pure def eligible(i: Input): bool = not(i.dry) and i.cli == Set()
    and i.single and i.fresh and i.declared != Set() and i.tty
  pure def initial(on: Set[int], storedKeys: Set[int]): State = { phase: 0, enabled: on, present: storedKeys,
    saved: on, savedPresent: storedKeys, exists: true, prompts: 0, engine: Set(), selected: Set() }
  action initKeys(i: Input, on: Set[int], storedKeys: Set[int]): bool = all {
    input' = i, s' = initial(on, storedKeys), before' = initial(on, storedKeys), event' = "init" }
  action initWith(i: Input, on: Set[int]): bool = initKeys(i, on, on)
  action init = {
    nondet dry = Set(false, true).oneOf()
    nondet cli = Set(0, 1).powerset().oneOf()
    nondet cliOn = cli.powerset().oneOf()
    nondet single = Set(false, true).oneOf()
    nondet fresh = Set(false, true).oneOf()
    nondet declared = Set(0, 1).powerset().oneOf()
    nondet tty = Set(false, true).oneOf()
    nondet valid = Set(false, true).oneOf()
    nondet exec = Set(false, true).oneOf()
    nondet storedKeys = Set(0, 1, 2).powerset().oneOf()
    nondet on = storedKeys.powerset().oneOf()
    all { cli.subseteq(declared), initKeys({ dry: dry, cli: cli, cliOn: cliOn,
      single: single, fresh: fresh, declared: declared, tty: tty, valid: valid, exec: exec }, on, storedKeys) }
  }
  action record(updated: State, name: str): bool = all {
    input' = input, s' = updated, before' = s, event' = name }
  action prepare = all { s.phase == 0, input.valid,
    record({ ...s, phase: if (eligible(input)) 1 else if (input.dry) 6 else 2,
      prompts: if (eligible(input)) s.prompts + 1 else s.prompts,
      enabled: s.enabled.exclude(input.cli).union(input.cliOn),
      present: s.present.union(input.cli) }, "prepare") }
  action reject = all { s.phase == 0, not(input.valid),
    record({ ...s, phase: 5, exists: not(input.exec) }, "reject") }
  action selectFlags(chosen: Set[int]): bool = all { s.phase == 1,
    chosen.subseteq(input.declared),
    record({ ...s, phase: 2, selected: chosen,
      enabled: s.enabled.exclude(input.declared).union(chosen),
      present: s.present.union(input.declared) }, "select") }
  action choose = { nondet chosen = input.declared.powerset().oneOf()
    selectFlags(chosen) }
  action cancel = all { s.phase == 1,
    record({ ...s, phase: 4, exists: not(input.exec) }, "cancel") }
  // Workspace success is abstracted here. Only then are flags saved and executed.
  action start = all { s.phase == 2,
    record({ ...s, phase: 3, saved: s.enabled, savedPresent: s.present,
      engine: s.enabled }, "start") }
  action idle = record(s, "idle")
  action step = any { prepare, reject, choose, cancel, start, idle }

  val promptGate = all {
    s.prompts <= 1,
    s.phase == 1 implies eligible(input) and input.valid and s.prompts == 1,
    Set("select", "cancel").contains(event) implies eligible(input) and input.valid,
    not(eligible(input)) implies s.prompts == 0,
  }
  val explicitSelection = event == "select" implies all {
    s.enabled.intersect(input.declared) == s.selected,
    input.declared.subseteq(s.present),
    s.enabled.exclude(input.declared) == before.enabled.exclude(input.declared),
    s.present.exclude(input.declared) == before.present.exclude(input.declared),
  }
  val cliAuthority = event == "prepare" implies all {
    s.enabled.intersect(input.cli) == input.cliOn,
    input.cli.subseteq(s.present),
    s.enabled.exclude(input.cli) == before.enabled.exclude(input.cli),
  }
  val persistence = all {
    event != "start" implies s.saved == before.saved and s.savedPresent == before.savedPresent,
    s.phase == 3 implies s.saved == s.engine and s.enabled == s.engine,
    s.phase == 4 implies s.saved == before.saved and s.exists == not(input.exec),
    s.phase == 6 implies s.prompts == 0 and s.engine == Set(),
  }
  val domains = all { 0.to(6).contains(s.phase), s.enabled.subseteq(Set(0, 1, 2)),
    s.enabled.subseteq(s.present), s.prompts >= 0,
    s.saved.subseteq(s.savedPresent), s.engine.subseteq(Set(0, 1, 2)) }
  val safety = promptGate and explicitSelection and cliAuthority and persistence and domains

  pure val interactive = { dry: false, cli: Set(), cliOn: Set(), single: true,
    fresh: true, declared: Set(0, 1), tty: true, valid: true, exec: false }
  run selectionTest = initWith(interactive, Set(1, 2)).then(prepare)
    .then(selectFlags(Set(0))).expect(s.enabled == Set(0, 2) and s.present == Set(0, 1, 2) and safety)
    .then(start).expect(s.saved == Set(0, 2) and s.engine == s.saved and s.prompts == 1 and safety)
  run emptySelectionTest = initWith(interactive, Set(0, 1, 2)).then(prepare)
    .then(selectFlags(Set())).then(start).expect(s.saved == Set(2) and safety)
  run explicitOffTest = initWith({ ...interactive, cli: Set(0), cliOn: Set() }, Set(0, 1))
    .then(prepare).then(start).expect(s.saved == Set(1) and s.prompts == 0 and safety)
  run dryRunTest = initWith({ ...interactive, dry: true }, Set(0)).then(prepare)
    .expect(s.phase == 6 and s.saved == Set(0) and safety)
  run batchTest = initWith({ ...interactive, single: false }, Set(0)).then(prepare).then(start)
    .expect(s.prompts == 0 and s.saved == Set(0) and safety)
  run resumeTest = initWith({ ...interactive, fresh: false }, Set(1)).then(prepare).then(start)
    .expect(s.prompts == 0 and s.saved == Set(1) and safety)
  run nonTerminalTest = initWith({ ...interactive, tty: false }, Set(1)).then(prepare).then(start)
    .expect(s.prompts == 0 and s.saved == Set(1) and safety)
  run noDeclarationsTest = initWith({ ...interactive, declared: Set() }, Set(2)).then(prepare).then(start)
    .expect(s.prompts == 0 and s.saved == Set(2) and safety)
  run keepDefaultsTest = initKeys(interactive, Set(1), Set(0, 1, 2)).then(prepare)
    .expect(s.enabled == Set(1) and safety).then(selectFlags(Set(1))).then(start)
    .expect(s.saved == Set(1) and s.savedPresent == Set(0, 1, 2) and safety)
  run cancellationTest = initWith(interactive, Set(1)).then(prepare).then(cancel)
    .expect(s.phase == 4 and s.saved == Set(1) and s.exists and safety)
  run execCancellationTest = initWith({ ...interactive, exec: true }, Set())
    .then(prepare).then(cancel).expect(not(s.exists) and s.engine == Set() and safety)
  run preflightTest = initWith({ ...interactive, valid: false }, Set(1)).then(reject)
    .expect(s.phase == 5 and s.prompts == 0 and s.saved == Set(1) and safety)
}
