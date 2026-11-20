// Bounded attempt lifecycle. Kernel capability flags are assumed properties of
// Bubblewrap binds; attempted writes are markers, not a filesystem-effect model.
module jcode_bubblewrap {
  type State = { phase: int, gen: int, ownerGen: int, runtime: bool,
    proxy: bool, sandboxed: bool, platformOK: bool, launchOK: bool,
    helloOK: bool, editable: bool, roWriteCap: bool, editWriteCap: bool,
    roAttempt: bool, editAttempt: bool, pending: int, result: int,
    turns: int, fallback: bool }
  var s: State
  var before: State
  var event: str

  // 0 ready, 1 host prep, 2 owned launch, 3 connected, 4 turn,
  // 5 reap pending, 6 reaped, 7 terminal. Outcomes: 0 none, 1 success,
  // 2 failure, 3 cancelled/aborted.
  pure val initial: State = { phase: 0, gen: 0, ownerGen: 0,
    runtime: false, proxy: false, sandboxed: false, platformOK: false,
    launchOK: false, helloOK: false, editable: false, roWriteCap: false,
    editWriteCap: false, roAttempt: false, editAttempt: false,
    pending: 0, result: 0, turns: 0, fallback: false }
  action init = all { s' = initial, before' = initial, event' = "init" }
  action record(updated: State, name: str): bool = all {
    before' = s, s' = updated, event' = name }

  action hostPrep(platform: bool, launch: bool, hello: bool, canEdit: bool): bool = all {
    (s.phase == 0 or s.phase == 7), s.gen < 2, not(s.runtime), not(s.proxy),
    record({ ...s, phase: 1, gen: s.gen + 1, platformOK: platform,
      launchOK: launch, helloOK: hello, editable: canEdit, sandboxed: false,
      roWriteCap: false, editWriteCap: false, roAttempt: false,
      editAttempt: false, pending: 0, result: 0, fallback: false }, "hostPrep") }
  action prepFailure = all { s.phase == 1, not(s.platformOK),
    record({ ...s, phase: 5, pending: 2 }, "prepFailure") }
  action launch = all { s.phase == 1, s.platformOK, s.launchOK,
    record({ ...s, phase: 2, ownerGen: s.gen, runtime: true, proxy: true,
      sandboxed: true, roWriteCap: false, editWriteCap: s.editable }, "launch") }
  action launchFailure = all { s.phase == 1, s.platformOK, not(s.launchOK),
    record({ ...s, phase: 5, pending: 2 }, "launchFailure") }
  action connect = all { s.phase == 2, s.helloOK,
    record({ ...s, phase: 3 }, "connect") }
  action helloFailure = all { s.phase == 2, not(s.helloOK),
    record({ ...s, phase: 5, pending: 2 }, "helloFailure") }
  action cancel = all { 1.to(3).contains(s.phase),
    record({ ...s, phase: 5, pending: 3 }, "cancel") }
  // This transition is the sole submit/turn authorization point.
  action turn = all { s.phase == 3, s.pending == 0,
    record({ ...s, phase: 4, turns: s.turns + 1 }, "turn") }
  action finishTurn(ok: bool): bool = all { s.phase == 4,
    record({ ...s, phase: 5, pending: if (ok) 1 else 2 }, "finishTurn") }
  action abortTurn = all { s.phase == 4,
    record({ ...s, phase: 5, pending: 3 }, "abortTurn") }
  // These record capability attempts only. Enforcement is an external kernel
  // contract; neither action observes nor proves a filesystem mutation.
  action attemptROWrite = all { s.phase == 4, not(s.roWriteCap), not(s.roAttempt),
    record({ ...s, roAttempt: true }, "attemptROWrite") }
  action attemptEditWrite = all { s.phase == 4, s.editWriteCap, not(s.editAttempt),
    record({ ...s, editAttempt: true }, "attemptEditWrite") }
  action staleTeardown(g: int): bool = all {
    2.to(5).contains(s.phase), s.runtime, g != s.ownerGen,
    record(s, "staleTeardown") }
  action reap = all { s.phase == 5,
    (not(s.runtime) and not(s.proxy)) or
      (s.runtime and s.proxy and s.ownerGen == s.gen),
    record({ ...s, phase: 6, runtime: false, proxy: false,
      roWriteCap: false, editWriteCap: false }, "reap") }
  action terminal = all { s.phase == 6, 1.to(3).contains(s.pending),
    record({ ...s, phase: 7, result: s.pending }, "terminal") }
  action step = {
    nondet staleGen = Set(0, 1, 2).oneOf()
    any { hostPrep(true, true, true, true), hostPrep(true, true, true, false),
      hostPrep(false, true, true, true), hostPrep(true, false, true, true),
      hostPrep(true, true, false, true), prepFailure, launch, launchFailure,
      connect, helloFailure, cancel, turn, finishTurn(true), finishTurn(false),
      abortTurn, attemptROWrite, attemptEditWrite, staleTeardown(staleGen),
      reap, terminal }
  }

  // RO and explicit-edit capabilities are modeled separately as the assumed
  // bwrap bind contract. This checks the declared contract, not OS enforcement.
  val safety = all {
    0.to(7).contains(s.phase), 0.to(2).contains(s.gen),
    0.to(2).contains(s.ownerGen), 0.to(3).contains(s.pending),
    0.to(3).contains(s.result), 0.to(2).contains(s.turns),
    s.runtime == s.proxy, s.turns <= s.gen,
    s.runtime implies (s.ownerGen == s.gen and s.sandboxed),
    s.runtime implies (not(s.roWriteCap) and (s.editWriteCap == s.editable)),
    not(s.runtime) implies (not(s.roWriteCap) and not(s.editWriteCap)),
    not(s.fallback),
    event == "turn" implies (before.phase == 3 and before.platformOK
      and before.launchOK and before.helloOK and before.runtime and before.proxy
      and before.sandboxed and before.pending == 0 and s.turns == before.turns + 1),
    event != "turn" implies s.turns == before.turns,
    Set("prepFailure", "launchFailure", "helloFailure", "cancel")
      .contains(event) implies (s.turns == before.turns and s.phase == 5),
    event == "staleTeardown" implies (s == before and s.runtime
      and s.ownerGen == before.ownerGen),
    event == "reap" implies (before.phase == 5 and not(s.runtime) and not(s.proxy)),
    s.result == 1 implies (s.phase == 7 and not(s.runtime) and not(s.proxy)
      and s.pending == 1),
    s.phase == 7 implies (s.result != 0 and not(s.runtime) and not(s.proxy)),
    event == "terminal" implies (before.phase == 6 and not(before.runtime)
      and not(before.proxy) and s.result == before.pending),
    event == "attemptROWrite" implies (before.phase == 4 and not(before.roWriteCap)),
    event == "attemptEditWrite" implies (before.phase == 4 and before.editWriteCap)
  }

  run successTest = init.then(hostPrep(true, true, true, true)).then(launch)
    .then(connect).then(turn).then(finishTurn(true)).then(reap).then(terminal)
    .expect(s.result == 1 and s.turns == 1 and safety)
  run platformFailureTest = init.then(hostPrep(false, true, true, true))
    .then(prepFailure).then(reap).then(terminal)
    .expect(s.result == 2 and s.turns == 0 and not(s.runtime) and not(s.fallback) and safety)
  run launchFailureTest = init.then(hostPrep(true, false, true, true))
    .then(launchFailure).then(reap).then(terminal)
    .expect(s.result == 2 and s.turns == 0 and not(s.runtime) and not(s.fallback) and safety)
  run helloFailureTest = init.then(hostPrep(true, true, false, true)).then(launch)
    .then(helloFailure).then(reap).then(terminal)
    .expect(s.result == 2 and s.turns == 0 and not(s.runtime) and not(s.fallback) and safety)
  run cancelBeforeSubmitTest = init.then(hostPrep(true, true, true, true)).then(launch)
    .then(connect).then(cancel).then(reap).then(terminal)
    .expect(s.result == 3 and s.turns == 0 and safety)
  run abortedTurnTest = init.then(hostPrep(true, true, true, true)).then(launch)
    .then(connect).then(turn).then(abortTurn).then(reap).then(terminal)
    .expect(s.result == 3 and s.turns == 1 and not(s.runtime) and safety)
  run staleGenerationTest = init.then(hostPrep(true, true, true, true)).then(launch)
    .then(connect).then(turn).then(finishTurn(true)).then(reap).then(terminal)
    .then(hostPrep(true, true, true, true)).then(launch).then(staleTeardown(1))
    .expect(s.gen == 2 and s.ownerGen == 2 and s.runtime and s.proxy and safety)
  run effectCapabilitiesTest = init.then(hostPrep(true, true, true, true)).then(launch)
    .then(connect).then(turn).then(attemptROWrite).then(attemptEditWrite)
    .then(finishTurn(true)).then(reap).then(terminal)
    .expect(s.roAttempt and s.editAttempt and s.result == 1 and safety)
}
