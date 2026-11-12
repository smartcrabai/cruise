module base_ci_gate

// One atom is one independent invocation. Inputs abstract CLI and UI setup.
abstract sig Entry {}
one sig Cli, Ui extends Entry {}
abstract sig Forge {}
one sig Github, Gitlab, Unsupported extends Forge {}
abstract sig Result {}
one sig Passing, Pending, NoChecks, Failing, FetchError extends Result {}
abstract sig Branch {}
one sig Configured, DefaultBranch extends Branch {}
abstract sig Decision {}
one sig Skip, Allow, Warn, Reject extends Decision {}
sig Invocation {
  entry: one Entry,
  execSession, resume, afterPr, dryRun, optedOut: one Bool,
  forge: one Forge,
  configuredBranch: one Bool,
  result: one Result,
  branch: one Branch,
  decision: one Decision
}
abstract sig Bool {}
one sig Yes, No extends Bool {}

pred bypass[i: Invocation] {
  i.execSession = Yes or i.resume = Yes or i.afterPr = Yes or
  i.optedOut = Yes or i.forge = Unsupported or
  (i.entry = Cli and i.dryRun = Yes)
}
fact Policy {
  all i: Invocation | {
    i.configuredBranch = Yes implies i.branch = Configured else i.branch = DefaultBranch
    bypass[i] implies i.decision = Skip else {
      i.result = Passing implies i.decision = Allow
      i.result in Pending + NoChecks implies i.decision = Warn
      i.result in Failing + FetchError implies i.decision = Reject
    }
  }
}
// A workspace is only created after a non-rejecting decision. A skip never
// invokes the forge API. These abstractions do not model implementation code.
fun apiCalls: set Invocation { {i: Invocation | not bypass[i]} }
fun workspace: set Invocation { {i: Invocation | i.decision != Reject and i.dryRun = No} }
assert FailClosed { all i: apiCalls | i.result in Failing + FetchError implies i.decision = Reject and i not in workspace }
assert WarningContinues { all i: apiCalls | i.result in Pending + NoChecks implies i.decision = Warn }
assert BypassHasNoApi { all i: Invocation | bypass[i] implies i not in apiCalls and i.decision = Skip }
assert ConfiguredBranchWins { all i: Invocation | i.configuredBranch = Yes iff i.branch = Configured }
assert BothEntrypointsAgree { all disj i, j: Invocation | i.execSession = j.execSession and i.resume = j.resume and i.afterPr = j.afterPr and i.dryRun = No and j.dryRun = No and i.optedOut = j.optedOut and i.forge = j.forge and i.result = j.result implies i.decision = j.decision }
assert PassingContinues { all i: apiCalls | i.result = Passing implies i.decision = Allow }
pred FailureWitness { some i: Invocation | i.entry = Cli and i.forge = Github and i.result = Failing and i.decision = Reject }
pred ErrorWitness { some i: Invocation | i.entry = Ui and i.forge = Gitlab and i.result = FetchError and i.decision = Reject }
pred WarningWitness { some i: Invocation | i.forge = Gitlab and i.result = Pending and i.decision = Warn and i in workspace }
pred SkipWitness { some i: Invocation | i.resume = Yes and i.result = Failing and i.decision = Skip and i not in apiCalls }
pred BranchWitness { some i, j: Invocation | i.configuredBranch = Yes and j.configuredBranch = No and i.branch != j.branch }
check FailClosed for 4 Invocation expect 0
check WarningContinues for 4 Invocation expect 0
check BypassHasNoApi for 4 Invocation expect 0
check ConfiguredBranchWins for 4 Invocation expect 0
check BothEntrypointsAgree for 4 Invocation expect 0
check PassingContinues for 4 Invocation expect 0
run FailureWitness for 4 Invocation expect 1
run ErrorWitness for 4 Invocation expect 1
run WarningWitness for 4 Invocation expect 1
run SkipWitness for 4 Invocation expect 1
run BranchWitness for 4 Invocation expect 1
