// Bounded atomic ledger protocol, not a model of filesystem or scheduling failures.
module instruction_inbox
open util/ordering[State] as trace
open util/ordering[Id] as ids
sig Id {}
sig Key {}
abstract sig Event {}
one sig Init, Open, Accept, Duplicate, Reject, Lease, Ack, Release, Completed, Carry, Expire, Crash, Idle extends Event {}
sig State {
  event: one Event, request: lone Key,
  opened: lone State,
  accepted, pending, leased, delivered, expired: set Id,
  keys: Key -> lone Id
}
pred isOpen[s: State] { some s.opened }
pred same[a,b: State] {
  b.opened = a.opened and b.accepted = a.accepted and b.pending = a.pending
  and b.leased = a.leased and b.delivered = a.delivered and b.expired = a.expired and b.keys = a.keys
}
pred common[a,b: State] { b.accepted = a.accepted and b.keys = a.keys }
pred stableTerminal[a,b: State] { b.delivered = a.delivered and b.expired = a.expired }
pred transition[a,b: State] {
  (b.event = Open and not isOpen[a] and isOpen[b] and common[a,b] and stableTerminal[a,b]
    and b.pending = a.pending + a.leased and no b.leased)
  or (b.event = Accept and isOpen[a] and (no b.request or no b.request.(a.keys))
    and some (Id - a.accepted) and let i = ids/min[Id - a.accepted] |
      b.opened = a.opened and b.accepted = a.accepted + i and b.pending = a.pending + i
      and b.leased = a.leased and stableTerminal[a,b] and b.keys = a.keys + b.request->i)
  or (b.event = Duplicate and some b.request.(a.keys) and same[a,b])
  or (b.event = Reject and not isOpen[a] and (no b.request or no b.request.(a.keys)) and same[a,b])
  or (b.event = Lease and isOpen[a] and no a.leased and some a.pending
    and b.opened = a.opened and common[a,b] and stableTerminal[a,b]
    and b.leased = a.pending and no b.pending)
  or (b.event = Ack and isOpen[a] and some a.leased and b.opened = a.opened
    and common[a,b] and b.pending = a.pending and no b.leased
    and b.delivered = a.delivered + a.leased and b.expired = a.expired)
  or (b.event = Release and isOpen[a] and some a.leased and b.opened = a.opened
    and common[a,b] and stableTerminal[a,b] and b.pending = a.pending + a.leased and no b.leased)
  or (b.event in Completed + Carry and isOpen[a] and no b.opened and common[a,b]
    and b.delivered = a.delivered and no b.leased
    and (b.event = Completed implies no b.pending and b.expired = a.expired + a.pending + a.leased)
    and (b.event = Carry implies b.pending = a.pending + a.leased and b.expired = a.expired))
  or (b.event = Expire and not isOpen[a] and no b.opened and common[a,b]
    and b.delivered = a.delivered and b.expired = a.expired + a.pending + a.leased
    and no b.pending and no b.leased)
  or (b.event = Crash and isOpen[a] and no b.opened and common[a,b]
    and stableTerminal[a,b] and b.pending = a.pending and b.leased = a.leased)
  or (b.event = Idle and same[a,b])
}
fact Trace {
  let s = trace/first | no s.opened + s.accepted + s.pending + s.leased + s.delivered + s.expired and no s.keys and s.event = Init
  all a: State - trace/last | transition[a, trace/next[a]]
}
assert Partition { all s: State | s.accepted = s.pending + s.leased + s.delivered + s.expired
  and no (s.pending & (s.leased + s.delivered + s.expired))
  and no (s.leased & (s.delivered + s.expired)) and no (s.delivered & s.expired) }
assert OneLease { all a: State - trace/last | let b = trace/next[a] |
  (b.event = Lease implies no a.leased and b.leased = a.pending)
  and (b.event = Accept implies b.leased = a.leased) }
assert ClosedAcceptance { all a: State - trace/last | let b = trace/next[a] |
  (b.event = Accept implies isOpen[a]) and (b.event = Reject implies not isOpen[a] and same[a,b]) }
assert Terminal { all a: State - trace/last | let b = trace/next[a] |
  a.delivered in b.delivered and a.expired in b.expired and no (b.pending & (a.delivered + a.expired)) }
assert Dedupe { all a: State - trace/last | let b = trace/next[a] |
  (b.event = Duplicate implies some b.request.(a.keys) and same[a,b])
  and (b.event = Accept implies no b.request.(a.keys) and #b.accepted = add[#a.accepted, 1]) }
assert CloseRecovery { all a: State - trace/last | let b = trace/next[a] |
  (b.event = Completed implies no b.pending + b.leased)
  and (b.event in Carry + Open implies b.pending = a.pending + a.leased and no b.leased) }
pred CarryWitness { some a,b,c,d,e,f,g: State |
  a = trace/first and b = trace/next[a] and c = trace/next[b] and d = trace/next[c]
  and e = trace/next[d] and f = trace/next[e] and g = trace/next[f]
  and b.event = Open and c.event = Accept and d.event = Lease and e.event = Carry
  and some e.pending and f.event = Open and g.event = Lease and g.leased = e.pending }
check Partition for 7 but exactly 7 State, exactly 3 Id, exactly 2 Key expect 0
check OneLease for 7 but exactly 7 State, exactly 3 Id, exactly 2 Key expect 0
check ClosedAcceptance for 7 but exactly 7 State, exactly 3 Id, exactly 2 Key expect 0
check Terminal for 7 but exactly 7 State, exactly 3 Id, exactly 2 Key expect 0
check Dedupe for 7 but exactly 7 State, exactly 3 Id, exactly 2 Key expect 0
check CloseRecovery for 7 but exactly 7 State, exactly 3 Id, exactly 2 Key expect 0
run CarryWitness for 7 but exactly 7 State, exactly 3 Id, exactly 2 Key expect 1
