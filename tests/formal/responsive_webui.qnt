// Observable responsive shell contract, plan §§0, 3, 4.1-4.3 and 6.
// CSS geometry and real htmx confirmation are checked by production browser BDD.
// Width samples quotient drag distances into lower/interior/upper boundary cases.
module responsive_webui {
  type Shell = { mobile: bool, open: bool, expanded: bool, backdrop: bool,
    width: int, saved: int, toggleFocused: bool }
  var s: Shell
  var before: Shell
  var event: str
  pure def bounded(w: int): int = if (w < 180) 180 else if (w > 600) 600 else w
  action start(mobile: bool, stored: int): bool = all {
    s' = { mobile: mobile, open: false, expanded: false, backdrop: false,
      width: bounded(stored), saved: stored, toggleFocused: false },
    before' = { mobile: mobile, open: false, expanded: false, backdrop: false,
      width: bounded(stored), saved: stored, toggleFocused: false }, event' = "init" }
  action init = {
    nondet mobile = Set(false, true).oneOf()
    nondet stored = Set(100, 180, 288, 600, 700).oneOf()
    start(mobile, stored)
  }
  action toggle = all { s.mobile, before' = s, event' = "toggle",
    s' = { ...s, open: not(s.open), expanded: not(s.open), backdrop: not(s.open) } }
  action dismiss(reason: str): bool = all {
    Set("backdrop", "escape", "mainRequest").contains(reason), before' = s, event' = reason,
    s' = { ...s, open: false, expanded: false, backdrop: false,
      toggleFocused: if (reason == "escape" and s.open) true else s.toggleFocused } }
  action retain(reason: str): bool = all {
    Set("poll", "settings", "cancelConfirm").contains(reason),
    before' = s, event' = reason, s' = s }
  action crossBreakpoint = all { before' = s, event' = "breakpoint",
    s' = { ...s, mobile: not(s.mobile), open: false, expanded: false, backdrop: false } }
  action resize(requested: int): bool = all { not(s.mobile), before' = s,
    event' = "resize", s' = { ...s, width: bounded(requested), saved: bounded(requested) } }
  action resizeSample = {
    nondet requested = Set(164, 180, 196, 272, 288, 304, 584, 600, 616).oneOf()
    resize(requested)
  }
  action step = any { toggle, dismiss("backdrop"), dismiss("escape"),
    dismiss("mainRequest"), retain("poll"), retain("settings"),
    retain("cancelConfirm"), crossBreakpoint, resizeSample }

  val accessibility = s.open == s.expanded and s.open == s.backdrop
  val desktop = not(s.mobile) implies not(s.open)
  val widthBounds = s.width >= 180 and s.width <= 600
  val selectiveClose = all {
    Set("backdrop", "escape", "mainRequest", "breakpoint").contains(event) implies not(s.open),
    Set("poll", "settings", "cancelConfirm").contains(event) implies s == before,
    event == "toggle" implies s.open != before.open,
    event == "escape" and before.open implies s.toggleFocused,
  }
  val persistence = all {
    event == "resize" implies s.saved == s.width,
    event != "resize" and event != "init" implies
      s.saved == before.saved and s.width == before.width,
  }
  val safety = accessibility and desktop and widthBounds and selectiveClose and persistence

  run escapeFocusTest = start(true, 288).then(toggle).then(dismiss("escape"))
    .expect(not(s.open) and s.toggleFocused and safety)
  run canceledRunAllTest = start(true, 288).then(toggle).then(retain("cancelConfirm"))
    .expect(s.open and safety)
  run pollAndSettingsTest = start(true, 288).then(toggle).then(retain("poll"))
    .then(retain("settings")).expect(s.open and safety)
  run navigationTest = start(true, 288).then(toggle).then(dismiss("mainRequest"))
    .expect(not(s.open) and safety)
  run backdropTest = start(true, 288).then(toggle).then(dismiss("backdrop"))
    .expect(not(s.open) and safety)
  run toggleTwiceTest = start(true, 288).then(toggle).then(toggle).expect(not(s.open) and safety)
  run breakpointPersistenceTest = start(false, 288).then(resize(304)).then(crossBreakpoint)
    .then(toggle).then(crossBreakpoint).expect(not(s.open) and s.width == 304 and s.saved == 304 and safety)
  run lowerClampTest = start(false, 100).then(resize(164)).expect(s.width == 180 and s.saved == 180 and safety)
  run upperClampTest = start(false, 700).then(resize(616)).expect(s.width == 600 and s.saved == 600 and safety)
}
