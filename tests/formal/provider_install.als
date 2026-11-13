// Independent bounded abstraction of one provider and one installation actor.
module provider_install
open util/ordering[State]
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Record {}
one sig Missing, Ready, Damaged extends Record {}
abstract sig Stage {}
one sig Idle, Staging, Verified extends Stage {}
// V1 and V2 are abstract valid versions; semver syntax is outside this model.
abstract sig Version {}
one sig V1, V2 extends Version {}
abstract sig Result {}
one sig Override, Managed, Path, Error extends Result {}
sig State {
  envSet, envValid, pathHas: one Bool,
  record: one Record, published, digestOk, probeOk: one Bool,
  recordVersion, latestVersion, candidateVersion: one Version,
  locked: one Bool, stage: one Stage, snapshot, candidateDigest, candidateProbe: one Bool
}
fun result[s: State]: one Result {
  s.envSet = Yes => (s.envValid = Yes => Override else Error)
  else (s.record = Ready => Managed else (s.pathHas = Yes => Path else Error))
}
pred initial[s: State] {
  s.envSet = No and s.envValid = No and s.pathHas = No
  s.record = Missing and s.published = No and s.digestOk = No and s.probeOk = No
  s.recordVersion = V1 and s.latestVersion = V1 and s.candidateVersion = V1
  s.locked = No and s.stage = Idle and s.snapshot = No
  s.candidateDigest = No and s.candidateProbe = No
}
pred sameEnv[a,b: State] { b.envSet = a.envSet and b.envValid = a.envValid and b.pathHas = a.pathHas }
pred sameLatest[a,b: State] { b.latestVersion = a.latestVersion }
pred samePublished[a,b: State] {
  b.record = a.record and b.recordVersion = a.recordVersion
  and b.published = a.published and b.digestOk = a.digestOk and b.probeOk = a.probeOk
}
pred sameWork[a,b: State] {
  b.locked = a.locked and b.stage = a.stage and b.snapshot = a.snapshot
  and b.candidateVersion = a.candidateVersion
  and b.candidateDigest = a.candidateDigest and b.candidateProbe = a.candidateProbe
}
pred environment[a,b: State] { samePublished[a,b] and sameWork[a,b] and sameLatest[a,b] }
pred begin[a,b: State] {
  a.locked = No and sameEnv[a,b] and samePublished[a,b] and sameLatest[a,b]
  b.locked = Yes and b.stage = Staging and b.snapshot = No
  b.candidateVersion = a.candidateVersion and b.candidateDigest = No and b.candidateProbe = No
}
pred resolveLatest[a,b: State] {
  a.locked = Yes and a.stage = Staging and a.snapshot = No
  sameEnv[a,b] and samePublished[a,b] and sameLatest[a,b]
  b.locked = Yes and b.stage = Staging and b.snapshot = Yes
  b.candidateVersion = a.latestVersion
  b.candidateDigest = a.candidateDigest and b.candidateProbe = a.candidateProbe
}
pred digest[a,b: State] {
  a.locked = Yes and a.stage = Staging and a.snapshot = Yes
  sameEnv[a,b] and samePublished[a,b] and sameLatest[a,b]
  b.locked = Yes and b.stage = Staging and b.snapshot = Yes
  b.candidateVersion = a.candidateVersion
  b.candidateDigest = Yes and b.candidateProbe = a.candidateProbe
}
pred probe[a,b: State] {
  a.locked = Yes and a.stage = Staging and a.snapshot = Yes and a.candidateDigest = Yes
  sameEnv[a,b] and samePublished[a,b] and sameLatest[a,b]
  b.locked = Yes and b.stage = Verified and b.snapshot = Yes
  b.candidateVersion = a.candidateVersion and b.candidateDigest = Yes and b.candidateProbe = Yes
}
pred publish[a,b: State] {
  a.locked = Yes and a.stage = Verified and a.snapshot = Yes
  and a.candidateDigest = Yes and a.candidateProbe = Yes
  sameEnv[a,b] and sameLatest[a,b]
  b.record = Ready and b.recordVersion = a.candidateVersion
  b.published = Yes and b.digestOk = Yes and b.probeOk = Yes
  b.locked = No and b.stage = Idle and b.snapshot = No
  b.candidateVersion = a.candidateVersion and b.candidateDigest = Yes and b.candidateProbe = Yes
}
pred alreadyCurrent[a,b: State] {
  a.locked = Yes and a.stage = Staging and a.snapshot = Yes
  and a.record = Ready and a.recordVersion = a.candidateVersion
  sameEnv[a,b] and samePublished[a,b] and sameLatest[a,b]
  b.locked = No and b.stage = Idle and b.snapshot = No
  b.candidateVersion = a.candidateVersion and b.candidateDigest = No and b.candidateProbe = No
}
pred failure[a,b: State] {
  a.locked = Yes and sameEnv[a,b] and samePublished[a,b] and sameLatest[a,b]
  b.locked = No and b.stage = Idle and b.snapshot = No
  b.candidateVersion = a.candidateVersion and b.candidateDigest = No and b.candidateProbe = No
}
pred metadataFailure[a,b: State] {
  a.locked = Yes and a.stage = Staging and a.snapshot = No
  sameEnv[a,b] and samePublished[a,b] and sameLatest[a,b]
  b.locked = No and b.stage = Idle and b.snapshot = No
  b.candidateVersion = a.candidateVersion and b.candidateDigest = No and b.candidateProbe = No
}
pred advanceLatest[a,b: State] {
  a.latestVersion = V1 and sameEnv[a,b] and samePublished[a,b]
  and sameWork[a,b]
  b.latestVersion = V2
}
pred damage[a,b: State] {
  a.locked = No and a.record = Ready and sameEnv[a,b] and sameLatest[a,b] and sameWork[a,b]
  b.record = Damaged and b.recordVersion = a.recordVersion
  b.published = a.published and b.digestOk = a.digestOk and b.probeOk = a.probeOk
}
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] |
    environment[a,b] or begin[a,b] or resolveLatest[a,b] or digest[a,b]
    or probe[a,b] or publish[a,b] or alreadyCurrent[a,b] or failure[a,b]
    or metadataFailure[a,b] or advanceLatest[a,b] or damage[a,b]
}
assert Resolution {
  all s: State | {
    s.envSet = Yes and s.envValid = Yes implies result[s] = Override
    s.envSet = Yes and s.envValid = No implies result[s] = Error
    result[s] = Managed implies s.envSet = No and s.record = Ready
    s.record = Damaged implies result[s] != Managed
    s.envSet = No and s.record != Ready and s.pathHas = Yes implies result[s] = Path
  }
}
assert Publication {
  all s: State | {
    s.published = Yes implies s.digestOk = Yes and s.probeOk = Yes
    s.record = Ready implies s.published = Yes
    s.stage != Idle implies s.locked = Yes
    s.snapshot = Yes implies s.locked = Yes
  }
}
assert FailurePreserves {
  all a: State - last | (failure[a,next[a]] or metadataFailure[a,next[a]])
    implies samePublished[a,next[a]]
}
assert PublishRequiresChecks {
  all a: State - last | publish[a,next[a]] implies
    a.locked = Yes and a.snapshot = Yes and a.candidateDigest = Yes and a.candidateProbe = Yes
}
assert MetadataFailureRequiresLock {
  all a: State - last | metadataFailure[a,next[a]] implies
    a.locked = Yes and a.stage = Staging and a.snapshot = No
}
assert LatestSnapshotUnderLock {
  all a: State - last | resolveLatest[a,next[a]] implies
    a.locked = Yes and next[a].candidateVersion = a.latestVersion
}
assert PublishUsesSnapshot {
  all a: State - last | publish[a,next[a]] implies
    a.snapshot = Yes and next[a].recordVersion = a.candidateVersion
}
assert OlderVersionRemainsManaged {
  all s: State | s.record = Ready and s.recordVersion != s.latestVersion
    and s.envSet = No implies result[s] = Managed
}
assert LatestChangePreservesRecord {
  all a: State - last | advanceLatest[a,next[a]] implies samePublished[a,next[a]]
}
pred UpdateWitness {
  some a,b,c: State | a in b.^prev and b in c.^prev
    and a.record = Ready and a.recordVersion = V1
    and b.latestVersion = V2 and b.record = Ready and b.recordVersion = V1
    and c.record = Ready and c.recordVersion = V2
}
pred SameVersionWitness {
  some a: State - last | a.record = Ready and a.locked = Yes
    and alreadyCurrent[a,next[a]] and result[next[a]] = Managed
}
pred FailedReplacementWitness {
  some a: State - last | a.record = Ready and a.locked = Yes
    and failure[a,next[a]] and result[next[a]] = Managed
}
pred MetadataFailureWitness {
  some a: State - last | a.record = Ready and a.recordVersion = V1
    and a.latestVersion = V2 and metadataFailure[a,next[a]]
    and next[a].recordVersion = V1 and next[a].latestVersion = V2
}
pred LatestChangesWhileLockedWitness {
  some a,b,c: State | next[a] = b and next[b] = c
    and a.locked = Yes and a.stage = Staging and a.latestVersion = V1
    and advanceLatest[a,b] and resolveLatest[b,c] and c.candidateVersion = V2
}
check Resolution for 10 State expect 0
check Publication for 10 State expect 0
check FailurePreserves for 10 State expect 0
check PublishRequiresChecks for 10 State expect 0
check LatestSnapshotUnderLock for 10 State expect 0
check PublishUsesSnapshot for 10 State expect 0
check OlderVersionRemainsManaged for 10 State expect 0
check LatestChangePreservesRecord for 10 State expect 0
run UpdateWitness for 12 State expect 1
run SameVersionWitness for 10 State expect 1
run FailedReplacementWitness for 10 State expect 1
run MetadataFailureWitness for 10 State expect 1
run LatestChangesWhileLockedWitness for 6 State expect 1
