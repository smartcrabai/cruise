// Jcode subagent route ordering, validation, and no-send-on-configuration-error.
module subagent_model {
  type Input = { global: int, local: int, purpose: int, wellformed: bool,
    command: bool, settingFails: bool }
  type State = { phase: int, route: int, agentsCalls: int,
    configured: bool, sent: bool, failed: bool, event: str }
  var input: Input
  var s: State
  var previous: State
  var event: str

  pure def accepted(i: Input): bool = i.wellformed and not(i.command)
  pure def chosen(i: Input): int =
    if (i.purpose == 2) -1
    else if (i.purpose == 0 and i.local != -1) i.local
    else i.global
  pure val initial = { phase: 0, route: -1, agentsCalls: 0,
    configured: false, sent: false, failed: false, event: "init" }
  action initWith(i: Input): bool = all {
    input' = i, s' = initial, previous' = initial, event' = "init"
  }
  action init = {
    nondet global = Set(-1, 0).oneOf()
    nondet local = Set(-1, 0, 1).oneOf()
    nondet purpose = Set(0, 1, 2).oneOf()
    nondet wellformed = Set(false, true).oneOf()
    nondet command = Set(false, true).oneOf()
    nondet settingFails = Set(false, true).oneOf()
    initWith({ global: global, local: local, purpose: purpose,
      wellformed: wellformed, command: command, settingFails: settingFails })
  }
  action record(updated: State, name: str): bool = all {
    input' = input, previous' = s, s' = updated, event' = name
  }
  action validate = all {
    s.phase == 0,
    record({ ...s, phase: if (accepted(input)) 1 else 4,
      failed: not(accepted(input)) }, "validate")
  }
  action configure = {
    val route = chosen(input)
    val failure = route != -1 and input.settingFails
    all {
      s.phase == 1,
      record({ ...s, phase: if (failure) 4 else 2, route: route,
        agentsCalls: s.agentsCalls + (if (route == -1) 0 else 1),
        configured: route != -1 and not(failure), failed: failure }, "configure")
    }
  }
  action send = all {
    s.phase == 2,
    record({ ...s, phase: 3, sent: true }, "send")
  }
  action idle = record(s, "idle")
  action step = any { validate, configure, send, idle }

  val rejection = not(accepted(input)) implies not(s.sent)
  val selection = s.configured implies s.route == chosen(input) and s.route != -1
  val ordering = all {
    event == "configure" and chosen(input) != -1 implies s.agentsCalls == previous.agentsCalls + 1,
    event == "send" implies previous.phase == 2 and
      (previous.route == -1 or previous.configured),
    s.failed implies not(s.sent) and s.phase == 4,
    s.agentsCalls <= 1
  }
  val scope = input.purpose == 2 implies chosen(input) == -1 and s.agentsCalls == 0
  val safety = rejection and selection and ordering and scope

  pure val ordinary = { global: 0, local: 1, purpose: 0, wellformed: true,
    command: false, settingFails: false }
  run stepPrecedenceTest = initWith(ordinary).then(validate).then(configure)
    .expect(s.route == 1 and s.configured and s.agentsCalls == 1 and not(s.sent) and safety)
    .then(send).expect(s.sent and safety)
  run workflowPlanningTest = initWith({ ...ordinary, purpose: 1 }).then(validate)
    .then(configure).then(send).expect(s.route == 0 and s.sent and safety)
  run titleExclusionTest = initWith({ ...ordinary, purpose: 2 }).then(validate)
    .then(configure).expect(s.route == -1 and s.agentsCalls == 0 and not(s.sent) and safety)
  run absentOverridePreservesEnvironmentTest = initWith({ ...ordinary, global: -1, local: -1 })
    .then(validate).then(configure).then(send)
    .expect(s.route == -1 and s.agentsCalls == 0 and s.sent and safety)
  run failedSettingDoesNotSendTest = initWith({ ...ordinary, settingFails: true })
    .then(validate).then(configure).expect(s.failed and not(s.sent) and safety)
  run malformedSelectionRejectedTest = initWith({ ...ordinary, wellformed: false })
    .then(validate).expect(s.failed and not(s.sent) and safety)
  run commandSelectionRejectedTest = initWith({ ...ordinary, command: true })
    .then(validate).expect(s.failed and not(s.sent) and safety)
}
