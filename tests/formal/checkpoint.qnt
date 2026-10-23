// Paired with checkpoint.als: identical initial state, actions and bounds.
// Reconstructed contract IDs from tests/graph_state.rs, not recovered audit labels.
// Abstraction: work -> work (retry/group), work -> finish (normal/skip), finish -> done.
// One retry pair, one forward pair, one group site. Limits/counts 0..2, effects 0..4.
// runtime is an opaque output/snapshot token. Successful checkpoint writes are atomic.
// Resume is from a successful checkpoint only. No exactly-once claim for crashes
// between an external side effect and its checkpoint, no fairness/liveness claim.
// Invalid input classes abstract validation, not JSON decoding (covered by BDD).
// Only automatic/node-ID resume is modeled. Manual reposition and usize overflow
// have concrete BDD coverage, not an unbounded claim from this scalar model.
// Bounded verification: 12 transitions. Witness tests additionally prevent vacuity.
// gate authorizes one effect after a successful save; saved marks resumable state.
// previous/event are observer instrumentation, not extra production state.
module checkpoint {
  type State = { current: int, pending: int, total: int, budget: int,
    forward: int, forwardBudget: int, groups: int, effects: int,
    runtime: int, limit: int, gate: bool, saved: bool }
  var s: State
  var previous: State
  var event: str

