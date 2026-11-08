// Finite abstraction: 0 means absent, automatic, or invalid; 1..7 are
// supported locales, with 1 representing English. Parsing and persistence are
// outside this model. Environment and system inputs stay fixed across steps.
module ui_locale {
  var env: int
  var config: int
  var system: int
  var effective: int
  var cookieA: int
  var cookieB: int
  var browserA: int
  var browserB: int
  pure def choose(e: int, c: int, s: int): int =
    if (e > 0) e else if (c > 0) c else if (s > 0) s else 1
  pure def browserChoice(cookie: int, process: int): int =
    if (cookie > 0) cookie else process
  action start(e: int, c: int, s: int): bool = all {
    0.to(7).contains(e), 0.to(7).contains(c), 0.to(7).contains(s),
    env' = e, config' = c, system' = s, effective' = choose(e, c, s),
    cookieA' = 0, cookieB' = 0,
    browserA' = choose(e, c, s), browserB' = choose(e, c, s),
  }
  action init = {
    nondet e = 0.to(7).oneOf()
    nondet c = 0.to(7).oneOf()
    nondet s = 0.to(7).oneOf()
    start(e, c, s)
  }
  action setConfig(c: int): bool = all {
    0.to(7).contains(c), env' = env, config' = c, system' = system,
    effective' = choose(env, c, system),
    cookieA' = cookieA, cookieB' = cookieB,
    browserA' = browserChoice(cookieA, choose(env, c, system)),
    browserB' = browserChoice(cookieB, choose(env, c, system)),
  }
  action setCookieA(c: int): bool = all {
    0.to(7).contains(c), env' = env, config' = config, system' = system,
    effective' = effective, cookieA' = c, cookieB' = cookieB,
    browserA' = browserChoice(c, effective), browserB' = browserB,
  }
  action setCookieB(c: int): bool = all {
    0.to(7).contains(c), env' = env, config' = config, system' = system,
    effective' = effective, cookieA' = cookieA, cookieB' = c,
    browserA' = browserA, browserB' = browserChoice(c, effective),
  }
  action step = {
    nondet c = 0.to(7).oneOf()
    any { setConfig(c), setCookieA(c), setCookieB(c) }
  }
  val safety = all {
    0.to(7).contains(env), 0.to(7).contains(config), 0.to(7).contains(system),
    1.to(7).contains(effective),
    env > 0 implies effective == env,
    env == 0 and config > 0 implies effective == config,
    env == 0 and config == 0 and system > 0 implies effective == system,
    env == 0 and config == 0 and system == 0 implies effective == 1,
    0.to(7).contains(cookieA), 0.to(7).contains(cookieB),
    browserA == browserChoice(cookieA, effective),
    browserB == browserChoice(cookieB, effective),
    1.to(7).contains(browserA), 1.to(7).contains(browserB),
  }
  run example = init.expect(safety).then(step).expect(safety)
  run overrideTest = start(7, 4, 3).expect(effective == 7 and safety)
    .then(setConfig(2)).expect(effective == 7 and safety)
  run configTest = start(0, 4, 3).expect(effective == 4 and safety)
    .then(setConfig(2)).expect(effective == 2 and safety)
  run systemTest = start(0, 4, 3).then(setConfig(0))
    .expect(effective == 3 and safety)
  run defaultTest = start(0, 4, 0).then(setConfig(0))
    .expect(effective == 1 and safety)
  run browserIsolationTest = start(7, 4, 3).then(setCookieA(2))
    .expect(effective == 7 and browserA == 2 and browserB == 7 and safety)
    .then(setCookieB(3))
    .expect(effective == 7 and browserA == 2 and browserB == 3 and safety)
    .then(setCookieA(0))
    .expect(browserA == 7 and browserB == 3 and safety)
  run browserFallbackTest = start(0, 4, 3).then(setCookieA(2))
    .then(setConfig(5))
    .expect(browserA == 2 and browserB == 5 and effective == 5 and safety)
    .then(setCookieA(0)).expect(browserA == 5 and browserB == 5 and safety)
}
