// Finite abstraction of workflow resolution, not a model of parsing or Rust code.
// 0 means absent, 1 blank, 2 invalid, 3 and 4 distinct valid values.
// Layer precedence: builtin < app < user < project < workflow < env < CLI.
// Session cleanup override is abstracted together with CLI as the final override.
module config_env_override {
  var builtin: int
  var appModel: int
  var userDefault: int
  var projectDefault: int
  var workflow: int
  var env: int
  var cliOverride: int
  var resolved: int
  var snapshotFrozen: bool
  var failed: bool
  var command: bool
  var sdk: bool
  var oldResolved: int
  var oldCommand: bool
  var oldSdk: bool
  var event: str

  pure def choose(high: int, low: int): int = if (high >= 3) high else low
  val base = choose(workflow, choose(projectDefault, choose(userDefault, choose(appModel, builtin))))
  val expected = choose(cliOverride, choose(env, base))
  action init = all {
    builtin' = 3, appModel' = 0, userDefault' = 0, projectDefault' = 0,
    workflow' = 0, env' = 0, cliOverride' = 0, resolved' = 3,
    snapshotFrozen' = false, failed' = false, command' = false, sdk' = false,
    oldResolved' = 3, oldCommand' = false, oldSdk' = false, event' = "init",
  }
  // Inputs may change between resolutions, including after a snapshot is taken.
  action inputs(a: int, u: int, p: int, w: int, e: int, c: int): bool = all {
    appModel' = a, userDefault' = u, projectDefault' = p, workflow' = w,
    env' = e, cliOverride' = c, builtin' = builtin, resolved' = resolved,
    snapshotFrozen' = snapshotFrozen, failed' = failed, command' = command, sdk' = sdk,
    oldResolved' = resolved, oldCommand' = command, oldSdk' = sdk, event' = "inputs",
  }
  // Simultaneous nonblank command and sdk env overrides fail atomically.
  action resolve(cmdEnv: bool, sdkEnv: bool): bool = all {
    not(snapshotFrozen),
    builtin' = builtin, appModel' = appModel, userDefault' = userDefault,
    projectDefault' = projectDefault, workflow' = workflow, env' = env,
    cliOverride' = cliOverride, snapshotFrozen' = snapshotFrozen,
    oldResolved' = resolved, oldCommand' = command, oldSdk' = sdk,
    if (env == 2 or (cmdEnv and sdkEnv)) all {
      failed' = true, resolved' = resolved, command' = command, sdk' = sdk,
      event' = "error",
    } else all {
      failed' = false, resolved' = expected,
      command' = if (cmdEnv) true else if (sdkEnv) false else command,
      sdk' = if (sdkEnv) true else if (cmdEnv) false else sdk,
      event' = "resolve",
    },
  }
  action freeze = all {
    not(failed), not(snapshotFrozen), snapshotFrozen' = true,
    builtin' = builtin, appModel' = appModel, userDefault' = userDefault,
    projectDefault' = projectDefault, workflow' = workflow, env' = env,
    cliOverride' = cliOverride, resolved' = resolved, failed' = failed,
    command' = command, sdk' = sdk, oldResolved' = resolved,
    oldCommand' = command, oldSdk' = sdk, event' = "freeze",
  }
  action step = any {
    inputs(0, 0, 0, 0, 0, 0), inputs(4, 0, 0, 0, 0, 0),
    inputs(3, 4, 0, 0, 0, 0), inputs(3, 3, 4, 0, 0, 0),
    inputs(3, 3, 3, 4, 0, 0), inputs(0, 0, 0, 3, 4, 0),
    inputs(0, 0, 0, 3, 1, 0), inputs(0, 0, 0, 3, 2, 0),
    inputs(0, 0, 0, 3, 4, 3), resolve(false, false),
    resolve(true, false), resolve(false, true), resolve(true, true), freeze,
  }
  val envWins = event == "resolve" and env >= 3 and cliOverride == 0 implies resolved == env
  val cliWins = event == "resolve" and cliOverride >= 3 implies resolved == cliOverride
  val blankIsNoop = event == "resolve" and env == 1 implies resolved == choose(cliOverride, base)
  val invalidFailsClosed = event == "error" implies failed and resolved == oldResolved and command == oldCommand and sdk == oldSdk
  val commandSdkExclusive = not(command and sdk)
  val snapshotStable = snapshotFrozen and event == "inputs" implies resolved == oldResolved
  val safety = envWins and cliWins and blankIsNoop and invalidFailsClosed and commandSdkExclusive and snapshotStable

  run precedenceTest = init.then(inputs(3, 3, 3, 4, 3, 0)).then(resolve(false, false))
    .expect(resolved == 3 and safety)
  run cliTest = init.then(inputs(0, 0, 0, 3, 4, 3)).then(resolve(false, false))
    .expect(resolved == 3 and safety)
  run blankTest = init.then(inputs(0, 0, 0, 4, 1, 0)).then(resolve(false, false))
    .expect(resolved == 4 and safety)
  run invalidTest = init.then(inputs(0, 0, 0, 4, 2, 0)).then(resolve(false, false))
    .expect(failed and resolved == 3 and safety)
  run exclusiveTest = init.then(resolve(true, false)).then(resolve(false, true))
    .then(resolve(true, true)).expect(failed and sdk and not(command) and safety)
  run snapshotTest = init.then(inputs(0, 0, 0, 0, 4, 0)).then(resolve(false, false))
    .then(freeze).then(inputs(0, 0, 0, 0, 3, 0)).expect(resolved == 4 and safety)
  run fileTest = init.then(inputs(0, 0, 0, 0, 4, 0)).then(resolve(false, false))
    .then(inputs(0, 0, 0, 0, 3, 0)).then(resolve(false, false))
    .expect(resolved == 3 and safety)
}
