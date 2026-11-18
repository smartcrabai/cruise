// Bounded abstraction of the per-user daemon registry protocol, not the HTTP
// or OS implementation. A PID may be reused with a different process identity.
// The lock serializes autostart contenders through publication and readiness.
module daemon
open util/ordering[State]
abstract sig Bool {} one sig Yes, No extends Bool {}
abstract sig Event {}
one sig Init, Start, Autostart, AutostartWait, AutostartExisting, Publish, Ready,
  Stop, Crash, Reuse, Status, Idle, RegistryGone, Takeover,
  BindUnspecifiedBusy, BindExplicitBusy, MarkMismatchIdle, MarkMismatchBusy,
  MismatchIdleRestart, MismatchBusyRefusal, ApiRequest, IssueLoginNonce,
  ConsumeLoginNonce, ReuseLoginNonce extends Event {}
one sig Owner, Other {}
abstract sig Identity {} one sig Original, Reused extends Identity {}
abstract sig BindOutcome {}
one sig DefaultPort, EphemeralFallback, ExplicitFailure extends BindOutcome {}
abstract sig ClientOutcome {}
one sig NoClientOutcome, ExistingInstance, RestartedInstance, RefusedClient
  extends ClientOutcome {}
sig State {
  event: one Event,
  lock: lone Owner,
  pending: one Bool,
  process: lone Identity,
  record: lone Identity,
  signalled: lone Identity,
  reported: one Bool,
  sessions: one Bool,
  work: one Bool,
  mismatch: one Bool,
  token: lone Identity,
  requestToken: lone Identity,
  requestAccepted: one Bool,
  loginNonce: lone Identity,
  nonceAvailable: one Bool,
  loginAccepted: one Bool,
  webSession: lone Identity,
  bindOutcome: one BindOutcome,
  client: one ClientOutcome
}
pred live[s: State] { some s.record and s.record = s.process }
pred init[s: State] {
  s.event = Init and no s.lock and s.pending = No
  no s.process and no s.record and no s.signalled and no s.token
  no s.loginNonce and no s.webSession
  s.reported = No and s.sessions = No and s.work = No and s.mismatch = No
  s.nonceAvailable = No and s.loginAccepted = No
  no s.requestToken and s.requestAccepted = No and s.bindOutcome = DefaultPort
  s.client = NoClientOutcome
}
pred step[a,b: State] {
  b.event != Init
  b.sessions = a.sessions
  b.event != Stop implies no b.signalled
  b.event != Status implies b.reported = No
  b.event != ApiRequest implies b.requestAccepted = No
  b.event not in IssueLoginNonce + Start + Autostart + Publish + Stop
    + RegistryGone + Takeover + MismatchIdleRestart implies
    b.loginNonce = a.loginNonce
  b.event not in IssueLoginNonce + ConsumeLoginNonce + ReuseLoginNonce
    + Start + Autostart + Publish + Stop + RegistryGone + Takeover
    + MismatchIdleRestart implies
    b.nonceAvailable = a.nonceAvailable
  b.event not in IssueLoginNonce + ConsumeLoginNonce + ReuseLoginNonce
    + Start + Autostart + Publish + Stop + RegistryGone + Takeover
    + MismatchIdleRestart implies
    b.loginAccepted = a.loginAccepted
  b.event not in ConsumeLoginNonce + Start + Autostart + Publish + Stop
    + RegistryGone + Takeover + MismatchIdleRestart implies
    b.webSession = a.webSession
  b.event not in BindUnspecifiedBusy + BindExplicitBusy implies
    b.bindOutcome = a.bindOutcome
  b.event not in AutostartExisting + MismatchIdleRestart + MismatchBusyRefusal
    implies b.client = NoClientOutcome
  b.event not in MarkMismatchIdle + MarkMismatchBusy + MismatchIdleRestart
    + MismatchBusyRefusal implies b.mismatch = a.mismatch
  b.event not in MarkMismatchIdle + MarkMismatchBusy implies b.work = a.work
  b.event not in Publish + Stop + RegistryGone + Takeover + MismatchIdleRestart
    implies b.token = a.token
  b.event not in Start + Autostart + Publish + Stop + RegistryGone + Takeover
    + MismatchIdleRestart implies b.record = a.record
  b.event not in Publish + Stop + Crash + Reuse + RegistryGone + Takeover
    + MismatchIdleRestart implies b.process = a.process
  b.event not in Start + Autostart + Publish + Ready + Stop + Crash + Reuse + Status
    + Idle + RegistryGone + Takeover + BindUnspecifiedBusy + BindExplicitBusy
    + MarkMismatchIdle + MarkMismatchBusy + MismatchIdleRestart
    + MismatchBusyRefusal + ApiRequest + AutostartWait + AutostartExisting
    implies no b.lock and b.pending = No

  b.event = Start implies {
    no a.lock and not live[a]
    b.lock = Owner and b.pending = Yes
    no b.record and b.process = a.process and no b.token
    no b.loginNonce and no b.webSession and b.nonceAvailable = No and
      b.loginAccepted = No
  }
  b.event = Autostart implies {
    no a.lock and not live[a]
    b.lock = Owner and b.pending = Yes
    no b.record and b.process = a.process and no b.token
    no b.loginNonce and no b.webSession and b.nonceAvailable = No and
      b.loginAccepted = No
  }
  b.event = AutostartWait implies {
    a.lock = Owner and a.pending = Yes
    b.lock = a.lock and b.pending = a.pending
    b.process = a.process and b.record = a.record and b.token = a.token
  }
  b.event = AutostartExisting implies {
    no a.lock and live[a]
    no b.lock and b.pending = No
    b.process = a.process and b.record = a.record and b.token = a.token
    b.client = ExistingInstance
  }
  b.event = Publish implies {
    a.lock = Owner and a.pending = Yes and not live[a]
    b.lock = Owner and b.pending = Yes
    b.process = Original and b.record = Original and b.token = Original
    no b.loginNonce and no b.webSession and b.nonceAvailable = No and
      b.loginAccepted = No
  }
  b.event = Ready implies {
    a.lock = Owner and a.pending = Yes and live[a]
    no b.lock and b.pending = No
    b.process = a.process and b.record = a.record and b.token = a.token
  }
  b.event = Stop implies {
    no a.lock and live[a]
    no b.lock and b.pending = No
    b.signalled = a.record and no b.process and no b.record and no b.token
    no b.loginNonce and no b.webSession and b.nonceAvailable = No and
      b.loginAccepted = No
  }
  b.event = Crash implies {
    some a.process and no a.lock
    no b.process and b.record = a.record and no b.lock and b.pending = No
  }
  b.event = Reuse implies {
    no a.lock and no a.process and some a.record
    b.process = Reused and b.record = a.record and no b.lock and b.pending = No
  }
  b.event = Status implies {
    b.lock = a.lock and b.pending = a.pending and b.process = a.process and b.record = a.record
    (live[a] implies b.reported = Yes) and (not live[a] implies b.reported = No)
  }
  b.event = Idle implies {
    b.lock = a.lock and b.pending = a.pending and b.process = a.process and b.record = a.record
  }
  b.event = RegistryGone implies {
    no a.lock and live[a]
    no b.lock and b.pending = No and no b.process and no b.record and no b.token
    no b.loginNonce and no b.webSession and b.nonceAvailable = No and
      b.loginAccepted = No
  }
  b.event = Takeover implies {
    no a.lock and live[a]
    no b.lock and b.pending = No
    a.record = Original and b.record = Reused
    b.process = Reused and b.token = Reused
    no b.loginNonce and no b.webSession and b.nonceAvailable = No and
      b.loginAccepted = No
  }
  b.event = BindUnspecifiedBusy implies {
    b.bindOutcome = EphemeralFallback
    b.lock = a.lock and b.pending = a.pending
    b.process = a.process and b.record = a.record
  }
  b.event = BindExplicitBusy implies {
    b.bindOutcome = ExplicitFailure
    b.lock = a.lock and b.pending = a.pending
    b.process = a.process and b.record = a.record
  }
  b.event = MarkMismatchIdle implies {
    live[a] and a.work = No
    b.mismatch = Yes and b.work = No
    b.process = a.process and b.record = a.record and b.token = a.token
  }
  b.event = MarkMismatchBusy implies {
    live[a] and a.work = Yes
    b.mismatch = Yes and b.work = Yes
    b.process = a.process and b.record = a.record and b.token = a.token
  }
  b.event = MismatchIdleRestart implies {
    live[a] and a.mismatch = Yes and a.work = No
    no b.lock and b.pending = No
    b.process = Reused and b.record = Reused and b.token = Reused
    no b.loginNonce and no b.webSession and b.nonceAvailable = No and
      b.loginAccepted = No
    b.mismatch = No and b.work = No and b.client = RestartedInstance
  }
  b.event = MismatchBusyRefusal implies {
    live[a] and a.mismatch = Yes and a.work = Yes
    b.lock = a.lock and b.pending = a.pending
    b.process = a.process and b.record = a.record and b.token = a.token
    b.mismatch = Yes and b.work = Yes and b.client = RefusedClient
  }
  b.event = IssueLoginNonce implies {
    live[a] and no a.lock and some a.token
    b.lock = a.lock and b.pending = a.pending
    b.process = a.process and b.record = a.record and b.token = a.token
    some b.loginNonce and b.loginNonce != a.token
    b.nonceAvailable = Yes and b.loginAccepted = No
    b.webSession = a.webSession
  }
  b.event = ConsumeLoginNonce implies {
    live[a] and no a.lock and some a.token
    a.nonceAvailable = Yes and some a.loginNonce
    b.loginNonce = a.loginNonce and b.nonceAvailable = No
    b.loginAccepted = Yes and some b.webSession and b.webSession != a.token
    b.lock = a.lock and b.pending = a.pending
    b.process = a.process and b.record = a.record and b.token = a.token
  }
  b.event = ReuseLoginNonce implies {
    live[a] and no a.lock and some a.loginNonce
    a.nonceAvailable = No
    b.loginNonce = a.loginNonce and b.nonceAvailable = No
    b.loginAccepted = No and b.webSession = a.webSession
    b.lock = a.lock and b.pending = a.pending
    b.process = a.process and b.record = a.record and b.token = a.token
  }
  b.event = ApiRequest implies {
    live[a]
    b.lock = a.lock and b.pending = a.pending
    b.process = a.process and b.record = a.record and b.token = a.token
    (b.requestAccepted = Yes) iff (some b.requestToken and b.requestToken = a.token)
  }
}
fact Traces { init[first] all a: State - last | step[a,next[a]] }

