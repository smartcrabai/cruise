// Independent bounded abstraction of one provider and one installation actor.
module provider_install
open util/ordering[State]
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Record {}
one sig Missing, Ready, Stale, Damaged extends Record {}
abstract sig Stage {}
one sig Idle, Staging, Verified extends Stage {}
abstract sig Result {}
one sig Override, Managed, Path, Error extends Result {}
sig State {
  envSet, envValid, pathHas: one Bool,
  record: one Record, published, digestOk, probeOk: one Bool,
  locked: one Bool, stage: one Stage, candidateDigest, candidateProbe: one Bool
}
fun result[s: State]: one Result {
  s.envSet = Yes => (s.envValid = Yes => Override else Error)
  else (s.record = Ready => Managed else (s.pathHas = Yes => Path else Error))
}
pred initial[s: State] {
  s.envSet = No and s.envValid = No and s.pathHas = No
  s.record = Missing and s.published = No and s.digestOk = No and s.probeOk = No
  s.locked = No and s.stage = Idle and s.candidateDigest = No and s.candidateProbe = No
}
pred sameEnv[a,b: State] { b.envSet = a.envSet and b.envValid = a.envValid and b.pathHas = a.pathHas }
pred samePublished[a,b: State] { b.record = a.record and b.published = a.published and b.digestOk = a.digestOk and b.probeOk = a.probeOk }
pred sameWork[a,b: State] { b.locked = a.locked and b.stage = a.stage and b.candidateDigest = a.candidateDigest and b.candidateProbe = a.candidateProbe }
pred environment[a,b: State] { samePublished[a,b] and sameWork[a,b] }
pred begin[a,b: State] {
  a.locked = No and sameEnv[a,b] and samePublished[a,b]
  b.locked = Yes and b.stage = Staging and b.candidateDigest = No and b.candidateProbe = No
}
pred digest[a,b: State] {
  a.locked = Yes and a.stage = Staging and sameEnv[a,b] and samePublished[a,b]
  b.locked = Yes and b.stage = Staging and b.candidateDigest = Yes and b.candidateProbe = a.candidateProbe
}
pred probe[a,b: State] {
  a.locked = Yes and a.stage = Staging and a.candidateDigest = Yes and sameEnv[a,b] and samePublished[a,b]
  b.locked = Yes and b.stage = Verified and b.candidateDigest = Yes and b.candidateProbe = Yes
}
pred publish[a,b: State] {
  a.locked = Yes and a.stage = Verified and a.candidateDigest = Yes and a.candidateProbe = Yes and sameEnv[a,b]
  b.record = Ready and b.published = Yes and b.digestOk = Yes and b.probeOk = Yes
  b.locked = No and b.stage = Idle and b.candidateDigest = a.candidateDigest and b.candidateProbe = a.candidateProbe
}
pred failure[a,b: State] {
  a.locked = Yes and sameEnv[a,b] and samePublished[a,b]
  b.locked = No and b.stage = Idle and b.candidateDigest = No and b.candidateProbe = No
}
pred deteriorate[a,b: State] {
  a.locked = No and a.record = Ready and b.record in Stale + Damaged
  sameEnv[a,b] and sameWork[a,b]
  b.published = a.published and b.digestOk = a.digestOk and b.probeOk = a.probeOk
}
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] |
    environment[a,b] or begin[a,b] or digest[a,b] or probe[a,b] or publish[a,b] or failure[a,b] or deteriorate[a,b]
}
assert Resolution {
  all s: State | {
    s.envSet = Yes and s.envValid = Yes implies result[s] = Override
    s.envSet = Yes and s.envValid = No implies result[s] = Error
    result[s] = Managed implies s.envSet = No and s.record = Ready
    s.record in Stale + Damaged implies result[s] != Managed
    s.envSet = No and s.record != Ready and s.pathHas = Yes implies result[s] = Path
  }
}
assert Publication {
  all s: State | {
    s.published = Yes implies s.digestOk = Yes and s.probeOk = Yes
    s.record = Ready implies s.published = Yes
    s.stage != Idle implies s.locked = Yes
  }
}
assert FailurePreserves { all a: State - last | failure[a,next[a]] implies samePublished[a,next[a]] }
assert PublishRequiresChecks { all a: State - last | publish[a,next[a]] implies a.locked = Yes and a.candidateDigest = Yes and a.candidateProbe = Yes }
pred ReinstallWitness {
  some a,b,c: State | a in b.^prev and b in c.^prev and a.record = Ready and b.record = Stale and c.record = Ready
}
pred FailedReplacementWitness {
  some a: State - last | a.record = Ready and a.locked = Yes and failure[a,next[a]] and result[next[a]] = Managed
}
check Resolution for 10 State expect 0
check Publication for 10 State expect 0
check FailurePreserves for 10 State expect 0
check PublishRequiresChecks for 10 State expect 0
run ReinstallWitness for 10 State expect 1
run FailedReplacementWitness for 10 State expect 1
