// Abstract browser contract for the Planning Ask panel, not a DOM/HTMax implementation.
// Two Ask owners catch accidental routing to the first form. Request tokens are
// independent of owners; reload rebuilds the view from the pending question.
module ask_keyboard {
  type Key = Enter | OtherKey
  type Focus = Ask(int) | Editor | OtherElement
  // Unchanged delegates the key to the browser (including modified Editor Enter).
  type Effect = Unchanged | Newline | Cancelled
  type Post = NoPost | AskPost({ session: int, requestId: int })
  type Event = Keydown({ focus: Focus, key: Key, ctrl: bool, meta: bool, valid: bool })
             | Reload | Complete(int) | Accept(int) | Open
  type Observation = { event: Event, priorBusy: Set[int], effect: Effect, post: Post }

  pure val owners = Set(1, 2)
  pure val tokens = Set(10, 20)
  pure val flags = Set(false, true)
  pure val targets = Set(Ask(1), Ask(2), Editor, OtherElement)
  pure val keyboardKeys = Set(Enter, OtherKey)

  var pending: int -> int
  var rendered: int -> int
  var busy: Set[int]
  var closed: Set[int]
  var observed: Observation

  action open(first, second) = all {
    pending' = Map(1 -> first, 2 -> second),
    rendered' = Map(1 -> first, 2 -> second),
    busy' = Set(),
    closed' = Set(),
    observed' = { event: Open, priorBusy: Set(), effect: Unchanged, post: NoPost },
  }

  action init = {
    nondet first = tokens.oneOf()
    nondet second = tokens.oneOf()
    open(first, second)
  }

