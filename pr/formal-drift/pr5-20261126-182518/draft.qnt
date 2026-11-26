module model {
  var limit: int
  var diagram: { state: str, via: str }
  action init = all { limit' = 3, diagram' = { state: "Limit 3", via: "start" } }
  action raise = all { limit == 3, limit' = 5, diagram' = { state: "Limit 5", via: "raise limit" } }
  action step = raise
  val safety = limit <= 5
  run raiseTest = init.then(raise).expect(limit == 5 and safety)
}
