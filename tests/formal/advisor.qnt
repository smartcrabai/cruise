// A single builtin advisor pass, possibly entered with advice from a prior visit.
module advisor {
  type Input = { enabled: bool, command: bool, userSkip: bool, skipApply: bool, stale: bool }
  type Phase = Init | Reviewing | AdviceReady | Continued | Stopped | Rejected | MissingAdvice
  type Visit = NoVisit | Success | OrdinaryFailure | WorkspaceChanged | Unverifiable | Interrupted | Skipped
  type Artifact = Absent | Stale | Fresh
  type State = { phase: Phase, visit: Visit, artifact: Artifact, turns: int, applies: int }
  var input: Input
  var s: State
  var previous: State

  pure def initial(i: Input): State = { phase: Init, visit: NoVisit,
    artifact: if (i.stale) Stale else Absent, turns: 0, applies: 0 }
  pure def canRun(i: Input): bool = i.enabled and not(i.userSkip)
  pure def fatal(v: Visit): bool = Set(WorkspaceChanged, Unverifiable, Interrupted).contains(v)
  action initWith(i: Input): bool = all { input' = i, s' = initial(i), previous' = initial(i) }
  action init = {
    nondet flag = Set(false, true).oneOf()
    nondet command = Set(false, true).oneOf()
    nondet userSkip = Set(false, true).oneOf()
    nondet skipApply = Set(false, true).oneOf()
    nondet stale = Set(false, true).oneOf()
    initWith({ enabled: flag, command: command, userSkip: userSkip, skipApply: skipApply, stale: stale })
  }
  action record(updated: State): bool = all { input' = input, previous' = s, s' = updated }
  action reject = all {
    s.phase == Init, canRun(input), input.command,
    record({ ...s, phase: Rejected })
  }
  action skip = all {
    s.phase == Init, not(canRun(input)),
    record({ ...s, phase: if (input.enabled and not(input.skipApply)) MissingAdvice else Continued,
      visit: Skipped, artifact: Absent })
  }
  action begin = all {
    s.phase == Init, canRun(input), not(input.command),
    record({ ...s, phase: Reviewing, artifact: Absent, turns: s.turns + 1 })
  }
  pure val finishOutcomes = Set(Success, OrdinaryFailure, WorkspaceChanged, Unverifiable, Interrupted)
  action finishWith(v: Visit): bool = all {
    s.phase == Reviewing,
    finishOutcomes.contains(v),
    record({ ...s, visit: v, phase: if (v == Success) AdviceReady else if (fatal(v)) Stopped else Continued,
      artifact: if (v == Success) Fresh else s.artifact })
  }
  action finish = {
    nondet v = finishOutcomes.oneOf()
    finishWith(v)
  }
  action applyAdvice = all {
    s.phase == AdviceReady, s.artifact == Fresh,
    record({ ...s, phase: Continued, applies: s.applies + (if (input.skipApply) 0 else 1) })
  }
  action idle = all { Set(Rejected, Stopped, Continued, MissingAdvice).contains(s.phase), record(s) }
  action step = any { reject, skip, begin, finish, applyAdvice, idle }

  val safety = all {
    s.applies > previous.applies implies
      previous.phase == AdviceReady and previous.visit == Success and previous.artifact == Fresh,
    s.applies <= s.turns and s.turns <= 1,
    input.skipApply implies s.applies == 0,
    s.applies == 1 implies s.phase == Continued and s.visit == Success,
    s.artifact == Fresh iff s.visit == Success,
    s.phase == Reviewing implies s.artifact == Absent,
    fatal(s.visit) implies s.phase == Stopped and s.applies == 0 and s.artifact == Absent,
    s.visit == OrdinaryFailure implies s.phase == Continued and s.applies == 0 and s.artifact == Absent,
    not(canRun(input)) implies s.turns == 0 and s.applies == 0 and s.phase != Rejected,
    s.visit == Skipped implies Set(Continued, MissingAdvice).contains(s.phase) and s.artifact == Absent,
    s.phase == MissingAdvice implies s.visit == Skipped and s.applies == 0 and s.artifact == Absent,
    canRun(input) and input.command implies
      s.turns == 0 and s.applies == 0 and Set(Init, Rejected).contains(s.phase),
  }

  pure val active = { enabled: true, command: false, userSkip: false, skipApply: false, stale: true }
  run successTest = initWith(active).then(begin).expect(s.artifact == Absent and safety)
    .then(finishWith(Success)).then(applyAdvice)
    .expect(s.phase == Continued and s.applies == 1 and safety).then(idle).expect(safety)
  run skipApplyTest = initWith({ ...active, skipApply: true }).then(begin)
    .then(finishWith(Success)).then(applyAdvice)
    .expect(s.phase == Continued and s.applies == 0 and s.artifact == Fresh and safety)
  run failureTest = initWith(active).then(begin).then(finishWith(OrdinaryFailure))
    .expect(s.phase == Continued and s.artifact == Absent and s.applies == 0 and safety)
  run changedTest = initWith(active).then(begin).then(finishWith(WorkspaceChanged))
    .then(idle).expect(s.phase == Stopped and safety)
  run unverifiableTest = initWith(active).then(begin).then(finishWith(Unverifiable))
    .then(idle).expect(s.phase == Stopped and safety)
  run interruptedTest = initWith(active).then(begin).then(finishWith(Interrupted))
    .then(idle).expect(s.phase == Stopped and safety)
  run flagOffTest = initWith({ ...active, enabled: false, command: true }).then(skip)
    .expect(s.visit == Skipped and s.artifact == Absent and safety)
  run explicitSkipTest = initWith({ ...active, userSkip: true, skipApply: true, command: true }).then(skip)
    .expect(s.phase == Continued and s.visit == Skipped and s.artifact == Absent and safety)
  run skippedAdvisorOnlyTest = initWith({ ...active, userSkip: true }).then(skip)
    .expect(s.phase == MissingAdvice and s.applies == 0 and safety)
  run commandRejectedTest = initWith({ ...active, command: true }).then(reject)
    .expect(s.phase == Rejected and safety)
}
