// Abstract lifecycle protocol from the WebUI daemon plan. Two PIDs and two
// process generations expose stale records and PID reuse. Instance IDs are
// distinct from process identities. Network, token validation, and timeouts are
// outside this model. A status result and a signal are checked at their
// linearization point, not against processes that may later exit or restart.
module webui_daemon {
  type Proc = { pid: int, identity: int, instance: int }
  type Record = { present: bool, pid: int, identity: int, instance: int }
  type State = { record: Record, procs: Set[Proc], lock: str,
    child: Proc, bound: bool, sessions: Set[str] }
  val emptyRecord = { present: false, pid: 0, identity: 0, instance: 0 }
  val noChild = { pid: 0, identity: 0, instance: 0 }
  var s: State
  var before: State
  var event: str
  var statusRunning: bool
  var signalPid: int
  var signalIdentity: int

  pure def live(r: Record, ps: Set[Proc]): bool =
    r.present and ps.exists(p => p.pid == r.pid and p.identity == r.identity)
  pure def ownRecord(p: Proc): Record =
    { present: true, pid: p.pid, identity: p.identity, instance: p.instance }
  pure def pidFree(pid: int, ps: Set[Proc]): bool = not(ps.exists(p => p.pid == pid))
  pure def candidate(pid: int, identity: int, instance: int): Proc =
    { pid: pid, identity: identity, instance: instance }

