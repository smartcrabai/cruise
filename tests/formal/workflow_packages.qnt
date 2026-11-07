// Abstract one bundle update/remove. Renames are atomic and staged files are
// complete before promotion. One previous directory, no external modifications.
module workflow_packages {
  type State = { finalDir: str, previous: str, staging: str,
    removing: bool, crashed: bool, lockMatches: bool }
  var s: State
  var before: State
  var event: str

  pure val initial: State = { finalDir: "Old", previous: "NoPrev",
    staging: "NoStg", removing: false, crashed: false, lockMatches: true }
  action init = all { s' = initial, before' = initial, event' = "init" }
  action record(newState: State, name: str): bool = all {
    before' = s, s' = newState, event' = name,
  }
  action stage = all { not(s.crashed), s.finalDir == "Old",
    s.previous == "NoPrev", s.staging == "NoStg", not(s.removing),
    record({ ...s, staging: "StgNew" }, "stage") }
  action backup = all { not(s.crashed), s.finalDir == "Old",
    s.staging == "StgNew", s.previous == "NoPrev",
    record({ ...s, finalDir: "Absent", previous: "PrevOld", lockMatches: false }, "backup") }
  action promote = all { not(s.crashed), s.finalDir == "Absent",
    s.previous == "PrevOld", s.staging == "StgNew",
    record({ ...s, finalDir: "New", staging: "NoStg", lockMatches: true }, "promote") }
  action cleanupPrev = all { not(s.crashed), s.finalDir == "New", s.previous == "PrevOld",
    record({ ...s, previous: "NoPrev" }, "cleanupPrev") }
  action crash = all { not(s.crashed), record({ ...s, crashed: true }, "crash") }
  action recover = all { s.crashed,
    val restored = s.finalDir == "Absent" and s.previous == "PrevOld"
    record({ ...s, finalDir: if (restored) "Old" else s.finalDir,
      previous: "NoPrev", staging: "NoStg", removing: false, crashed: false,
      lockMatches: restored or s.finalDir != "Absent" }, "recover") }
  action removeStart = all { not(s.crashed), s.finalDir != "Absent",
    s.previous == "NoPrev", s.staging == "NoStg", not(s.removing),
    record({ ...s, finalDir: "Absent", removing: true, lockMatches: false }, "removeStart") }
  action removeDone = all { not(s.crashed), s.removing,
    record({ ...s, removing: false }, "removeDone") }
  action idle = record(s, "idle")
  action step = any { stage, backup, promote, cleanupPrev, crash, recover,
    removeStart, removeDone, idle }

  val i1_whole = Set("Absent", "Old", "New").contains(s.finalDir)
  val i2_lock = s.finalDir != "Absent" implies s.lockMatches
  val i3_recovery_cleanup = event == "recover" implies
    s.staging == "NoStg" and not(s.removing)
  val i4_recovery_restore = event == "recover" and before.previous == "PrevOld"
    and before.finalDir == "Absent" implies s.finalDir == "Old"
  // Recovery never replaces an already visible bundle, even with a stale backup.
  val i5_recovery_preserve = event == "recover" and before.finalDir != "Absent"
    implies s.finalDir == before.finalDir and s.previous == "NoPrev"
  // Once removal has renamed the final away, recovery must not resurrect it.
  val i6_remove_no_resurrection = event == "recover" and before.removing
    implies s.finalDir == "Absent" and s.previous == "NoPrev"
  val safety = i1_whole and i2_lock and i3_recovery_cleanup and i4_recovery_restore
    and i5_recovery_preserve and i6_remove_no_resurrection

  run restoreOldTest = init.then(stage).then(backup).then(crash).then(recover)
    .expect(s.finalDir == "Old" and safety)
  run updateNewTest = init.then(stage).then(backup).then(promote).then(cleanupPrev)
    .expect(s.finalDir == "New" and safety)
  run interruptedStageTest = init.then(stage).then(crash).then(recover)
    .expect(s.finalDir == "Old" and safety)
  run interruptedRemoveTest = init.then(removeStart).then(crash).then(recover)
    .expect(s.finalDir == "Absent" and not(s.removing) and safety)
  run interruptedAfterPromoteTest = init.then(stage).then(backup).then(promote)
    .then(crash).then(recover)
    .expect(s.finalDir == "New" and s.previous == "NoPrev" and safety)
  run interruptedAfterRemoveDoneTest = init.then(removeStart).then(removeDone)
    .then(crash).then(recover)
    .expect(s.finalDir == "Absent" and safety)
}
