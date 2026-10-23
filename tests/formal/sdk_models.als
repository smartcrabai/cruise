// Same representative contract and abstraction as sdk_models.qnt.
// 13 ordered states = 12 transitions, signed 5-bit integers. No liveness claim.
module sdk_models
open util/ordering[State]
open util/integer
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Event {}
one sig Init, Start, Blocked, Checkpoint, Text, Fail, Succeed, Cancel, NewCall, Idle extends Event {}
one sig Config { base, owner: one Int }
sig State {
  phase, cursor, current, calls, attempts: one Int,
  visible: one Bool, cool: set Int,
  resumeOwner, resumeId, checkpointOwner, checkpointId: one Int,
  event: one Event
}
fact Inputs { Config.base in 0 + 1 and Config.owner in -1 + 0 + 1 }
fun sdk[i: Int]: one Int { i = 1 => 1.sub[Config.base] else Config.base }
fun model[i: Int]: one Int { i = 3 => 1 else 0 }
fun key[i: Int]: one Int { sdk[i].mul[2].add[model[i]] }
fun available[a: State]: one Int {
  (a.cursor <= 0 and key[0] not in a.cool) => 0 else
  (a.cursor <= 1 and key[1] not in a.cool) => 1 else
  (a.cursor <= 2 and key[2] not in a.cool) => 2 else
  (a.cursor <= 3 and key[3] not in a.cool) => 3 else 4
}
pred assign[b: State, p, c, i, n, a: Int, v: Bool, cold: set Int, ro, ri, co, ci: Int] {
  b.phase = p and b.cursor = c and b.current = i and b.calls = n and b.attempts = a
  b.visible = v and b.cool = cold and b.resumeOwner = ro and b.resumeId = ri
  b.checkpointOwner = co and b.checkpointId = ci
}
pred identical[a,b: State] {
  assign[b,a.phase,a.cursor,a.current,a.calls,a.attempts,a.visible,a.cool,
    a.resumeOwner,a.resumeId,a.checkpointOwner,a.checkpointId]
}
pred initial[a: State] { assign[a,0,0,-1,1,0,No,none,-1,0,-1,0] and a.event = Init }
pred start[a,b: State] {
  a.phase = 0 and available[a] < 4
  let i = available[a], target = sdk[i] |
    assign[b,1,a.cursor,i,a.calls,a.attempts.add[1],No,a.cool,
      (Config.owner = target => target else -1), (Config.owner = target => 1 else 0),
      a.checkpointOwner,a.checkpointId]
  b.event = Start
}
pred blocked[a,b: State] {
  a.phase = 0 and available[a] = 4
  assign[b,2,a.cursor,a.current,a.calls,a.attempts,a.visible,a.cool,
    a.resumeOwner,a.resumeId,a.checkpointOwner,a.checkpointId]
  b.event = Blocked
}
pred checkpoint[a,b: State] {
  a.phase = 1
  assign[b,1,a.cursor,a.current,a.calls,a.attempts,a.visible,a.cool,
    a.resumeOwner,a.resumeId,sdk[a.current],a.attempts.add[1]]
  b.event = Checkpoint
}
pred text[a,b: State] {
  a.phase = 1
  assign[b,1,a.cursor,a.current,a.calls,a.attempts,Yes,a.cool,
    a.resumeOwner,a.resumeId,a.checkpointOwner,a.checkpointId]
  b.event = Text
}
pred fail[a,b: State] {
  a.phase = 1
  assign[b,(a.visible = Yes => 2 else 0),a.current.add[1],a.current,a.calls,a.attempts,
    a.visible,a.cool + key[a.current],a.resumeOwner,a.resumeId,a.checkpointOwner,a.checkpointId]
  b.event = Fail
}
pred terminal[a,b: State] {
  a.phase = 1 and b.event in Succeed + Cancel
  assign[b,2,a.cursor,a.current,a.calls,a.attempts,a.visible,a.cool,
    a.resumeOwner,a.resumeId,a.checkpointOwner,a.checkpointId]
}
pred newCall[a,b: State] {
  a.phase = 2 and a.calls < 2
  assign[b,0,0,-1,a.calls.add[1],a.attempts,No,a.cool,
    a.resumeOwner,a.resumeId,a.checkpointOwner,a.checkpointId]
  b.event = NewCall
}
pred idle[a,b: State] { identical[a,b] and b.event = Idle }
pred step[a,b: State] {
  start[a,b] or blocked[a,b] or checkpoint[a,b] or text[a,b] or fail[a,b]
    or terminal[a,b] or newCall[a,b] or idle[a,b]
}
fact Trace { initial[first] and all a: State - last | step[a,next[a]] }
assert Selection {
  all a: State | a.current != 2
  all a: State - last | let b = next[a] | b.event = Start implies
    (b.current = available[a] and key[b.current] not in a.cool
      and (b.current = 1 implies sdk[b.current] != Config.base)
      and (b.current != 1 implies sdk[b.current] = Config.base))
}
assert ResumeSafety {
  all a: State - last | let b = next[a] | b.event = Start implies
    (b.resumeId in 0 + 1 and (b.resumeId = 1 iff Config.owner = sdk[b.current])
      and (b.resumeId = 1 implies b.resumeOwner = sdk[b.current])
      and (b.resumeId != a.checkpointId or a.checkpointId = 0))
}
assert CheckpointSafety {
  all a: State - last | let b = next[a] | b.event = Checkpoint implies
    (b.checkpointOwner = sdk[b.current] and b.checkpointId = b.attempts.add[1])
}
assert CooldownSafety {
  all a: State - last | let b = next[a] |
    a.cool in b.cool
    and (b.event = Fail implies b.cool = a.cool + key[a.current])
    and (b.event != Fail implies b.cool = a.cool)
}
assert ReplaySafety {
  all a: State - last | let b = next[a] |
    ((b.event = Fail and a.visible = Yes) implies b.phase = 2)
    and (b.event = Start implies a.phase = 0)
    and (b.event = Cancel implies (b.phase = 2 and b.attempts = a.attempts))
}
assert Domains {
  all a: State | a.phase >= 0 and a.phase <= 2 and a.cursor >= 0 and a.cursor <= 4
    and a.current >= -1 and a.current <= 3 and a.calls >= 1 and a.calls <= 2
    and a.attempts >= 0 and a.attempts <= 6
    and (all k: a.cool | k >= 0 and k <= 3)
    and (a.phase = 1 implies (a.current >= 0 and a.current < 4))
}
// Witness requires all three starts, SDK restoration, cross-SDK same-model
// eligibility, duplicate same-SDK effort skip, and checkpoint ownership.
pred CrossSdkWitness {
  Config.base = 0 and Config.owner = 0
  some disj a,b,c: State | lt[a,b] and lt[b,c]
    and a.event = Start and a.current = 0 and a.resumeId = 1
    and b.event = Start and b.current = 1 and b.resumeId = 0
    and c.event = Start and c.current = 3 and c.resumeId = 1
  some a: State | a.event = Checkpoint and a.checkpointOwner = 1
}
pred ReverseInheritanceWitness {
  Config.base = 1 and Config.owner = 1
  some a: State | a.event = Start and a.current = 3 and sdk[a.current] = 1 and a.attempts = 3
}
pred VisibleFailureWitness {
  some a: State - last | a.visible = Yes and next[a].event = Fail
  last.phase = 2 and last.attempts = 1 and last.calls = 1
}
pred PolicyReloadWitness {
  some a: State - last | a.event = NewCall and next[a].event = Start
    and next[a].current = 1 and key[0] in a.cool and key[1] not in a.cool
}
pred CancellationWitness {
  Config.base = 1 and Config.owner = 0
  some a: State - last | a.event = Checkpoint and next[a].event = Cancel
    and a.checkpointOwner = 1 and a.resumeId = 0
}
pred AllCoolingWitness {
  last.phase = 2 and last.event = Blocked and last.calls = 2 and last.attempts = 3
  key[0] + key[1] + key[3] in last.cool
}
check Selection for 13 State, 5 Int expect 0
check ResumeSafety for 13 State, 5 Int expect 0
check CheckpointSafety for 13 State, 5 Int expect 0
check CooldownSafety for 13 State, 5 Int expect 0
check ReplaySafety for 13 State, 5 Int expect 0
check Domains for 13 State, 5 Int expect 0
run CrossSdkWitness for 13 State, 5 Int expect 1
run ReverseInheritanceWitness for 13 State, 5 Int expect 1
run VisibleFailureWitness for 13 State, 5 Int expect 1
run PolicyReloadWitness for 13 State, 5 Int expect 1
run CancellationWitness for 13 State, 5 Int expect 1
run AllCoolingWitness for 13 State, 5 Int expect 1
