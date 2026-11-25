// Bounded abstraction of one Manager reservation, from `retry` or dispatch saving the
// attempt until its child launches, while sibling children integrate and the daemon
// restarts inside dispatch. The pin is the attempt's base SHA; publication is its
// owned session directory. Atomic actions abstract the control and writer locks.
// Does not model Git, the filesystem, capacity, pause, or the child's own result.
// Production: manager_coordinator::retry_task, reserve_and_claim, repin_reservation,
// reactivate_stale_reservation, claim_reserved_child and integrate.
module manager_reservation {
  type State = { tip: int, siblings: int, blocked: bool, reserved: bool, pin: int,
    published: bool, launches: int, base: int }
  var s: State
  var previous: State
  var event: str
  // `reserved` is a saved attempt with `launch_started == false`. `siblings` counts
  // completed children that an active Goal integrates before it dispatches.
  pure val initial: State = { tip: 0, siblings: 2, blocked: false, reserved: false, pin: -1,
    published: false, launches: 0, base: -1 }
  action init = all { s' = initial, previous' = initial, event' = "init" }
  action record(n: State, e: str): bool = all { previous' = s, s' = n, event' = e }
  action reserve = all { not(s.blocked), not(s.reserved), s.launches == 0,
    record({ ...s, reserved: true, pin: s.tip }, "reserve") }
  action integrateSibling = all { not(s.blocked), s.siblings > 0,
    record({ ...s, tip: s.tip + 1, siblings: s.siblings - 1 }, "integrate") }
  // Only an unpublished reservation follows the tip; the child then starts from it.
  val current = s.pin == s.tip or not(s.published)
  action dispatch = all { not(s.blocked), s.reserved,
    if (current) record({ ...s, reserved: false, pin: s.tip, published: true,
      launches: s.launches + 1, base: s.tip }, "launch")
    else record({ ...s, blocked: true }, "block") }
  // The daemon stops after the durable re-pin, or after publishing the session.
  action stopAfterRepin = all { not(s.blocked), s.reserved, not(s.published),
    record({ ...s, pin: s.tip }, "stopAfterRepin") }
  action stopAfterPublish = all { not(s.blocked), s.reserved, current,
    record({ ...s, pin: s.tip, published: true }, "stopAfterPublish") }
  // Earlier versions blocked every stale reservation, so such records exist on disk.
  action legacyBlock = all { not(s.blocked), s.reserved, s.pin != s.tip,
    record({ ...s, blocked: true }, "legacyBlock") }
  // `cruise manager retry GOAL TASK` on the blocked Goal: no second attempt is saved.
  val recoverable = s.blocked and s.reserved and not(s.published)
  action reactivate = all { recoverable, record({ ...s, blocked: false }, "reactivate") }
  action rejectRetry = all { not(recoverable), s.blocked or s.reserved or s.launches > 0,
    record(s, "rejectRetry") }
  action idle = record(s, "idle")
  action step = any { reserve, integrateSibling, dispatch, stopAfterRepin, stopAfterPublish,
    legacyBlock, reactivate, rejectRetry, idle }

  val launchedOnce = s.launches <= 1
  // The freshness check is never bypassed: a child starts from the tip at dispatch.
  val startsFromTip = event == "launch" implies
    s.base == previous.tip and s.base == s.tip and s.pin == s.base
  // A session published under a pin is never re-pinned, only launched from that pin.
  val publishedPinIsFinal = previous.published implies s.pin == previous.pin
  val blockedSafety = previous.blocked implies
    s == previous or (event == "reactivate" and not(previous.published))
  val recoveryPreserves = event == "reactivate" implies
    s == { ...previous, blocked: false } and s.reserved and s.launches == 0
  val lifecycle = all {
    s.blocked implies s.reserved,
    s.reserved implies s.launches == 0 and s.pin <= s.tip,
    s.published implies s.reserved or s.launches == 1,
    s.launches == 1 implies s.published and s.base == s.pin,
  }
  val safety = launchedOnce and startsFromTip and publishedPinIsFinal and blockedSafety
    and recoveryPreserves and lifecycle

  run siblingIntegrationRepinsTest = init.then(reserve).then(integrateSibling).then(dispatch)
    .expect(s.launches == 1 and s.base == 1 and not(s.blocked) and safety)
  run legacyBlockRecoversTest = init.then(reserve).then(integrateSibling).then(legacyBlock)
    .then(reactivate).then(integrateSibling).then(dispatch)
    .expect(s.launches == 1 and s.base == 2 and not(s.blocked) and safety)
  run recoveredReservationLaunchesOnceTest = init.then(reserve).then(integrateSibling)
    .then(legacyBlock).then(reactivate).then(dispatch).then(dispatch).fail()
  run retryAfterLaunchIsRejectedTest = init.then(reserve).then(integrateSibling).then(dispatch)
    .then(rejectRetry).expect(s.launches == 1 and safety)
  run retryAfterLaunchDoesNotReserveTest = init.then(reserve).then(dispatch).then(reserve).fail()
  run publishedStaleStaysBlockedTest = init.then(reserve).then(stopAfterPublish)
    .then(integrateSibling).then(dispatch).then(rejectRetry)
    .expect(s.blocked and s.launches == 0 and s.pin == 0 and safety)
  run publishedStaleDoesNotReactivateTest = init.then(reserve).then(stopAfterPublish)
    .then(integrateSibling).then(dispatch).then(reactivate).fail()
  run restartAfterRepinTest = init.then(reserve).then(integrateSibling).then(stopAfterRepin)
    .then(integrateSibling).then(dispatch)
    .expect(s.launches == 1 and s.base == 2 and safety)
  run restartAfterPublishTest = init.then(reserve).then(stopAfterPublish).then(dispatch)
    .expect(s.launches == 1 and s.base == 0 and safety)
}
