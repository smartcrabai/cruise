// Abstract contract for the provider installer, not a production refinement.
// Latest metadata is snapshotted only after taking the provider install lock.
// A failed replacement preserves the previously published version and binary.
module provider_install {
  // Version names are abstract identities; semver syntax is outside this model.
  type State = { envSet: bool, envValid: bool, record: str, recordVersion: str,
    latestVersion: str, pathHas: bool, published: bool, digestOk: bool,
    probeOk: bool, locked: bool, stage: int, snapshot: bool,
    candidateVersion: str, candidateDigest: bool, candidateProbe: bool }
  // stage: 0 idle, 1 staging, 2 verified. Record: none, ready, damaged.
  var s: State
  var previous: State
  var event: str
  pure val initial: State = { envSet: false, envValid: false, record: "none",
    recordVersion: "none", latestVersion: "v1", pathHas: false,
    published: false, digestOk: false, probeOk: false, locked: false,
    stage: 0, snapshot: false, candidateVersion: "none",
    candidateDigest: false, candidateProbe: false }
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
    recordState({ ...s, locked: true, stage: 1, snapshot: false,
      candidateVersion: "none", candidateDigest: false, candidateProbe: false }, "begin") }
  action resolveLatest = all { s.locked, s.stage == 1, not(s.snapshot),
    recordState({ ...s, snapshot: true, candidateVersion: s.latestVersion }, "snapshot") }
  action alreadyCurrent = all { s.locked, s.stage == 1, s.snapshot,
    s.record == "ready", s.recordVersion == s.candidateVersion,
    recordState({ ...s, locked: false, stage: 0, snapshot: false,
      candidateVersion: "none", candidateDigest: false, candidateProbe: false }, "same") }
  action verifyDigest = all { s.locked, s.stage == 1, s.snapshot,
    recordState({ ...s, candidateDigest: true }, "digest") }
  action verifyProbe = all { s.locked, s.stage == 1, s.snapshot, s.candidateDigest,
    recordState({ ...s, candidateProbe: true, stage: 2 }, "probe") }
  // Abstracts final rename and atomic record switch under the lock.
  action publish = all { s.locked, s.stage == 2, s.snapshot,
    s.candidateDigest, s.candidateProbe,
    recordState({ ...s, published: true, digestOk: true, probeOk: true,
      record: "ready", recordVersion: s.candidateVersion, stage: 0,
      locked: false, snapshot: false }, "publish") }
  action installFailure = all { s.locked,
    recordState({ ...s, locked: false, stage: 0, snapshot: false,
      candidateVersion: "none", candidateDigest: false, candidateProbe: false }, "fail") }
  action metadataFailure = all { s.locked, s.stage == 1, not(s.snapshot),
    recordState({ ...s, locked: false, stage: 0, candidateVersion: "none",
      candidateDigest: false, candidateProbe: false }, "metadata-fail") }
  action advanceLatest = all { s.latestVersion == "v1",
    recordState({ ...s, latestVersion: "v2" }, "latest") }
  action damage = all { not(s.locked), s.record == "ready",
    recordState({ ...s, record: "damaged" }, "damage") }
  action step = any { setEnvironment(false, false, false),
    setEnvironment(false, false, true), setEnvironment(true, true, false),
    setEnvironment(true, true, true), setEnvironment(true, false, false),
    setEnvironment(true, false, true), begin, resolveLatest, alreadyCurrent,
    verifyDigest, verifyProbe, publish, installFailure, metadataFailure, advanceLatest, damage }

  val overrideValid = s.envSet and s.envValid implies result == "override"
  val overrideInvalid = s.envSet and not(s.envValid) implies result == "error"
  val managedReady = result == "managed" implies not(s.envSet) and s.record == "ready"
  val damagedNotManaged = s.record == "damaged" implies result != "managed"
  val olderVersionRemainsManaged = not(s.envSet) and s.record == "ready"
    and s.recordVersion != s.latestVersion implies result == "managed"
  val fallbackCorrect = not(s.envSet) implies
    (if (s.record == "ready") result == "managed"
     else if (s.pathHas) result == "path" else result == "error")
  val publicationVerified = s.published implies s.digestOk and s.probeOk
  val readyPublished = s.record == "ready" implies s.published
  val lifecycle = all {
    (event == "fail" or event == "metadata-fail") implies all {
      s.record == previous.record, s.recordVersion == previous.recordVersion,
      s.published == previous.published, s.digestOk == previous.digestOk,
      s.probeOk == previous.probeOk },
    event == "metadata-fail" implies all {
      previous.locked, previous.stage == 1, not(previous.snapshot) },
    event == "snapshot" implies all { previous.locked,
      s.candidateVersion == previous.latestVersion },
    event == "publish" implies all { previous.locked and previous.snapshot,
      previous.candidateDigest and previous.candidateProbe,
      s.recordVersion == previous.candidateVersion },
    event == "same" implies all { previous.locked and previous.snapshot,
      previous.recordVersion == previous.candidateVersion,
      s.recordVersion == previous.recordVersion, s.published == previous.published },
    event == "latest" implies all { s.record == previous.record,
      s.recordVersion == previous.recordVersion, s.published == previous.published },
    s.snapshot implies s.locked and (s.stage == 1 or s.stage == 2),
    s.stage != 0 implies s.locked
  }
  val safety = overrideValid and overrideInvalid and managedReady
    and damagedNotManaged and olderVersionRemainsManaged and fallbackCorrect
    and publicationVerified and readyPublished and lifecycle

  run missingTest = init.expect(result == "error" and safety)
  run pathTest = init.then(setEnvironment(false, false, true)).expect(result == "path" and safety)
  run installTest = init.then(begin).then(resolveLatest).then(verifyDigest)
    .then(verifyProbe).then(publish).expect(result == "managed" and safety)
  run overrideTest = init.then(begin).then(resolveLatest).then(verifyDigest)
    .then(verifyProbe).then(publish).then(setEnvironment(true, true, true))
    .expect(result == "override" and safety)
  run badOverrideTest = init.then(begin).then(resolveLatest).then(verifyDigest)
    .then(verifyProbe).then(publish).then(setEnvironment(true, false, true))
    .expect(result == "error" and safety)
  run latestUpdateTest = init.then(begin).then(resolveLatest).then(verifyDigest)
    .then(verifyProbe).then(publish).then(advanceLatest)
    .expect(result == "managed" and s.recordVersion == "v1" and safety)
    .then(begin).then(resolveLatest).then(verifyDigest).then(verifyProbe).then(publish)
    .expect(result == "managed" and s.recordVersion == "v2" and safety)
  run lockedResolutionTest = init.then(begin).then(advanceLatest).then(resolveLatest)
    .then(verifyDigest).then(verifyProbe).then(publish)
    .expect(result == "managed" and s.recordVersion == "v2" and safety)
  run metadataFailureTest = init.then(begin).then(resolveLatest).then(verifyDigest)
    .then(verifyProbe).then(publish).then(advanceLatest).then(begin).then(metadataFailure)
    .expect(result == "managed" and s.recordVersion == "v1"
      and s.latestVersion == "v2" and s.published and safety)
  run sameVersionTest = init.then(begin).then(resolveLatest).then(verifyDigest)
    .then(verifyProbe).then(publish).then(begin).then(resolveLatest).then(alreadyCurrent)
    .expect(result == "managed" and s.recordVersion == "v1" and safety)
  run damagedTest = init.then(begin).then(resolveLatest).then(verifyDigest)
    .then(verifyProbe).then(publish).then(damage).expect(result == "error" and safety)
  run failedReplacementTest = init.then(begin).then(resolveLatest).then(verifyDigest)
    .then(verifyProbe).then(publish).then(begin).then(installFailure)
    .expect(result == "managed" and s.recordVersion == "v1" and s.published and safety)
  run failedInitialTest = init.then(begin).then(installFailure)
    .expect(result == "error" and not(s.published) and safety)
}
