// Same observable shell as responsive_webui.qnt, 9 states / 8 transitions.
// Width atoms quotient pixels: Below < 180, Minimum=180, Normal=288,
// Raised=304, Maximum=600, Above > 600. Browser BDD checks numeric geometry.
module responsive_webui
open util/ordering[State]
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Width {}
one sig Below, Minimum, Normal, Raised, Maximum, Above extends Width {}
abstract sig Event {}
one sig Init, Toggle, Backdrop, Escape, MainRequest, Poll, Settings, CancelConfirm, Breakpoint, Resize extends Event {}
sig State {
  mobile, opened, expanded, backdrop, toggleFocused: one Bool,
  width, saved: one Width,
  event: one Event
}
fun bounded[w: Width]: one Width { w = Below => Minimum else (w = Above => Maximum else w) }
fun flipped[b: Bool]: one Bool { Bool - b }
pred initial[a: State] {
  a.opened = No and a.expanded = No and a.backdrop = No and a.toggleFocused = No
  a.width = bounded[a.saved] and a.event = Init
}
pred transition[a,b: State] {
  b.event != Init
  // Only visible controls are user-actuatable. Confirm cancellation occurs
  // before htmx:before:request, so it is not a MainRequest.
  b.event = Toggle implies a.mobile = Yes
  b.event = Resize implies a.mobile = No
  b.mobile = (b.event = Breakpoint => flipped[a.mobile] else a.mobile)
  b.opened = (b.event = Toggle => flipped[a.opened] else
    (b.event in Backdrop + Escape + MainRequest + Breakpoint => No else a.opened))
  b.expanded = b.opened and b.backdrop = b.opened
  b.toggleFocused = (b.event = Escape and a.opened = Yes => Yes else a.toggleFocused)
  b.event = Resize implies {
    b.width in Minimum + Normal + Raised + Maximum
    b.saved = b.width
  }
  b.event != Resize implies { b.width = a.width and b.saved = a.saved }
}
fact Traces { initial[first] all a: State - last | transition[a,next[a]] }
assert Accessibility { all a: State | a.opened = a.expanded and a.opened = a.backdrop }
assert DesktopAndBounds { all a: State | {
  a.mobile = No implies a.opened = No
  a.width in Minimum + Normal + Raised + Maximum
} }
assert SelectiveClose { all a: State - last | let b = next[a] | {
  b.event in Backdrop + Escape + MainRequest + Breakpoint implies b.opened = No
  b.event in Poll + Settings + CancelConfirm implies b.opened = a.opened
  b.event = Toggle implies b.opened != a.opened
  b.event = Escape and a.opened = Yes implies b.toggleFocused = Yes
} }
assert Persistence { all a: State - last | let b = next[a] | {
  b.event = Resize implies b.saved = b.width
  b.event != Resize implies b.saved = a.saved and b.width = a.width
} }
// Require actual open states and nontrivial events, not idle-only traces.
pred CancelThenNavigate {
  first.mobile = Yes
  some a: State - last | let b = next[a] | {
    a.opened = Yes and b.event = CancelConfirm and b.opened = Yes
    b != last and next[b].event = MainRequest and next[b].opened = No
  }
}
pred EscapeRestoresFocus {
  some a: State - last | a.opened = Yes and next[a].event = Escape and next[a].toggleFocused = Yes
}
pred ResizeSurvivesCrossing {
  first.mobile = No and first.saved = Normal
  next[first].event = Resize and next[first].width = Raised
  next[next[first]].event = Breakpoint
  next[next[next[first]]].event = Toggle
  next[next[next[next[first]]]].event = Breakpoint
  last.saved = Raised and last.width = Raised
  all a: State - first - next[first] | a.event != Resize
}
pred PollAndSettingsPreserve {
  some a: State - last | let b = next[a] | {
    a.opened = Yes and b.event = Poll and b.opened = Yes
    b != last and next[b].event = Settings and next[b].opened = Yes
  }
}
check Accessibility for 9 State expect 0
check DesktopAndBounds for 9 State expect 0
check SelectiveClose for 9 State expect 0
check Persistence for 9 State expect 0
run CancelThenNavigate for 9 State expect 1
run EscapeRestoresFocus for 9 State expect 1
run ResizeSurvivesCrossing for 9 State expect 1
run PollAndSettingsPreserve for 9 State expect 1
