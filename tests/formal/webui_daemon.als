// Bounded abstraction of the WebUI registry protocol, not the HTTP or OS implementation.
// A PID can be reused with a different process-start identity. Locks serialize
// start/stop, while the daemon child publishes under the parent's lock.
module webui_daemon
open util/ordering[State]
abstract sig Bool {} one sig Yes, No extends Bool {}
abstract sig Event {} one sig Init, Start, Publish, Stop, Crash, Reuse, Status, Idle extends Event {}
one sig Owner, Other {}
abstract sig Identity {} one sig Original, Reused extends Identity {}
sig State {
  event: one Event,
  lock: lone Owner,
  pending: one Bool,
  process: lone Identity,
  record: lone Identity,
  signalled: lone Identity,
  reported: one Bool,
  sessions: one Bool
}
pred live[s: State] { some s.record and s.record = s.process }
pred init[s: State] {
  s.event = Init and no s.lock and s.pending = No
  no s.process and no s.record and no s.signalled
  s.reported = No and s.sessions = No
}
pred step[a,b: State] {
  b.event != Init
  b.sessions = a.sessions
  b.event != Stop implies no b.signalled
  b.event != Status implies b.reported = No
  b.event = Start implies {
    no a.lock and not live[a]
    b.lock = Owner and b.pending = Yes
    no b.record and b.process = a.process
  }
  b.event = Publish implies {
    a.lock = Owner and a.pending = Yes and not live[a]
    no b.lock and b.pending = No
    b.process = Original and b.record = Original
  }
  b.event = Stop implies {
    no a.lock and live[a]
    no b.lock and b.pending = No
    b.signalled = a.record and no b.process and no b.record
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
}
fact Traces { init[first] all a: State - last | step[a,next[a]] }
assert SignalIdentity { all a: State - last | let b = next[a] | some b.signalled implies b.signalled = a.process and b.signalled = a.record }
assert StatusIdentity { all a: State - last | next[a].event = Status implies (next[a].reported = Yes iff live[a]) }
assert NoAutomaticExecution { all s: State | s.sessions = No }
assert PublishExcludesLiveRecord { all a: State - last | next[a].event = Publish implies not live[a] }
pred StartStop { some a: State - last | next[a].event = Stop }
pred CrashReuseStatus { some a: State - last | a.event = Reuse and next[a].event = Status and next[a].reported = No }
check SignalIdentity for exactly 6 State expect 0
check StatusIdentity for exactly 6 State expect 0
check NoAutomaticExecution for exactly 6 State expect 0
check PublishExcludesLiveRecord for exactly 6 State expect 0
run StartStop for exactly 6 State expect 1
run CrashReuseStatus for exactly 6 State expect 1