  // A keydown can still reach the handler during an in-flight request (e.g.
  // synthetic dispatch). The model checks HTMax's drop behavior in that case.
  action keydown(focus, key, ctrl, meta, valid) = {
    val shortcut = key == Enter and (ctrl or meta)
    val event = Keydown({ focus: focus, key: key, ctrl: ctrl, meta: meta, valid: valid })
    val effect = match focus {
      | Ask(owner) => if (closed.contains(owner)) Unchanged
          else if (shortcut) Cancelled else if (key == Enter) Newline else Unchanged
      | Editor => if (key == Enter and not(ctrl or meta)) Newline else Unchanged
      | OtherElement => Unchanged
    }
    val post = match focus {
      | Ask(owner) => if (shortcut and valid and not(busy.contains(owner)) and not(closed.contains(owner)))
          AskPost({ session: owner, requestId: rendered.get(owner) }) else NoPost
      | _ => NoPost
    }
    all {
      pending' = pending,
      rendered' = rendered,
      closed' = closed,
      observed' = { event: event, priorBusy: busy, effect: effect, post: post },
      busy' = match post {
        | AskPost(destination) => busy.union(Set(destination.session))
        | NoPost => busy
      },
    }
  }

  // Completion represents an in-flight request ending without closing the panel
  // (e.g. a failed response); a busy press must not issue a queued follow-up.
  action complete(owner) = all {
    busy.contains(owner),
    pending' = pending,
    rendered' = rendered,
    closed' = closed,
    busy' = busy.exclude(Set(owner)),
    observed' = { event: Complete(owner), priorBusy: busy, effect: Unchanged, post: NoPost },
  }

  // Keep the last token as history, but remove the panel from active use.
  action accept(owner) = all {
    busy.contains(owner),
    pending' = pending,
    rendered' = rendered,
    busy' = busy.exclude(Set(owner)),
    closed' = closed.union(Set(owner)),
    observed' = { event: Accept(owner), priorBusy: busy, effect: Unchanged, post: NoPost },
  }

  // A reload is modeled while the broker still has the question and the view
  // is idle. No claim is made about cross-tab or cross-navigation deduplication.
  action reload = all {
    busy == Set(),
    pending' = pending,
    rendered' = pending,
    closed' = closed,
    busy' = busy,
    observed' = { event: Reload, priorBusy: busy, effect: Unchanged, post: NoPost },
  }

  action step = any {
    {
      nondet focus = targets.oneOf()
      nondet key = keyboardKeys.oneOf()
      nondet ctrl = flags.oneOf()
      nondet meta = flags.oneOf()
      nondet valid = flags.oneOf()
      keydown(focus, key, ctrl, meta, valid)
    },
    { nondet owner = owners.oneOf() complete(owner) },
    { nondet owner = owners.oneOf() accept(owner) },
    reload,
  }

  val stableIdentity = owners.forall(owner => rendered.get(owner) == pending.get(owner))

  val eventContract = match observed.event {
    | Keydown(k) => match k.focus {
      | Ask(owner) => all {
        (observed.effect == Cancelled) == (not(closed.contains(owner)) and k.key == Enter and (k.ctrl or k.meta)),
        (observed.effect == Newline) == (not(closed.contains(owner)) and k.key == Enter and not(k.ctrl) and not(k.meta)),
        if (k.key != Enter or not(k.ctrl or k.meta) or not(k.valid) or observed.priorBusy.contains(owner) or closed.contains(owner))
          observed.post == NoPost and busy == observed.priorBusy
        else all {
          observed.post == AskPost({ session: owner, requestId: pending.get(owner) }),
          busy == observed.priorBusy.union(Set(owner)),
        },
      }
      | Editor => all {
        observed.post == NoPost,
        (observed.effect == Newline) == (k.key == Enter and not(k.ctrl) and not(k.meta)),
        observed.effect != Cancelled,
        busy == observed.priorBusy,
      }
      | OtherElement => observed.post == NoPost and observed.effect == Unchanged and busy == observed.priorBusy
    }
    | Complete(owner) => all {
      observed.post == NoPost,
      observed.priorBusy.contains(owner),
      busy == observed.priorBusy.exclude(Set(owner)),
    }
    | Accept(owner) => all {
      observed.post == NoPost,
      observed.priorBusy.contains(owner),
      closed.contains(owner),
      busy == observed.priorBusy.exclude(Set(owner)),
    }
    | _ => observed.post == NoPost and observed.effect == Unchanged and busy == observed.priorBusy
  }

  val safety = stableIdentity and eventContract and busy.intersect(closed) == Set()

  run plainEnterAndNonEnterTest = open(10, 20)
    .then(keydown(Ask(2), Enter, false, false, true))
    .expect(observed.effect == Newline and observed.post == NoPost)
    .then(keydown(Ask(2), OtherKey, true, true, true))
    .expect(observed.effect == Unchanged and observed.post == NoPost)

  run ctrlAndMetaUseTheOwnerTest = open(10, 20)
    .then(keydown(Ask(2), Enter, true, false, true))
    .expect(observed.post == AskPost({ session: 2, requestId: 20 }))
    .then(keydown(Ask(1), Enter, false, true, true))
    .expect(observed.post == AskPost({ session: 1, requestId: 10 }) and busy == owners)

  run requiredValidationAndEditorIsolationTest = open(10, 20)
    .then(keydown(Ask(1), Enter, false, true, false))
    .expect(observed.effect == Cancelled and observed.post == NoPost and busy == Set())
    .then(keydown(Editor, Enter, true, false, true))
    .expect(observed.effect == Unchanged and observed.post == NoPost and busy == Set())

  run reloadAndDropBusyTest = open(20, 10)
    .then(reload)
    .expect(rendered.get(2) == 10)
    .then(keydown(Ask(2), Enter, false, true, true))
    .expect(observed.post == AskPost({ session: 2, requestId: 10 }))
    .then(keydown(Ask(2), Enter, true, true, true))
    .expect(observed.effect == Cancelled and observed.post == NoPost and busy == Set(2))
    .then(complete(2))
    .expect(observed.post == NoPost and busy == Set())

  run acceptanceDoesNotReplayTest = open(10, 20)
    .then(keydown(Ask(1), Enter, true, false, true))
    .then(accept(1))
    .expect(closed == Set(1) and busy == Set() and observed.post == NoPost)
    .then(reload)
    .expect(closed == Set(1))
    .then(keydown(Ask(1), Enter, false, true, true))
    .expect(observed.post == NoPost and observed.effect == Unchanged)
    .then(keydown(Ask(2), Enter, false, true, true))
    .expect(observed.post == AskPost({ session: 2, requestId: 20 }))
}