assert SignalIdentity {
  all a: State - last | let b = next[a] |
    some b.signalled implies b.signalled = a.process and b.signalled = a.record
}
assert StatusIdentity {
  all a: State - last | next[a].event = Status implies
    (next[a].reported = Yes iff live[a])
}
assert NoAutomaticExecution { all s: State | s.sessions = No }
assert PublishExcludesLiveRecord {
  all a: State - last | next[a].event = Publish implies not live[a]
}
assert SingleLiveOwner {
  all s: State | lone (s.process & s.record)
}
assert AutostartUsesRegistryLock {
  all a: State - last | let b = next[a] |
    b.event = Autostart implies no a.lock and not live[a] and
      b.lock = Owner and b.pending = Yes
}
assert AutostartWaitsForSingleOwner {
  all a: State - last | let b = next[a] |
    b.event = AutostartWait implies a.lock = Owner and b.lock = Owner and
      b.process = a.process and b.record = a.record
}
assert ParentHoldsLockThroughReadiness {
  all a: State - last | let b = next[a] |
    b.event = Publish implies b.lock = Owner and b.pending = Yes and live[b]
  all a: State - last | let b = next[a] |
    b.event = Ready implies a.lock = Owner and a.pending = Yes and
      live[a] and no b.lock and b.pending = No
}
assert RegistryDisappearanceStopsOwner {
  all a: State - last | next[a].event = RegistryGone implies
    no next[a].process and no next[a].record
}
assert RegistryTakeoverStopsPreviousOwner {
  all a: State - last | let b = next[a] |
    b.event = Takeover implies a.process = Original and
      b.process = Reused and b.record = Reused
}
assert UnspecifiedPortFallsBack {
  all a: State - last | let b = next[a] |
    b.event = BindUnspecifiedBusy implies b.bindOutcome = EphemeralFallback
}
assert ExplicitPortIsStrict {
  all a: State - last | let b = next[a] |
    b.event = BindExplicitBusy implies b.bindOutcome = ExplicitFailure
}
assert VersionMismatchRestartsWhenIdle {
  all a: State - last | let b = next[a] |
    b.event = MismatchIdleRestart implies live[a] and a.mismatch = Yes and
      a.work = No and b.process = Reused and b.record = Reused and
      b.client = RestartedInstance
}
assert VersionMismatchBusyIsRefused {
  all a: State - last | let b = next[a] |
    b.event = MismatchBusyRefusal implies live[a] and a.mismatch = Yes and
      a.work = Yes and b.process = a.process and b.record = a.record and
      b.client = RefusedClient
}
assert ApiRequiresOwnerToken {
  all a: State - last | let b = next[a] |
    b.event = ApiRequest implies
      (b.requestAccepted = Yes iff some a.token and some b.requestToken and
        b.requestToken = a.token)
}

