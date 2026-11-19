// Bounded abstraction of the per-user daemon lifecycle. Two PIDs and process
// identities expose stale records and PID reuse. Requests linearize while the
// registry lock is free; a waiting autostart cannot create a second child.
// Port binding, version compatibility, and API authorization are abstract
// outcomes, not models of sockets, HTTP parsing, or constant-time comparison.
module daemon {
  type Proc = { pid: int, identity: int, instance: int }
  type Record = { present: bool, pid: int, identity: int, instance: int }
  type State = { record: Record, procs: Set[Proc], lock: str,
    child: Proc, bound: bool, sessions: Set[str], workActive: bool,
    versionMismatch: bool, ownerToken: int, requestToken: int,
    requestAccepted: bool, loginNonce: int, nonceAvailable: bool,
    loginAccepted: bool, webSession: int,
    bindOutcome: str, clientOutcome: str }
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
  pure def withoutOwner(ps: Set[Proc], r: Record): Set[Proc] =
    ps.filter(p => not(p.pid == r.pid and p.identity == r.identity))

  action init = all {
    s' = { record: emptyRecord, procs: Set(), lock: "free", child: noChild,
      bound: false, sessions: Set("planned", "suspended"), workActive: false,
      versionMismatch: false, ownerToken: 0, requestToken: 0,
      requestAccepted: false, loginNonce: 0, nonceAvailable: false,
      loginAccepted: false, webSession: 0,
      bindOutcome: "none", clientOutcome: "none" },
    before' = { record: emptyRecord, procs: Set(), lock: "free", child: noChild,
      bound: false, sessions: Set("planned", "suspended"), workActive: false,
      versionMismatch: false, ownerToken: 0, requestToken: 0,
      requestAccepted: false, loginNonce: 0, nonceAvailable: false,
      loginAccepted: false, webSession: 0,
      bindOutcome: "none", clientOutcome: "none" },
    event' = "init", statusRunning' = false, signalPid' = 0, signalIdentity' = 0,
  }
  // The parent holds the registry lock through child publication and readiness.
  action startBegin(p: Proc): bool = all {
    s.lock == "free", not(live(s.record, s.procs)), pidFree(p.pid, s.procs),
    before' = s, event' = "startBegin", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: emptyRecord, lock: "start", child: p,
      loginNonce: 0, nonceAvailable: false, loginAccepted: false, webSession: 0,
      procs: s.procs.union(Set(p)), bound: false },
  }
  // A client autostarts only after taking the same exclusive registry lock.
  action autostartBegin(p: Proc): bool = all {
    s.lock == "free", not(live(s.record, s.procs)), pidFree(p.pid, s.procs),
    before' = s, event' = "autostartBegin", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: emptyRecord, lock: "start", child: p,
      loginNonce: 0, nonceAvailable: false, loginAccepted: false, webSession: 0,
      procs: s.procs.union(Set(p)), bound: false, clientOutcome: "starting" },
  }
  // Contending clients wait for the current lock holder; they do not spawn.
  action autostartWait = all {
    s.lock == "start",
    before' = s, event' = "autostartWait", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0, s' = s,
  }
  action autostartExisting = all {
    s.lock == "free", live(s.record, s.procs),
    before' = s, event' = "autostartExisting", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, clientOutcome: "existing" },
  }
  action childPublish = all {
    s.lock == "start", s.child != noChild, not(s.bound),
    s.procs.contains(s.child),
    not(live(s.record, s.procs)) or s.record.instance == s.child.instance,
    before' = s, event' = "childPublish", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: ownRecord(s.child), bound: true,
      ownerToken: s.child.instance, loginNonce: 0, nonceAvailable: false,
      loginAccepted: false, webSession: 0 },
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
    s' = { ...s, record: ownRecord(p), procs: s.procs.union(Set(p)),
      ownerToken: p.instance, loginNonce: 0, nonceAvailable: false,
      loginAccepted: false, webSession: 0 },
  }
  // This action abstracts an authenticated POST /api/v1/shutdown.
  action apiShutdown = all {
    s.lock == "free", live(s.record, s.procs),
    before' = s, event' = "apiShutdown", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: emptyRecord, ownerToken: 0,
      loginNonce: 0, nonceAvailable: false, loginAccepted: false, webSession: 0,
      procs: withoutOwner(s.procs, s.record) },
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
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: emptyRecord, ownerToken: 0, loginNonce: 0,
      nonceAvailable: false, loginAccepted: false, webSession: 0 },
  }
  action status = all {
    before' = s, event' = "status", statusRunning' = live(s.record, s.procs),
    signalPid' = 0, signalIdentity' = 0, s' = s,
  }
  // The daemon exits when its record disappears or another instance owns it.
  action registryDisappears = all {
    s.lock == "free", live(s.record, s.procs),
    before' = s, event' = "registryDisappears", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: emptyRecord, ownerToken: 0, loginNonce: 0,
      nonceAvailable: false, loginAccepted: false, webSession: 0,
      procs: withoutOwner(s.procs, s.record) },
  }
  action registryTakeover(p: Proc): bool = all {
    s.lock == "free", live(s.record, s.procs), pidFree(p.pid, s.procs),
    p.instance != s.record.instance,
    before' = s, event' = "registryTakeover", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: ownRecord(p), ownerToken: p.instance,
      loginNonce: 0, nonceAvailable: false, loginAccepted: false, webSession: 0,
      procs: withoutOwner(s.procs, s.record).union(Set(p)), bound: true },
  }
  // Bind results abstract default-port collision handling, not socket APIs.
  action unspecifiedPortBusyFallback = all {
    before' = s, event' = "unspecifiedPortBusyFallback", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, bindOutcome: "ephemeral" },
  }
  action explicitPortBusyFailure = all {
    before' = s, event' = "explicitPortBusyFailure", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, bindOutcome: "failed" },
  }
  action markVersionMismatch(work: bool): bool = all {
    s.lock == "free", live(s.record, s.procs),
    before' = s, event' = "markVersionMismatch", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, versionMismatch: true, workActive: work },
  }
  action versionMismatchIdleRestart(p: Proc): bool = all {
    s.lock == "free", live(s.record, s.procs), s.versionMismatch,
    not(s.workActive), pidFree(p.pid, s.procs), p.instance != s.record.instance,
    before' = s, event' = "versionMismatchIdleRestart", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, record: ownRecord(p), ownerToken: p.instance,
      loginNonce: 0, nonceAvailable: false, loginAccepted: false, webSession: 0,
      procs: withoutOwner(s.procs, s.record).union(Set(p)),
      versionMismatch: false, clientOutcome: "restarted" },
  }
  action versionMismatchBusyRefusal = all {
    s.lock == "free", live(s.record, s.procs), s.versionMismatch, s.workActive,
    before' = s, event' = "versionMismatchBusyRefusal", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, clientOutcome: "refused" },
  }
  action apiRequest(token: int): bool = all {
    s.lock == "free", live(s.record, s.procs),
    before' = s, event' = "apiRequest", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, requestToken: token, requestAccepted: token == s.ownerToken },
  }

  // The authenticated API issues a short-lived nonce that is consumed once.
  action requestLoginNonce(token: int, nonce: int): bool = all {
    s.lock == "free", live(s.record, s.procs), token == s.ownerToken,
    nonce != 0, nonce != s.ownerToken,
    before' = s, event' = "requestLoginNonce", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, loginNonce: nonce, nonceAvailable: true, loginAccepted: false },
  }
  action consumeLoginNonce(nonce: int, session: int): bool = all {
    s.lock == "free", live(s.record, s.procs), s.nonceAvailable,
    nonce == s.loginNonce, session != 0, session != s.ownerToken,
    before' = s, event' = "consumeLoginNonce", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, nonceAvailable: false, loginAccepted: true, webSession: session },
  }
  action reuseLoginNonce(nonce: int): bool = all {
    s.lock == "free", live(s.record, s.procs), not(s.nonceAvailable),
    nonce != 0, nonce == s.loginNonce,
    before' = s, event' = "reuseLoginNonce", statusRunning' = false,
    signalPid' = 0, signalIdentity' = 0,
    s' = { ...s, loginAccepted: false },
  }
  action lifecycleStep = {
    nondet pid = Set(1, 2).oneOf()
    nondet identity = Set(1, 2).oneOf()
    nondet instance = Set(1, 2).oneOf()
    nondet loginToken = Set(0, 1, 2).oneOf()
    nondet nonce = Set(0, 1, 2).oneOf()
    nondet session = Set(0, 1, 2).oneOf()
    val p = candidate(pid, identity, instance)
    any { startBegin(p), autostartBegin(p), autostartWait, autostartExisting,
      childPublish, childBindFail, startFinish, foregroundStart(p), apiShutdown,
      crash(p), pidReuse(p), stopSignal(pid, identity), stop, status,
      registryDisappears, registryTakeover(p), unspecifiedPortBusyFallback,
      explicitPortBusyFailure, markVersionMismatch(true), markVersionMismatch(false),
      versionMismatchIdleRestart(p), versionMismatchBusyRefusal,
      apiRequest(0), apiRequest(1), apiRequest(2),
      requestLoginNonce(loginToken, nonce), consumeLoginNonce(nonce, session),
      reuseLoginNonce(nonce) }
  }
  action step = lifecycleStep

  // Include a pending child as a live owner candidate while the parent holds
  // the lock, so autostart contenders cannot overlap the readiness handshake.
  val oneLiveInstance = s.procs.filter(p => p.instance != 0 and
    (s.record.present and p.pid == s.record.pid and p.identity == s.record.identity and p.instance == s.record.instance or
     s.child != noChild and p == s.child)).size() <= 1
  val startOnlyWhenNoLiveRecord = Set("childPublish", "foregroundStart", "autostartBegin").contains(event)
    implies (not(live(before.record, before.procs)) or
      event == "childPublish" and before.record.instance == before.child.instance)
  val autostartUsesRegistryLock = event == "autostartBegin" implies
    before.lock == "free" and not(live(before.record, before.procs))
  val stopSignalsOnlyIdentityMatch = event == "stopSignal" implies all {
    before.record.present, signalPid == before.record.pid,
    signalIdentity == before.record.identity, live(before.record, before.procs),
  }
  val registryChangeStopsOldOwner = Set("registryDisappears", "registryTakeover").contains(event)
    implies not(s.procs.exists(p => p.pid == before.record.pid and p.identity == before.record.identity))
  val unspecifiedBindFallsBack = event == "unspecifiedPortBusyFallback" implies
    s.bindOutcome == "ephemeral"
  val explicitBindIsStrict = event == "explicitPortBusyFailure" implies
    s.bindOutcome == "failed"
  val mismatchRestartsOnlyWhenIdle = event == "versionMismatchIdleRestart" implies
    before.versionMismatch and not(before.workActive) and s.clientOutcome == "restarted"
  val mismatchBusyIsRefused = event == "versionMismatchBusyRefusal" implies
    before.versionMismatch and before.workActive and s.clientOutcome == "refused" and
      s.record == before.record and s.procs == before.procs
  val apiAuthorizationMatchesOwner = event == "apiRequest" implies
    (s.requestAccepted == (s.requestToken == before.ownerToken))
  val lifecycleLeavesSessionsUnchanged = s.sessions == before.sessions
  val statusMatchesIdentity = event == "status" implies
    (statusRunning == live(before.record, before.procs))
  val loginNonceIsSingleUse =
    (event == "consumeLoginNonce" implies before.nonceAvailable and
      not(s.nonceAvailable) and s.loginAccepted) and
    (event == "reuseLoginNonce" implies not(before.nonceAvailable) and
      not(s.nonceAvailable) and not(s.loginAccepted) and
      s.webSession == before.webSession)
  val loginSecretsAreSeparated =
    (s.loginNonce == 0 or s.loginNonce != s.ownerToken) and
    (s.webSession == 0 or s.webSession != s.ownerToken)
  val safety = oneLiveInstance and startOnlyWhenNoLiveRecord and
    autostartUsesRegistryLock and stopSignalsOnlyIdentityMatch and
    registryChangeStopsOldOwner and unspecifiedBindFallsBack and
    explicitBindIsStrict and mismatchRestartsOnlyWhenIdle and
    mismatchBusyIsRefused and apiAuthorizationMatchesOwner and
    loginNonceIsSingleUse and loginSecretsAreSeparated and
    lifecycleLeavesSessionsUnchanged and statusMatchesIdentity

  run daemonRoundTripTest = init.then(startBegin(candidate(1, 1, 1))).then(childPublish)
    .then(startFinish).then(status).expect(statusRunning and safety)
    .then(apiShutdown).then(status).expect(not(statusRunning) and safety)
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
    .then(apiShutdown).then(startBegin(candidate(2, 1, 2))).then(childPublish)
    .then(startFinish).then(status)
    .expect(statusRunning and s.record.instance == 2 and safety)
  run parentHoldsLockTest = init.then(startBegin(candidate(1, 1, 1)))
    .expect(s.lock == "start" and s.child == candidate(1, 1, 1) and safety)
  run autostartSingleOwnerTest = init.then(autostartBegin(candidate(1, 1, 1)))
    .then(childPublish)
    .expect(s.lock == "start" and live(s.record, s.procs) and safety)
    .then(autostartWait)
    .expect(s.lock == "start" and s.procs.size() == 1 and safety)
    .then(startFinish).then(autostartExisting)
    .expect(s.procs.size() == 1 and s.clientOutcome == "existing" and safety)
  run registryDisappearanceShutdownTest = init.then(foregroundStart(candidate(1, 1, 1)))
    .then(registryDisappears)
    .expect(registryChangeStopsOldOwner and not(live(s.record, s.procs)) and safety)
  run registryTakeoverShutdownTest = init.then(foregroundStart(candidate(1, 1, 1)))
    .then(registryTakeover(candidate(2, 2, 2)))
    .expect(registryChangeStopsOldOwner and live(s.record, s.procs) and
      s.record.instance == 2 and safety)
  run unspecifiedPortFallbackTest = init.then(unspecifiedPortBusyFallback)
    .expect(unspecifiedBindFallsBack and s.bindOutcome == "ephemeral" and safety)
  run explicitPortStrictnessTest = init.then(explicitPortBusyFailure)
    .expect(explicitBindIsStrict and s.bindOutcome == "failed" and safety)
  run versionMismatchIdleRestartTest = init.then(foregroundStart(candidate(1, 1, 1)))
    .then(markVersionMismatch(false)).then(versionMismatchIdleRestart(candidate(2, 2, 2)))
    .expect(mismatchRestartsOnlyWhenIdle and s.record.instance == 2 and safety)
  run versionMismatchBusyRefusalTest = init.then(foregroundStart(candidate(1, 1, 1)))
    .then(markVersionMismatch(true)).then(versionMismatchBusyRefusal)
    .expect(mismatchBusyIsRefused and s.record.instance == 1 and safety)
  run apiAuthorizationTest = init.then(foregroundStart(candidate(1, 1, 1)))
    .then(apiRequest(0)).expect(not(s.requestAccepted) and apiAuthorizationMatchesOwner and safety)
    .then(apiRequest(1)).expect(s.requestAccepted and apiAuthorizationMatchesOwner and safety)

  run loginNonceSingleUseTest = init.then(foregroundStart(candidate(1, 1, 1)))
    .then(requestLoginNonce(1, 2)).expect(s.nonceAvailable and loginSecretsAreSeparated and safety)
    .then(consumeLoginNonce(2, 2)).expect(s.loginAccepted and not(s.nonceAvailable) and safety)
    .then(reuseLoginNonce(2)).expect(not(s.loginAccepted) and not(s.nonceAvailable) and safety)
  run webSessionSeparateFromApiTokenTest = init.then(foregroundStart(candidate(1, 1, 1)))
    .then(requestLoginNonce(1, 2)).then(consumeLoginNonce(2, 2))
    .expect(s.webSession != s.ownerToken and loginSecretsAreSeparated and safety)
}