  action init = all {
    s' = { record: emptyRecord, procs: Set(), lock: "free", child: noChild,
      bound: false, sessions: Set("planned", "suspended") },
    before' = { record: emptyRecord, procs: Set(), lock: "free", child: noChild,
      bound: false, sessions: Set("planned", "suspended") },
    event' = "init", statusRunning' = false, signalPid' = 0, signalIdentity' = 0,
  }
  // Parent owns the lock through child publication and readiness handshake.
  action startBegin(p: Proc): bool = all {
    s.lock == "free", not(live(s.record, s.procs)), pidFree(p.pid, s.procs),
    before' = s, event' = "startBegin", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: emptyRecord, lock: "start", child: p,
      procs: s.procs.union(Set(p)), bound: false },
  }
  action childPublish = all {
    s.lock == "start", s.child != noChild, not(s.bound),
    s.procs.contains(s.child),
    not(live(s.record, s.procs)) or s.record.instance == s.child.instance,
    before' = s, event' = "childPublish", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: ownRecord(s.child), bound: true },
  }
  action childBindFail = all {
    s.lock == "start", not(s.bound), s.procs.contains(s.child),
    before' = s, event' = "childBindFail", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, procs: s.procs.exclude(Set(s.child)) },
  }
  action startFinish = all {
    s.lock == "start", s.bound or not(s.procs.contains(s.child)),
    before' = s, event' = "startFinish", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, lock: "free", child: noChild, bound: false },
  }
  action foregroundStart(p: Proc): bool = all {
    s.lock == "free", not(live(s.record, s.procs)), pidFree(p.pid, s.procs),
    before' = s, event' = "foregroundStart", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: ownRecord(p), procs: s.procs.union(Set(p)) },
  }
  // Control stop abstracts a successful authenticated shutdown and owned cleanup.
  action controlStop = all {
    s.lock == "free", live(s.record, s.procs),
    before' = s, event' = "controlStop", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: emptyRecord,
      procs: s.procs.filter(p => not(p.pid == s.record.pid and p.identity == s.record.identity)) },
  }
  action crash(p: Proc): bool = all {
    s.procs.contains(p), before' = s, event' = "crash", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, procs: s.procs.exclude(Set(p)) },
  }
  // The new process is unrelated, so it cannot be the registered instance.
  action pidReuse(p: Proc): bool = all {
    s.record.present, p.pid == s.record.pid, p.identity != s.record.identity,
    p.instance != s.record.instance, pidFree(p.pid, s.procs),
    before' = s, event' = "pidReuse", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, procs: s.procs.union(Set(p)) },
  }
  // Signal is sent only after rechecking both components of process identity.
  action stopSignal(pid: int, identity: int): bool = all {
    s.lock == "free", live(s.record, s.procs),
    pid == s.record.pid, identity == s.record.identity,
    before' = s, event' = "stopSignal", statusRunning' = false,
    signalPid' = pid, signalIdentity' = identity,
    s' = { ...s, procs: s.procs.filter(p => not(p.pid == pid and p.identity == identity)) },
  }
  action stop = all {
    s.lock == "free", not(live(s.record, s.procs)),
    before' = s, event' = "stop", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0, s' = { ...s, record: emptyRecord },
  }
  action status = all {
    before' = s, event' = "status", statusRunning' = live(s.record, s.procs),
    signalPid' = 0, signalIdentity' = 0, s' = s,
  }
  action lifecycleStep = {
    nondet pid = Set(1, 2).oneOf()
    nondet identity = Set(1, 2).oneOf()
    nondet instance = Set(1, 2).oneOf()
    val p = candidate(pid, identity, instance)
    any { startBegin(p), childPublish, childBindFail, startFinish,
      foregroundStart(p), controlStop, crash(p), pidReuse(p),
      stopSignal(pid, identity), stop, status }
  }
  action step = lifecycleStep

  // The registry has one slot. Also forbid two live WebUI owners, rather than
  // relying only on the cardinality of that slot. Reused PIDs are unrelated.
  val oneLiveInstance = s.procs.filter(p => p.instance != 0 and
    (s.record.present and p.pid == s.record.pid and p.identity == s.record.identity and p.instance == s.record.instance or
     s.child != noChild and p == s.child)).size() <= 1
  val startOnlyWhenNoLiveRecord = Set("childPublish", "foregroundStart").contains(event)
    implies (not(live(before.record, before.procs)) or
      event == "childPublish" and before.record.instance == before.child.instance)
  val stopSignalsOnlyIdentityMatch = event == "stopSignal" implies all {
    before.record.present, signalPid == before.record.pid,
    signalIdentity == before.record.identity, live(before.record, before.procs),
  }
  val lifecycleLeavesSessionsUnchanged = s.sessions == before.sessions
  val statusMatchesIdentity = event == "status" implies
    (statusRunning == live(before.record, before.procs))
  val safety = oneLiveInstance and startOnlyWhenNoLiveRecord and
    stopSignalsOnlyIdentityMatch and lifecycleLeavesSessionsUnchanged and statusMatchesIdentity

  run daemonRoundTripTest = init.then(startBegin(candidate(1, 1, 1))).then(childPublish)
    .then(startFinish).then(status).expect(statusRunning and safety)
    .then(controlStop).then(status).expect(not(statusRunning) and safety)
  run bindFailureTest = init.then(startBegin(candidate(1, 1, 1))).then(childBindFail)
    .then(startFinish).then(status).expect(not(statusRunning) and safety)
  run stalePidReuseTest = init.then(foregroundStart(candidate(1, 1, 1)))
    .then(crash(candidate(1, 1, 1))).then(pidReuse(candidate(1, 2, 2)))
    .then(status).expect(not(statusRunning) and safety)
    .then(stop).expect(s.procs.contains(candidate(1, 2, 2)) and safety)
  run staleReplacementTest = init.then(foregroundStart(candidate(1, 1, 1)))
    .then(crash(candidate(1, 1, 1))).then(startBegin(candidate(2, 1, 2)))
    .then(childPublish).then(startFinish).expect(live(s.record, s.procs) and safety)
  run signalMatchingIdentityTest = init.then(foregroundStart(candidate(1, 1, 1)))
    .then(stopSignal(1, 1)).expect(stopSignalsOnlyIdentityMatch and safety)
  // Restart is stop followed by a fresh locked start, not an in-place mutation.
  run restartRoundTripTest = init.then(foregroundStart(candidate(1, 1, 1)))
    .then(controlStop).then(startBegin(candidate(2, 1, 2))).then(childPublish)
    .then(startFinish).then(status)
    .expect(statusRunning and s.record.instance == 2 and safety)
  // A contender cannot acquire the lock while a parent awaits publication.
  run parentHoldsLockTest = init.then(startBegin(candidate(1, 1, 1)))
    .expect(s.lock == "start" and s.child == candidate(1, 1, 1) and safety)
}
