// pstack create/maintain-verification-skill -> Cruise PR implementation evidence
// (owner-approved model; implemented by src/feature_evidence.rs and the evidence steps
// in builtin/tdd.yaml). A project-local verify skill (Launch/Doctor/Drive/Evidence/
// Cleanup + feature map with covered paths) drives each feature the diff touches.
// The host validates the captured files and publishes them in the PR/MR body.
module pr_verification_evidence {
  type Diagram = { state: str, via: str }
  type State = {
    on: bool, skill: bool, touched: Set[str], results: str -> str,
    head: int, evidenceHead: int, artifacts: Set[str], kept: bool, scanned: bool,
    imageDest: bool, prOpen: bool, body: Set[str], bodyHead: int, bodyImages: bool,
    stale: bool, merged: bool, bodyValidated: bool, bodyResults: str -> str, mapFixes: int
  }
  var s: State
  var diagram: Diagram

  pure val features = Set("cli-run", "webui-session")
  pure val outcomes = Set("proved", "failed", "blocked", "map drift")

  action go(state: str, via: str, after: State): bool = all {
    diagram' = { state: state, via: via }, s' = after,
  }
  action start(on: bool, skill: bool, touched: Set[str], imageDest: bool): bool = all {
    s' = { on: on, skill: skill, touched: touched, results: features.mapBy(_ => "none"),
      head: 1, evidenceHead: 0, artifacts: Set(), kept: false, scanned: false,
      imageDest: imageDest, prOpen: false, body: Set(), bodyHead: 0, bodyImages: false,
      stale: false, merged: false, bodyValidated: false,
      bodyResults: features.mapBy(_ => "none"), mapFixes: 0 },
    diagram' = { state: "Implementation verified", via: "start" },
  }
  action init = start(true, true, features, true)

  action noSkill = all {
    diagram.state == "Implementation verified", s.on, not(s.skill),
    go("Stopped: no verification skill", "evidence required; nothing to drive", s),
  }
  action doctor(ok: bool): bool = all {
    diagram.state == "Implementation verified", s.on, s.skill,
    go(if (ok) "Instance healthy" else "Stopped: evidence blocked",
      if (ok) "launch isolated instance; doctor" else "doctor failed", s),
  }
  // Agent drives each touched feature through the skill and captures action + end state.
  action drive(results: str -> str): bool = all {
    diagram.state == "Instance healthy", results.keys() == s.touched,
    results.keys().forall(f => outcomes.contains(results.get(f))),
    go("Features driven", "drive touched features; capture evidence",
      { ...s, evidenceHead: s.head,
        results: features.mapBy(f => if (s.touched.contains(f)) results.get(f) else "none"),
        artifacts: s.touched.filter(f => results.get(f) == "proved") }),
  }
  // The app works but the map's recipe is wrong: edit only the skill directory
  // (host path check), then drive again once. A product regression is "failed".
  action fixMap = all {
    diagram.state == "Features driven", s.mapFixes < 1,
    s.touched.exists(f => s.results.get(f) == "map drift"),
    go("Instance healthy", "update feature map only; drive again", { ...s, mapFixes: s.mapFixes + 1 }),
  }
  action cleanup(keepsEvidence: bool): bool = all {
    diagram.state == "Features driven",
    not(s.touched.exists(f => s.results.get(f) == "map drift") and s.mapFixes < 1),
    go(if (keepsEvidence) "Cleaned up" else "Stopped: cleanup removed evidence",
      "stop owned instances only", { ...s, kept: keepsEvidence }),
  }
  // Host: artifacts exist, digests match, workspace fingerprint unchanged, secret scan.
  action validate(scanOk: bool): bool = all {
    diagram.state == "Cleaned up", s.evidenceHead == s.head,
    val failed = s.touched.exists(f => s.results.get(f) == "failed")
    val blocked = s.touched.exists(f => Set("blocked", "map drift").contains(s.results.get(f)))
    go(if (not(scanOk)) "Stopped: secret scan failed"
       else if (failed) "Stopped: feature regression"
       else if (blocked) "Stopped: evidence blocked" else "Evidence validated",
      "host validates artifacts", { ...s, scanned: scanOk }),
  }
  action publish = all {
    if (s.on) diagram.state == "Evidence validated" else diagram.state == "Implementation verified",
    go("PR open", if (s.on) "body gets evidence for this head" else "existing PR body",
      { ...s, prOpen: true, body: if (s.on) s.artifacts else Set(),
        bodyHead: if (s.on) s.evidenceHead else 0,
        bodyImages: s.on and s.imageDest and s.artifacts != Set(),
        bodyValidated: s.kept and s.scanned and s.skill, bodyResults: s.results }),
  }
  // A later CI fix or sync commit moves the head; the body is marked stale.
  action newHead = all {
    diagram.state == "PR open",
    go("PR open", "new head; evidence marked stale",
      { ...s, head: s.head + 1, stale: s.on }),
  }
  action refresh = all {
    diagram.state == "PR open", s.stale,
    go("Instance healthy", "rerun evidence for new head", { ...s, kept: false, scanned: false }),
  }
  action republish = all {
    diagram.state == "Evidence validated", s.prOpen,
    go("PR open", "replace evidence section",
      { ...s, body: s.artifacts, bodyHead: s.evidenceHead, stale: false,
        bodyImages: s.imageDest and s.artifacts != Set(),
        bodyValidated: s.kept and s.scanned and s.skill, bodyResults: s.results }),
  }
  action merge = all {
    diagram.state == "PR open", not(s.stale),
    go("Merged", "owner-confirmed merge", { ...s, merged: true }),
  }

  action step = any {
    noSkill,
    nondet ok = Set(true, false).oneOf()
    doctor(ok),
    nondet r = s.touched.setOfMaps(outcomes).oneOf()
    drive(r),
    nondet k = Set(true, false).oneOf()
    cleanup(k),
    nondet sc = Set(true, false).oneOf()
    validate(sc),
    publish, newHead, refresh, republish, merge, fixMap,
  }

  // The body is a snapshot: it claims only features proved in the run it was built from.
  val invProvedOnly = s.body.forall(f => s.bodyResults.get(f) == "proved" and s.touched.contains(f))
  val invBound = (s.on and s.prOpen) implies (s.bodyHead == s.evidenceHead or s.stale)
  val invStale = (s.on and s.prOpen) implies (s.stale iff s.head != s.bodyHead)
  val invGatesBeforePr = (s.on and s.prOpen) implies s.bodyValidated
  val invImages = s.bodyImages implies s.imageDest
  val invMerge = s.merged implies not(s.stale)
  val safety = invProvedOnly and invBound and invStale and invGatesBeforePr and invImages and invMerge

  action proved = init.then(doctor(true)).then(drive(Map("cli-run" -> "proved", "webui-session" -> "proved")))
    .then(cleanup(true)).then(validate(true))
  run provedEvidencePublishedTest = proved.then(publish)
    .expect(s.body == features and s.bodyHead == 1 and s.bodyImages and safety)
  run textOnlyWithoutImageDestinationTest = start(true, true, features, false).then(doctor(true))
    .then(drive(Map("cli-run" -> "proved", "webui-session" -> "proved"))).then(cleanup(true))
    .then(validate(true)).then(publish).expect(not(s.bodyImages) and s.body == features and safety)
  run regressionStopsBeforePrTest = init.then(doctor(true))
    .then(drive(Map("cli-run" -> "proved", "webui-session" -> "failed"))).then(cleanup(true))
    .then(validate(true)).expect(diagram.state == "Stopped: feature regression").then(publish).fail()
  run blockedStopsBeforePrTest = init.then(doctor(true))
    .then(drive(Map("cli-run" -> "blocked", "webui-session" -> "proved"))).then(cleanup(true))
    .then(validate(true)).expect(diagram.state == "Stopped: evidence blocked").then(publish).fail()
  run cleanupMustKeepEvidenceTest = init.then(doctor(true))
    .then(drive(Map("cli-run" -> "proved", "webui-session" -> "proved"))).then(cleanup(false))
    .then(validate(true)).fail()
  run secretScanStopsTest = init.then(doctor(true))
    .then(drive(Map("cli-run" -> "proved", "webui-session" -> "proved"))).then(cleanup(true))
    .then(validate(false)).then(publish).fail()
  run noSkillStopsTest = start(true, false, features, true).then(noSkill).then(publish).fail()
  run staleBlocksMergeTest = proved.then(publish).then(newHead).expect(s.stale and safety).then(merge).fail()
  run refreshClearsStaleTest = proved.then(publish).then(newHead).then(refresh)
    .then(drive(Map("cli-run" -> "proved", "webui-session" -> "proved"))).then(cleanup(true))
    .then(validate(true)).then(republish).expect(not(s.stale) and s.bodyHead == 2 and safety)
    .then(merge).expect(s.merged)
  run untouchedFeatureNotClaimedTest = start(true, true, Set("cli-run"), true).then(doctor(true))
    .then(drive(Map("cli-run" -> "proved"))).then(cleanup(true)).then(validate(true)).then(publish)
    .expect(s.body == Set("cli-run") and safety)
  run mapDriftFixedOnceTest = init.then(doctor(true))
    .then(drive(Map("cli-run" -> "map drift", "webui-session" -> "proved"))).then(fixMap)
    .then(drive(Map("cli-run" -> "proved", "webui-session" -> "proved"))).then(cleanup(true))
    .then(validate(true)).then(publish).expect(s.body == features and s.mapFixes == 1 and safety)
  run mapDriftTwiceBlocksTest = init.then(doctor(true))
    .then(drive(Map("cli-run" -> "map drift", "webui-session" -> "proved"))).then(fixMap)
    .then(drive(Map("cli-run" -> "map drift", "webui-session" -> "proved"))).then(cleanup(true))
    .then(validate(true)).expect(diagram.state == "Stopped: evidence blocked")
  run offKeepsCurrentPrTest = start(false, false, features, false).then(publish)
    .expect(s.prOpen and s.body == Set() and safety)
}
