// Bounded browser contract for a single pending Planning question. The two
// Request atoms represent distinct identities, not just a pending boolean.
// Browser BDD separately checks the concrete session URL and answer payload.
module planning_answer_keyboard
open util/ordering[State]

abstract sig Event {}
one sig Initial, Question, Reload, TypeAnswer, Erase, PlainEnter, MetaEnter,
  CtrlEnter, OtherTargetEnter, SendButton, Accept, Fail extends Event {}
abstract sig Draft {}
one sig Empty, Text, Multiline extends Draft {}
sig Request {}
sig State {
  event: one Event,
  pending, inFlight, displayed, posted: lone Request,
  draft: one Draft
}

pred initial[s: State] {
  s.event = Initial
  no s.pending + s.inFlight + s.displayed + s.posted
  s.draft = Empty
}

pred transition[a, b: State] {
  b.event != Initial
  b.event = Question implies {
    no a.pending
    one b.pending
  } else b.pending = (b.event = Accept => none else a.pending)
  b.displayed = b.pending

  b.event in Reload + TypeAnswer + Erase + PlainEnter + MetaEnter + CtrlEnter + SendButton
    implies some a.displayed
  b.event in Accept + Fail implies some a.inFlight
  // Reload is modeled before the POST; transport handling across reloads is
  // outside this contract. HTMX disables the textarea during the POST.
  b.event in Reload + TypeAnswer + Erase + PlainEnter implies no a.inFlight
  b.draft = (b.event in Question + Reload + Erase + Accept => Empty else
    (b.event = TypeAnswer => Text else (b.event = PlainEnter => Multiline else a.draft)))

  b.posted = (b.event in (MetaEnter + CtrlEnter + SendButton) and a.draft != Empty and no a.inFlight => a.displayed else none)
  b.inFlight = (b.event in Accept + Fail => none else a.inFlight + b.posted)
}

fact Traces {
  initial[first]
  all a: State - last | transition[a, next[a]]
}

assert PlainEnterEditsWithoutPosting {
  all a: State - last | let b = next[a] | b.event = PlainEnter implies {
    no b.posted
    b.draft = Multiline
    b.displayed = a.displayed
  }
}
assert RestrictedSubmissions {
  all a: State - last | let b = next[a] | {
    (b.event = OtherTargetEnter or a.draft = Empty or some a.inFlight) implies no b.posted
    some b.posted implies b.posted = a.pending and b.draft = a.draft
    b.event in MetaEnter + CtrlEnter + SendButton and a.draft != Empty and no a.inFlight
      implies some b.posted
  }
}
assert ReloadRetainsQuestionAndAcceptanceClearsIt {
  all a: State - last | let b = next[a] | {
    b.event = Reload implies b.displayed = a.pending and some b.displayed and no b.posted
    b.event = Accept implies no b.pending + b.displayed + b.inFlight + b.posted
    b.event = Fail implies b.displayed = a.pending and no b.inFlight + b.posted
  }
}

pred CmdLifecycle {
  let q = next[first], loaded = next[q], typed = next[loaded], newline = next[typed], submit = next[newline] | {
    q.event = Question
    loaded.event = Reload and loaded.displayed = q.pending
    typed.event = TypeAnswer
    newline.event = PlainEnter and no newline.posted
    submit.event = MetaEnter and submit.draft = Multiline and submit.posted = q.pending
    last.event = Accept
  }
}
pred CtrlValidatesAndDropsRepeat {
  let q = next[first], blank = next[q], typed = next[blank], submit = next[typed], repeat = next[submit] | {
    q.event = Question
    blank.event = CtrlEnter and no blank.posted
    typed.event = TypeAnswer
    submit.event = CtrlEnter and some submit.posted
    repeat.event = CtrlEnter and no repeat.posted and repeat.inFlight = submit.posted
    last.event = Accept
  }
}
pred OtherTargetAndButtonRecovery {
  let q = next[first], typed = next[q], other = next[typed], submit = next[other], failed = next[submit] | {
    q.event = Question
    typed.event = TypeAnswer
    other.event = OtherTargetEnter and no other.posted
    submit.event = SendButton and some submit.posted
    failed.event = Fail
    last.event = SendButton and last.posted = submit.posted
  }
}
pred OnlyNewRequestAfterAcceptance {
  let q1 = next[first], typed1 = next[q1], submit1 = next[typed1], done = next[submit1],
      q2 = next[done], typed2 = next[q2] | {
    q1.event = Question
    typed1.event = TypeAnswer
    submit1.event = MetaEnter and submit1.posted = q1.pending
    done.event = Accept
    q2.event = Question and q2.pending != q1.pending
    typed2.event = TypeAnswer
    last.event = CtrlEnter and last.posted = q2.pending
  }
}

check PlainEnterEditsWithoutPosting for 8 State, 2 Request expect 0
check RestrictedSubmissions for 8 State, 2 Request expect 0
check ReloadRetainsQuestionAndAcceptanceClearsIt for 8 State, 2 Request expect 0
run CmdLifecycle for 7 State, exactly 2 Request expect 1
run CtrlValidatesAndDropsRepeat for 7 State, exactly 2 Request expect 1
run OtherTargetAndButtonRecovery for 7 State, exactly 2 Request expect 1
run OnlyNewRequestAfterAcceptance for 8 State, exactly 2 Request expect 1