  pure val initial: State = { current: 0, pending: 0, total: 0, budget: 0,
    forward: 0, forwardBudget: 0, groups: 0, effects: 0,
    runtime: 0, limit: 2, gate: false, saved: true }
  action init = all { s' = initial, previous' = initial, event' = "init" }
  action record(updated: State, name: str): bool = all {
    previous' = s, s' = updated, event' = name,
  }
  // engine::save_checkpoint before a side effect. A failed write cannot authorize it.
  action authorize = all {
    s.current < 2,
    record({ ...s, gate: true, saved: true }, "save"),
  }
  action checkpointFailure = record({ ...s, gate: false }, "checkpointFailure")
  // pending: 0 none, 1 retry, 2 group retry, 3 skip/exhaustion, 4 forward, 5 exit.
  action execute(kind: int): bool = all {
    s.current < 2, s.pending == 0, s.gate,
    if (s.current == 0) 1.to(4).contains(kind) else kind == 5,
    // step_loop_iteration skips an exhausted invocation before executing its body.
    s.current != 0 or s.groups < 2 or kind == 3,
    kind == 3 or s.effects < 4,
    val n = if (kind == 3) s.effects else s.effects + 1
    record({ ...s, pending: kind, effects: n, runtime: n,
      gate: false, saved: false }, "execute"),
  }
  // engine::accept_transition plus its successful post-transition checkpoint.
  // Failure of that checkpoint halts before another effect, represented by gate.
  action accept = all {
    s.pending > 0,
    if (s.pending <= 2) all {
      s.budget < s.limit, s.total < 2,
      record({ ...s, pending: 0, total: s.total + 1,
        budget: s.budget + 1,
        groups: s.groups + (if (s.pending == 2) 1 else 0),
        gate: false, saved: true }, "accept"),
    } else if (s.pending <= 4) all {
      s.pending == 3 or s.forwardBudget < s.limit,
      record({ ...s, current: 1, pending: 0, forward: s.forward + 1,
        forwardBudget: s.forwardBudget + (if (s.pending == 4) 1 else 0),
        gate: false, saved: true }, "accept"),
    } else record({ ...s, current: 2, pending: 0,
      gate: false, saved: true }, "accept"),
  }
  action rejectBudget = all {
    (s.pending <= 2 and s.pending > 0 and s.budget >= s.limit)
      or (s.pending == 4 and s.forwardBudget >= s.limit),
    record({ ...s, saved: true, gate: false }, "rejectBudget"),
  }
  // prepare_resume/restore: stale session position never overrides node-id authority.
  action resume = all { s.saved, record(s, "resume") }
  action setLimit(n: int): bool = all {
    0.to(2).contains(n), record({ ...s, limit: n }, "policy"),
  }
  // prepare_resume/load_graph reject corrupt, legacy, incomplete, identity/topology
  // mismatch without modifying the checkpoint. Validation details live in BDD.
  action rejectInvalid = record(s, "rejectInvalid")
  action idle = record(s, "idle")
  action step = any { authorize, checkpointFailure,
    execute(1), execute(2), execute(3), execute(4), execute(5), accept,
    rejectBudget, resume, setLimit(0), setLimit(1), setLimit(2), rejectInvalid, idle }

  // CP-01: engine::accept_transition counts accepted pairs, not denied requests.
  // A lowered policy may be below historical counts; it must not erase them.
  val cp01_accounting = all {
    0.to(2).contains(s.total), 0.to(s.total).contains(s.budget),
    0.to(1).contains(s.forward), 0.to(s.forward).contains(s.forwardBudget),
    s.total >= previous.total, s.budget >= previous.budget,
    event == "accept" and previous.pending <= 2 implies
      previous.budget < previous.limit and s.budget == previous.budget + 1
        and s.total == previous.total + 1
        and s.forward == previous.forward and s.forwardBudget == previous.forwardBudget,
    event == "accept" and previous.pending >= 3 and previous.pending <= 4 implies
      s.forward == previous.forward + 1
        and s.forwardBudget == previous.forwardBudget + (if (previous.pending == 4) 1 else 0)
        and s.total == previous.total and s.budget == previous.budget,
    event != "accept" or previous.pending == 5 implies
      s.total == previous.total and s.budget == previous.budget
        and s.forward == previous.forward and s.forwardBudget == previous.forwardBudget,
    event == "rejectBudget" implies
      s.total == previous.total and s.budget == previous.budget
        and s.forward == previous.forward and s.forwardBudget == previous.forwardBudget,
  }
  // CP-02: a pending outcome is accepted without rerunning its source command.
  val cp02_pending = all {
    s.pending > 0 implies s.current < 2,
    event == "execute" implies previous.pending == 0,
    event == "accept" or event == "rejectBudget" implies s.effects == previous.effects,
    event == "rejectBudget" implies s.pending == previous.pending,
  }
  // CP-03: a successful pre-effect checkpoint is required, failed writes gate effects.
  val cp03_checkpoint_gate = all {
    event == "execute" implies previous.gate,
    event == "checkpointFailure" implies not(s.gate) and s.effects == previous.effects,
  }
  // CP-04: skip/exhaustion records a traversal, but neither budget nor command effect.
  val cp04_skip = all {
    event == "execute" and s.pending == 3 implies s.effects == previous.effects,
    event == "accept" and previous.pending == 3 implies
      s.forward == previous.forward + 1 and s.forwardBudget == previous.forwardBudget,
  }
  // CP-05: checkpoint authority includes position, counters and opaque runtime.
  val cp05_resume = event == "resume" implies s == previous
  // CP-06: rejected incompatible input preserves checkpoint bytes/state.
  val cp06_reject = event == "rejectInvalid" implies s == previous
  // CP-07: group retries are accepted atomically with pair counts, runtime survives resume.
  val cp07_group = all {
    0.to(s.budget).contains(s.groups),
    event == "accept" and previous.pending == 2 implies s.groups == previous.groups + 1,
    not(event == "accept" and previous.pending == 2) implies s.groups == previous.groups,
    event == "execute" and previous.current == 0 and previous.groups == 2 implies
      s.pending == 3 and s.effects == previous.effects,
    event != "execute" implies s.runtime == previous.runtime,
  }
  // CP-08: two fixed node identities, terminal completion has no pending work/effects.
  val cp08_completed = all {
    0.to(2).contains(s.current), 0.to(5).contains(s.pending),
    0.to(4).contains(s.effects), s.runtime == s.effects,
    s.current == 2 implies s.pending == 0,
    previous.current == 2 implies s.current == 2 and s.effects == previous.effects,
  }
  val safety = cp01_accounting and cp02_pending and cp03_checkpoint_gate
    and cp04_skip and cp05_resume and cp06_reject and cp07_group and cp08_completed

  run retryResumeTest = init.then(authorize).then(execute(1)).then(accept)
    .then(setLimit(0)).then(authorize).then(execute(1)).then(rejectBudget)
    .then(resume).then(setLimit(2)).then(accept)
    .expect(s.total == 2 and s.effects == 2 and s.pending == 0 and safety)
  run skipCompletionTest = init.then(checkpointFailure).then(authorize)
    .then(execute(3)).then(accept).then(authorize).then(execute(5)).then(accept)
    .then(resume).expect(s.current == 2 and s.effects == 1
      and s.forward == 1 and s.forwardBudget == 0 and safety)
  run groupExhaustionTest = init.then(authorize).then(execute(2)).then(accept)
    .then(authorize).then(execute(2)).then(accept)
    .then(authorize).then(execute(3)).then(accept)
    .then(authorize).then(execute(5)).then(accept)
    .expect(s.groups == 2 and s.current == 2 and s.effects == 3
      and s.forward == 1 and s.forwardBudget == 0 and safety)
  run groupRuntimeTest = init.then(authorize).then(execute(2)).then(accept)
    .then(resume).then(rejectInvalid)
    .expect(s.groups == 1 and s.runtime == 1 and safety)
}
