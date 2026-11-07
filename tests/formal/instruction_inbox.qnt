// Bounded instruction inbox protocol, abstracting the exclusive ledger lock as
// atomic actions. IDs 0..2 and keys 0..1 are finite; -1 means no dedupe key.
// A lease takes the whole pending snapshot, including for a parallel block.
// A crash leaves the lease recorded; opening the next run releases it.
// This is a protocol model, not a claim about filesystem failures or liveness.
module instruction_inbox {
  type Inbox = { runOpen: bool, accepted: Set[int], pending: Set[int],
    leased: Set[int], delivered: Set[int], expired: Set[int],
    nextId: int, key0: int, key1: int }
  var s: Inbox
  var previous: Inbox
  var event: str
  var requestKey: int

  pure val initial: Inbox = { runOpen: false, accepted: Set(), pending: Set(),
    leased: Set(), delivered: Set(), expired: Set(), nextId: 0, key0: -1, key1: -1 }
  action init = all { s' = initial, previous' = initial, event' = "init", requestKey' = -1 }
  action record(n: Inbox, e: str, k: int): bool = all {
    previous' = s, s' = n, event' = e, requestKey' = k,
  }
  pure def keyId(i: Inbox, k: int): int =
    if (k == 0) i.key0 else if (k == 1) i.key1 else -1
  action openRun = all {
    not(s.runOpen),
    record({ ...s, runOpen: true, pending: s.pending.union(s.leased), leased: Set() }, "open", -1),
  }
  action accept(k: int): bool = all {
    Set(-1, 0, 1).contains(k),
    if (keyId(s, k) >= 0) record(s, "duplicate", k)
    else if (not(s.runOpen)) record(s, "rejectClosed", k)
    else all {
      s.nextId < 3,
      record({ ...s, accepted: s.accepted.union(Set(s.nextId)),
        pending: s.pending.union(Set(s.nextId)), nextId: s.nextId + 1,
        key0: if (k == 0) s.nextId else s.key0,
        key1: if (k == 1) s.nextId else s.key1 }, "accept", k),
    },
  }
  action lease = all {
    s.runOpen, s.leased == Set(), s.pending != Set(),
    record({ ...s, leased: s.pending, pending: Set() }, "lease", -1),
  }
  action ack = all {
    s.runOpen, s.leased != Set(),
    record({ ...s, delivered: s.delivered.union(s.leased), leased: Set() }, "ack", -1),
  }
  action release = all {
    s.runOpen, s.leased != Set(),
    record({ ...s, pending: s.pending.union(s.leased), leased: Set() }, "release", -1),
  }
  // Closing releases an outstanding lease first. Completed expires all remaining
  // work. Failed and suspended preserve it for a later open and lease.
  action closeRun(completed: bool): bool = all {
    s.runOpen,
    val remaining = s.pending.union(s.leased)
    record({ ...s, runOpen: false, leased: Set(),
      pending: if (completed) Set() else remaining,
      expired: if (completed) s.expired.union(remaining) else s.expired },
      if (completed) "completed" else "failedOrSuspended", -1),
  }
  action expire = all {
    not(s.runOpen),
    record({ ...s, expired: s.expired.union(s.pending).union(s.leased),
      pending: Set(), leased: Set() }, "expire", -1),
  }
  action crash = all { s.runOpen, record({ ...s, runOpen: false }, "crash", -1) }
  action idle = record(s, "idle", -1)
  action step = any { openRun, accept(-1), accept(0), accept(1), lease, ack,
    release, closeRun(true), closeRun(false), expire, crash, idle }

  // F-1: all accepted IDs have exactly one disposition.
  val f1_partition = all {
    s.accepted == s.pending.union(s.leased).union(s.delivered).union(s.expired),
    s.pending.intersect(s.leased.union(s.delivered).union(s.expired)) == Set(),
    s.leased.intersect(s.delivered.union(s.expired)) == Set(),
    s.delivered.intersect(s.expired) == Set(),
    s.accepted == Set(0, 1, 2).filter(i => i < s.nextId),
  }
  // F-2: one outstanding batch. New arrivals remain pending until it resolves.
  val f2_one_lease = all {
    event == "lease" implies previous.leased == Set() and s.leased == previous.pending,
    previous.leased != Set() and event == "accept" implies s.leased == previous.leased,
  }
  val f3_no_closed_accept = all {
    event == "accept" implies previous.runOpen,
    event == "rejectClosed" implies not(previous.runOpen) and s == previous,
  }
  val f4_terminal = all {
    previous.delivered.subseteq(s.delivered), previous.expired.subseteq(s.expired),
    s.pending.intersect(previous.delivered.union(previous.expired)) == Set(),
  }
  val f5_dedupe = all {
    event == "duplicate" implies keyId(previous, requestKey) >= 0
      and s == previous,
    event == "accept" implies keyId(previous, requestKey) == -1
      and s.nextId == previous.nextId + 1,
  }
  val f6_close = all {
    event == "completed" implies s.pending == Set() and s.leased == Set(),
    event == "failedOrSuspended" implies
      s.pending == previous.pending.union(previous.leased) and s.leased == Set(),
    event == "open" implies s.pending == previous.pending.union(previous.leased),
  }
  val safety = f1_partition and f2_one_lease and f3_no_closed_accept
    and f4_terminal and f5_dedupe and f6_close

  run completedTest = init.then(openRun).then(accept(0)).then(lease)
    .then(accept(1)).then(closeRun(true))
    .expect(s.expired == Set(0, 1) and s.pending == Set() and safety)
  run carryTest = init.then(openRun).then(accept(0)).then(lease)
    .then(closeRun(false)).then(openRun).then(lease).then(ack)
    .expect(s.delivered == Set(0) and safety)
  run crashRecoveryTest = init.then(openRun).then(accept(0)).then(lease)
    .then(crash).then(accept(1)).then(openRun).then(lease).then(release).then(lease).then(ack)
    .expect(s.delivered == Set(0) and s.nextId == 1 and safety)
  run dedupeTest = init.then(openRun).then(accept(0)).then(accept(0))
    .then(lease).then(ack).then(closeRun(true)).then(accept(0))
    .expect(s.nextId == 1 and s.delivered == Set(0) and safety)
  run expireTest = init.then(openRun).then(accept(-1)).then(closeRun(false))
    .then(expire).then(openRun).expect(s.expired == Set(0) and s.pending == Set() and safety)
}