assert LoginNonceIsSingleUse {
  all a: State - last | let b = next[a] |
    b.event = ConsumeLoginNonce implies a.nonceAvailable = Yes and
      b.nonceAvailable = No and b.loginAccepted = Yes
  all a: State - last | let b = next[a] |
    b.event = ReuseLoginNonce implies a.nonceAvailable = No and
      b.nonceAvailable = No and b.loginAccepted = No and
      b.webSession = a.webSession
}
assert LoginNonceDiffersFromApiToken {
  all s: State | some s.loginNonce implies s.loginNonce != s.token
}
assert LoginSessionDiffersFromApiToken {
  all s: State | some s.webSession implies s.webSession != s.token
}

pred StartStop { some a: State - last | next[a].event = Stop }
pred CrashReuseStatus {
  some a: State - last |
    a.event = Reuse and next[a].event = Status and next[a].reported = No
}
pred AutostartSerializationWitness {
  some a,b,c,d,e: State |
    a.event = Autostart and b.event = Publish and
    c.event = AutostartWait and d.event = Ready and
    e.event = AutostartExisting and next[a] = b and next[b] = c and
    next[c] = d and next[d] = e and b.lock = Owner and
    b.pending = Yes and c.lock = Owner and c.process = b.process and
    c.record = b.record and no d.lock and d.pending = No and
    e.client = ExistingInstance
}
pred IdleVersionRestartWitness {
  some a,b: State - last |
    a.event = MarkMismatchIdle and b.event = MismatchIdleRestart and
    next[a] = b and b.client = RestartedInstance
}
pred MissingTokenWitness {
  some a: State - last |
    a.event = Publish and next[a].event = ApiRequest and
    no next[a].requestToken and next[a].requestAccepted = No
}
pred LoginNonceSingleUseWitness {
  some a,b,c,d,e: State |
    a.event = Publish and b.event = Ready and c.event = IssueLoginNonce and
    d.event = ConsumeLoginNonce and e.event = ReuseLoginNonce and
    next[a] = b and next[b] = c and next[c] = d and next[d] = e and
    c.loginNonce != c.token and d.webSession != d.token and
    e.nonceAvailable = No and e.loginAccepted = No
}
check SignalIdentity for exactly 7 State expect 0
check StatusIdentity for exactly 7 State expect 0
check NoAutomaticExecution for exactly 7 State expect 0
check PublishExcludesLiveRecord for exactly 7 State expect 0
check SingleLiveOwner for exactly 7 State expect 0
check AutostartUsesRegistryLock for exactly 7 State expect 0
check AutostartWaitsForSingleOwner for exactly 7 State expect 0
check ParentHoldsLockThroughReadiness for exactly 7 State expect 0
check RegistryDisappearanceStopsOwner for exactly 7 State expect 0
check RegistryTakeoverStopsPreviousOwner for exactly 7 State expect 0
check UnspecifiedPortFallsBack for exactly 7 State expect 0
check ExplicitPortIsStrict for exactly 7 State expect 0
check VersionMismatchRestartsWhenIdle for exactly 7 State expect 0
check VersionMismatchBusyIsRefused for exactly 7 State expect 0
check ApiRequiresOwnerToken for exactly 7 State expect 0
check LoginNonceIsSingleUse for exactly 7 State expect 0
check LoginNonceDiffersFromApiToken for exactly 7 State expect 0
check LoginSessionDiffersFromApiToken for exactly 7 State expect 0
run StartStop for exactly 7 State expect 1
run CrashReuseStatus for exactly 7 State expect 1
run AutostartSerializationWitness for exactly 7 State expect 1
run IdleVersionRestartWitness for exactly 7 State expect 1
run MissingTokenWitness for exactly 7 State expect 1
run LoginNonceSingleUseWitness for exactly 7 State expect 1
