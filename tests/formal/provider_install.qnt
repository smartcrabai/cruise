// Abstract contract for the planned provider installer, not a production refinement.
// One provider, one installer and one prior publication. Resolution is read-only.
// Verification booleans for a published binary are separate from the candidate,
// so a failed replacement cannot invalidate the previous publication.
module provider_install {
  type State = { envSet: bool, envValid: bool, record: str, pathHas: bool,
    published: bool, digestOk: bool, probeOk: bool,
    locked: bool, stage: int, candidateDigest: bool, candidateProbe: bool }
  // stage: 0 idle, 1 staging, 2 verified. Record: none, ready, stale, damaged.
  var s: State
  var previous: State
  var event: str
  pure val initial: State = { envSet: false, envValid: false, record: "none",
    pathHas: false, published: false, digestOk: false, probeOk: false,
    locked: false, stage: 0, candidateDigest: false, candidateProbe: false }
  action init = all { s' = initial, previous' = initial, event' = "init" }
  action recordState(updated: State, name: str): bool = all {
    s' = updated, previous' = s, event' = name
  }
  val result = if (s.envSet) (if (s.envValid) "override" else "error")
    else if (s.record == "ready") "managed"
    else if (s.pathHas) "path" else "error"
  action setEnvironment(isSet: bool, valid: bool, path: bool): bool =
    recordState({ ...s, envSet: isSet, envValid: valid, pathHas: path }, "environment")
  action begin = all { not(s.locked),
    recordState({ ...s, locked: true, stage: 1,
      candidateDigest: false, candidateProbe: false }, "begin") }
  action verifyDigest = all { s.locked, s.stage == 1,
    recordState({ ...s, candidateDigest: true }, "digest") }
  action verifyProbe = all { s.locked, s.stage == 1, s.candidateDigest,
    recordState({ ...s, candidateProbe: true, stage: 2 }, "probe") }
  // Abstracts final rename and atomic record switch under the lock.
  action publish = all { s.locked, s.stage == 2, s.candidateDigest, s.candidateProbe,
    recordState({ ...s, published: true, digestOk: true, probeOk: true,
      record: "ready", stage: 0, locked: false }, "publish") }
  action installFailure = all { s.locked,
    recordState({ ...s, locked: false, stage: 0,
      candidateDigest: false, candidateProbe: false }, "fail") }
  // External pin change and damage are modeled only when no install is active.
  action stale = all { not(s.locked), s.record == "ready",
    recordState({ ...s, record: "stale" }, "stale") }
  action damage = all { not(s.locked), s.record == "ready",
    recordState({ ...s, record: "damaged" }, "damage") }
  action step = any { setEnvironment(false, false, false),
    setEnvironment(false, false, true), setEnvironment(true, true, false),
    setEnvironment(true, true, true), setEnvironment(true, false, false),
    setEnvironment(true, false, true), begin, verifyDigest, verifyProbe,
    publish, installFailure, stale, damage }

  val overrideValid = s.envSet and s.envValid implies result == "override"
  val overrideInvalid = s.envSet and not(s.envValid) implies result == "error"
  val managedReady = result == "managed" implies not(s.envSet) and s.record == "ready"
  val staleNotManaged = (s.record == "stale" or s.record == "damaged") implies result != "managed"
  val fallbackCorrect = not(s.envSet) implies
    (if (s.record == "ready") result == "managed"
     else if (s.pathHas) result == "path" else result == "error")
  val publicationVerified = s.published implies s.digestOk and s.probeOk
  val readyPublished = s.record == "ready" implies s.published
  val lifecycle = all {
    event == "fail" implies all { s.record == previous.record,
      s.published == previous.published, s.digestOk == previous.digestOk,
      s.probeOk == previous.probeOk },
    event == "publish" implies previous.locked and previous.candidateDigest and previous.candidateProbe,
    s.stage != 0 implies s.locked,
  }
  val safety = overrideValid and overrideInvalid and managedReady and staleNotManaged
    and fallbackCorrect and publicationVerified and readyPublished and lifecycle

  run missingTest = init.expect(result == "error" and safety)
  run pathTest = init.then(setEnvironment(false, false, true)).expect(result == "path" and safety)
  run installTest = init.then(begin).then(verifyDigest).then(verifyProbe).then(publish)
    .expect(result == "managed" and safety)
  run overrideTest = init.then(begin).then(verifyDigest).then(verifyProbe).then(publish)
    .then(setEnvironment(true, true, true)).expect(result == "override" and safety)
  run badOverrideTest = init.then(begin).then(verifyDigest).then(verifyProbe).then(publish)
    .then(setEnvironment(true, false, true)).expect(result == "error" and safety)
  run staleReinstallTest = init.then(begin).then(verifyDigest).then(verifyProbe).then(publish)
    .then(stale).then(setEnvironment(false, false, true)).expect(result == "path" and safety)
    .then(begin).then(verifyDigest).then(verifyProbe).then(publish).expect(result == "managed" and safety)
  run damagedTest = init.then(begin).then(verifyDigest).then(verifyProbe).then(publish)
    .then(damage).expect(result == "error" and safety)
  run failedReplacementTest = init.then(begin).then(verifyDigest).then(verifyProbe).then(publish)
    .then(begin).then(verifyDigest).then(installFailure).expect(result == "managed" and s.published and safety)
  run failedInitialTest = init.then(begin).then(installFailure).expect(result == "error" and not(s.published) and safety)
}
