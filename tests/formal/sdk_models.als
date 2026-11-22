// Jcode-only ordered model fallback with effort-identity deduplication.
module sdk_models
open util/ordering[State]
open util/integer
abstract sig Phase {}
one sig Searching, Executing, Done extends Phase {}
abstract sig Event {}
one sig Init, Start, Fail, Text, Succeed, Blocked, Idle extends Event {}
abstract sig Bool {}
one sig Yes, No extends Bool {}
sig State {
  phase: one Phase,
  cursor, current, attempts: one Int,
  attempted: set Int,
  visible: one Bool,
  event: one Event
}
fun candidateAt[p: Int]: one Int { p <= 1 => 0 else 1 }
fun nextPosition[a: State]: one Int {
  (a.cursor <= 0 and 0 not in a.attempted) => 0 else
  (a.cursor <= 1 and 0 not in a.attempted) => 1 else
  (a.cursor <= 2 and 1 not in a.attempted) => 2 else 3
}
pred assign[b: State, ph: Phase, c, m, n: Int, seen: set Int, v: Bool, e: Event] {
  b.phase = ph and b.cursor = c and b.current = m and b.attempts = n
  b.attempted = seen and b.visible = v and b.event = e
}
pred initial[a: State] { assign[a,Searching,0,-1,0,none,No,Init] }
pred start[a,b: State] {
  a.phase = Searching
  let p = nextPosition[a] | {
    p < 3
    assign[b,Executing,a.cursor,candidateAt[p],a.attempts.add[1],a.attempted,No,Start]
  }
}
pred blocked[a,b: State] {
  a.phase = Searching and nextPosition[a] = 3
  assign[b,Done,a.cursor,-1,a.attempts,a.attempted,a.visible,Blocked]
}
pred fail[a,b: State] {
  a.phase = Executing
  assign[b,(a.visible = Yes => Done else Searching),a.cursor.add[1],a.current,
    a.attempts,a.attempted + a.current,a.visible,Fail]
}
pred text[a,b: State] {
  a.phase = Executing
  assign[b,Executing,a.cursor,a.current,a.attempts,a.attempted,Yes,Text]
}
pred succeed[a,b: State] {
  a.phase = Executing
  assign[b,Done,a.cursor,a.current,a.attempts,a.attempted,a.visible,Succeed]
}
pred idle[a,b: State] {
  assign[b,a.phase,a.cursor,a.current,a.attempts,a.attempted,a.visible,Idle]
}
fact Trace {
  initial[first]
  all a: State - last | let b = next[a] |
    start[a,b] or blocked[a,b] or fail[a,b] or text[a,b] or succeed[a,b] or idle[a,b]
}
assert Selection {
  all a: State - last | let b = next[a] |
    b.event = Start implies (a.phase = Searching and b.current = candidateAt[nextPosition[a]]
      and b.current not in a.attempted)
}
assert FallbackSafety {
  all a: State - last | let b = next[a] |
    a.attempted in b.attempted
    and (b.event = Fail implies b.attempted = a.attempted + a.current)
    and (a.visible = Yes implies b.visible = Yes and b.event != Start)
    and (b.event = Start and a.attempts > 0 implies a.event = Fail and a.phase = Searching)
    and b.attempts <= 2
}
assert Domains {
  all a: State | a.cursor >= 0 and a.cursor <= 4
    and a.current >= -1 and a.current <= 1
    and a.attempts >= 0 and a.attempts <= 2
    and a.attempted in 0 + 1
}
pred OrderedFallbackWitness {
  some disj a,b: State | lt[a,b] and a.event = Start and a.current = 0
    and b.event = Start and b.current = 1
  last.phase = Done and last.event = Succeed and last.attempts = 2
}
pred VisibleFailureWitness {
  some a,b: State | a.event = Text and b.event = Fail and next[a] = b
  last.phase = Done and last.attempts = 1 and last.visible = Yes
}
pred ExhaustedWitness {
  last.phase = Done and last.event = Blocked and last.attempts = 2
  last.attempted = 0 + 1
}
check Selection for 9 State, 5 Int expect 0
check FallbackSafety for 9 State, 5 Int expect 0
check Domains for 9 State, 5 Int expect 0
run OrderedFallbackWitness for 9 State, 5 Int expect 1
run VisibleFailureWitness for 9 State, 5 Int expect 1
run ExhaustedWitness for 9 State, 5 Int expect 1
