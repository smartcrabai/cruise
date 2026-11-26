// Counter in src/counter.sh: increments until the limit, then refuses.
module counter {
  var n: int
  var diagram: { state: str, via: str }
  pure val limit = 3
  action init = all { n' = 0, diagram' = { state: "Zero", via: "start" } }
  action inc = all {
    n' = if (n < limit) n + 1 else n,
    diagram' = { state: if (n >= limit) "Refused at limit" else if (n + 1 == limit) "At limit" else "Counting", via: "increment" },
  }
  action step = inc
  val safety = n <= limit
  run incTest = init.then(inc).expect(n == 1 and safety)
  run limitTest = init.then(inc).then(inc).then(inc).expect(n == limit and safety)
  run refuseTest = init.then(inc).then(inc).then(inc).then(inc).expect(n == limit and safety)
}
