// Abstract classification of well-formed Alloy command receipts in
// src/formal_verify/alloy.rs. Process failures and malformed receipts are not
// represented here. All checks are bounded, not unbounded proofs.
module plan_verify

abstract sig Kind {}
one sig Check, Run extends Kind {}
abstract sig Outcome {}
one sig Sat, Unsat extends Outcome {}
abstract sig Verdict {}
one sig Passed, Failed extends Verdict {}
sig Receipt { kind: one Kind, outcome: one Outcome, verdict: one Verdict }

// A check passes only without a counterexample. A run passes only when its
// requested witness exists. The receipt parser establishes the outcome first.
pred classify[r: Receipt] {
  r.verdict = (r.kind = Check implies
    (r.outcome = Unsat implies Passed else Failed) else
    (r.outcome = Sat implies Passed else Failed))
}
fact Classification { all r: Receipt | classify[r] }

assert CheckPassIffUnsat {
  all r: Receipt | r.kind = Check implies (r.verdict = Passed iff r.outcome = Unsat)
}
assert RunPassIffSat {
  all r: Receipt | r.kind = Run implies (r.verdict = Passed iff r.outcome = Sat)
}
assert CounterexampleAndMissingWitnessFail {
  all r: Receipt | (r.kind = Check and r.outcome = Sat or
                    r.kind = Run and r.outcome = Unsat) implies r.verdict = Failed
}

// Exercise both outcomes of both command kinds in the same instance, so the
// assertions cannot pass merely because no receipts or command kinds exist.
pred ConsistencyRun {
  some r: Receipt | r.kind = Check and r.outcome = Unsat and r.verdict = Passed
  some r: Receipt | r.kind = Check and r.outcome = Sat and r.verdict = Failed
  some r: Receipt | r.kind = Run and r.outcome = Sat and r.verdict = Passed
  some r: Receipt | r.kind = Run and r.outcome = Unsat and r.verdict = Failed
}
check CheckPassIffUnsat for 4 expect 0
check RunPassIffSat for 4 expect 0
check CounterexampleAndMissingWitnessFail for 4 expect 0
run ConsistencyRun for 4 expect 1
