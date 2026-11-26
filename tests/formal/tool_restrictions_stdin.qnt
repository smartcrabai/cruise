// A single hook child and the test harness's one write, then wait.
// Only the ordering of child exit and parent write matters for BrokenPipe.
// A write attempt drops the parent's writer even on error (EOF if still open).
module tool_restrictions_stdin {
  type State = { repaired: bool, earlyExitAllowed: bool, gate: str,
    write: str, observed: bool, verdict: str }
  var s: State

  action start(repaired: bool, earlyExitAllowed: bool): bool =
    s' = { repaired: repaired, earlyExitAllowed: earlyExitAllowed, gate: "running",
      write: "pending", observed: false, verdict: "pending" }

  action init = {
    nondet repaired = Set(false, true).oneOf()
    nondet earlyExitAllowed = Set(false, true).oneOf()
    start(repaired, earlyExitAllowed)
  }

  // The gate result summarizes the scenario's exit-code, stdout and stderr checks.
  action exitGate(matchesExpected: bool): bool = all {
    s.gate == "running",
    s' = { ...s, gate: if (matchesExpected) "expected" else "unexpected" },
  }

  action writePayload = all {
    s.write == "pending",
    s' = { ...s, write: if (s.gate == "running") "sent" else "brokenPipe" },
  }

  action writeIoError = all {
    s.write == "pending",
    s' = { ...s, write: "otherError" },
  }

  action abortBeforeWait = all {
    not(s.repaired), s.verdict == "pending",
    Set("brokenPipe", "otherError").contains(s.write),
    s' = { ...s, verdict: "fail" },
  }

  action observe = all {
    s.gate != "running", s.write != "pending", s.verdict == "pending",
    s.repaired or s.write == "sent",
    s' = { ...s, observed: true,
      verdict: if (s.gate == "expected" and
        (s.write == "sent" or (s.earlyExitAllowed and s.write == "brokenPipe")))
        "pass" else "fail" },
  }

  action step = any {
    exitGate(true), exitGate(false), writePayload, writeIoError, abortBeforeWait, observe,
    all { s.verdict != "pending", s' = s },
  }

  val safety = all {
    s.verdict == "pass" implies s.observed and s.gate == "expected",
    s.repaired and s.verdict != "pending" implies s.observed,
    s.verdict == "pass" and s.write != "sent" implies
      s.repaired and s.earlyExitAllowed and s.write == "brokenPipe",
    s.repaired and s.observed and s.gate == "expected" and s.earlyExitAllowed and
      Set("sent", "brokenPipe").contains(s.write) implies s.verdict == "pass",
  }

  run beforeWriteFirstTest = start(false, true).then(writePayload).then(exitGate(true))
    .then(observe).expect(safety and s.verdict == "pass" and s.observed)
  run beforeExitFirstTest = start(false, true).then(exitGate(true)).then(writePayload)
    .then(abortBeforeWait).expect(safety and s.write == "brokenPipe" and s.verdict == "fail" and not(s.observed))
  run afterWriteFirstTest = start(true, true).then(writePayload).then(exitGate(true))
    .then(observe).expect(safety and s.verdict == "pass" and s.observed)
  run afterExitFirstTest = start(true, true).then(exitGate(true)).then(writePayload)
    .then(observe).expect(safety and s.write == "brokenPipe" and s.verdict == "pass" and s.observed)
  run badGateResultTest = start(true, true).then(exitGate(false)).then(writePayload)
    .then(observe).expect(safety and s.write == "brokenPipe" and s.verdict == "fail" and s.observed)
  run inputRequiredEarlyCloseTest = start(true, false).then(exitGate(true)).then(writePayload)
    .then(observe).expect(safety and s.write == "brokenPipe" and s.verdict == "fail" and s.observed)
  run inputRequiredWriteFirstTest = start(true, false).then(writePayload).then(exitGate(true))
    .then(observe).expect(safety and s.verdict == "pass" and s.observed)
  run otherIoErrorTest = start(true, true).then(exitGate(true)).then(writeIoError)
    .then(observe).expect(safety and s.verdict == "fail" and s.observed)
}
