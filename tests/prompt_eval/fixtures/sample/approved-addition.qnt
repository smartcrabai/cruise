module model {
  var result: Set[int]
  var diagram: { state: str, via: str }
  action init = all { result' = Set(), diagram' = { state: "Ready", via: "start" } }
  action add(a: int, b: int): bool = {
    val sum = a + b
    val valid = -2147483648 <= sum and sum <= 2147483647
    all {
      result' = if (valid) Set(sum) else Set(),
      diagram' = if (valid) { state: "Value", via: "add" }
                 else { state: "Overflow", via: "reject out of range" }
    }
  }
  action step = any { add(2, 3), add(2147483647, 1) }
  val invRange = result.forall(x => -2147483648 <= x and x <= 2147483647)
  run addsTest = init.then(add(2, 3)).expect(result == Set(5) and invRange)
  run rejectsOverflowTest = init.then(add(2147483647, 1))
    .expect(result == Set() and diagram.state == "Overflow" and invRange)
}
