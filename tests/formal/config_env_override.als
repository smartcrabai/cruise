module config_env_override

abstract sig Value {}
one sig Builtin, App, User, Project, Workflow, Env, Session, Cli, Parent, Child extends Value {}
abstract sig Flag {}
one sig Yes, No extends Flag {}
one sig Input {
  app, user, project, workflow, env, session, cli: lone Value,
  retryParent, retryChild: lone Value,
  requestSnapshotA, requestSnapshotB, snapshotProcessEnv: lone Value,
  processEnv: one Value,
  requestCwdBaseA, requestCwdBaseB, daemonCwdBase: one Value,
  valid, fileSession: one Flag,
  snapshot, oldConfig: one Value
}
fun base: one Value {
  some Input.workflow => Input.workflow else
  some Input.project => Input.project else
  some Input.user => Input.user else
  some Input.app => Input.app else Builtin
}
fun effective: one Value {
  some Input.cli => Input.cli else
  some Input.session => Input.session else
  some Input.env => Input.env else base
}
// Each CLI request gets an independent workflow environment snapshot. Unlike
// a no-context WebUI request, it never falls back to the daemon workflow env.
fun requestEffectiveA: one Value {
  some Input.cli => Input.cli else
  some Input.session => Input.session else
  some Input.requestSnapshotA => Input.requestSnapshotA else Input.requestCwdBaseA
}
fun requestEffectiveB: one Value {
  some Input.cli => Input.cli else
  some Input.session => Input.session else
  some Input.requestSnapshotB => Input.requestSnapshotB else Input.requestCwdBaseB
}
fun webUiEffective: one Value {
  some Input.session => Input.session else
  some Input.env => Input.env else Input.daemonCwdBase
}
// Process-level environment names are read from the daemon process, never the
// client request snapshot.
fun requestProcessEnv: one Value { Input.processEnv }
fun reopened: one Value { Input.fileSession = Yes => effective else Input.snapshot }
fun retryValue: one Value {
  some Input.retryChild => Input.retryChild else
  some Input.retryParent => Input.retryParent else Input.oldConfig
}
// Invalid collection must leave the complete original value untouched.
fun applied: one Value { Input.valid = Yes => effective else Input.oldConfig }
// Effective app values are observed but never written back.
fun persistedApp: one Value { some Input.app => Input.app else Builtin }

assert CliWins { some Input.cli implies effective = Input.cli }
assert SessionWins { no Input.cli and some Input.session implies effective = Input.session }
assert EnvWins { no Input.cli + Input.session and some Input.env implies effective = Input.env }
assert WorkflowWins { no Input.cli + Input.session + Input.env and some Input.workflow implies effective = Input.workflow }
assert ProjectWins { no Input.cli + Input.session + Input.env + Input.workflow and some Input.project implies effective = Input.project }
assert UserWins { no Input.cli + Input.session + Input.env + Input.workflow + Input.project and some Input.user implies effective = Input.user }
assert AppWins { no Input.cli + Input.session + Input.env + Input.workflow + Input.project + Input.user and some Input.app implies effective = Input.app }
assert ChildWins { some Input.retryChild implies retryValue = Input.retryChild }
assert SnapshotStable { Input.fileSession = No implies reopened = Input.snapshot }
assert FileReresolves { Input.fileSession = Yes implies reopened = effective }
assert InvalidAtomic { Input.valid = No implies applied = Input.oldConfig }
assert NoEnvPersistence { persistedApp = (some Input.app => Input.app else Builtin) }
assert RequestSnapshotAUsed {
  no Input.cli + Input.session and some Input.requestSnapshotA
    implies requestEffectiveA = Input.requestSnapshotA
}
assert RequestSnapshotBUsed {
  no Input.cli + Input.session and some Input.requestSnapshotB
    implies requestEffectiveB = Input.requestSnapshotB
}
assert MissingRequestSnapshotIgnoresDaemonEnv {
  no Input.cli + Input.session + Input.requestSnapshotA
    implies requestEffectiveA = Input.requestCwdBaseA
}
assert RequestSnapshotsIsolated {
  no Input.cli + Input.session
    and some Input.requestSnapshotA and some Input.requestSnapshotB
    and Input.requestSnapshotA != Input.requestSnapshotB
    implies requestEffectiveA != requestEffectiveB
}
assert RequestCwdsIsolated {
  no Input.cli + Input.session + Input.requestSnapshotA + Input.requestSnapshotB
    and Input.requestCwdBaseA != Input.requestCwdBaseB
    implies requestEffectiveA != requestEffectiveB
}
assert ProcessEnvironmentNotTakenFromSnapshot {
  some Input.snapshotProcessEnv and Input.snapshotProcessEnv != Input.processEnv
    implies requestProcessEnv = Input.processEnv
}
assert WebUiUsesDaemonWorkflowEnvironment {
  no Input.session and some Input.env
    implies webUiEffective = Input.env
}
assert WebUiUsesDaemonCwd {
  no Input.session + Input.env
    implies webUiEffective = Input.daemonCwdBase
}

pred PrecedenceWitness {
  Input.cli = Cli and Input.session = Session and Input.env = Env
  and Input.workflow = Workflow and effective = Cli
  and Input.retryParent = Parent and Input.retryChild = Child and retryValue = Child
  and Input.fileSession = Yes and reopened = Cli
  and Input.snapshot = User and reopened != Input.snapshot
}
pred SnapshotWitness {
  Input.fileSession = No and Input.snapshot = User and Input.env = Env
  and no Input.cli + Input.session and reopened = User and effective = Env
}
pred RequestIsolationWitness {
  no Input.cli + Input.session
    and Input.requestSnapshotA = Cli and Input.requestSnapshotB = Child
    and Input.requestCwdBaseA = Parent and Input.requestCwdBaseB = Workflow
    and Input.env = Env
    and requestEffectiveA = Cli and requestEffectiveB = Child
    and requestEffectiveA != requestEffectiveB
}
pred RequestCwdWitness {
  no Input.cli + Input.session + Input.requestSnapshotA + Input.requestSnapshotB
    and Input.requestCwdBaseA = Parent and Input.requestCwdBaseB = Child
    and requestEffectiveA = Parent and requestEffectiveB = Child
    and requestEffectiveA != requestEffectiveB
}
pred ProcessSnapshotWitness {
  Input.snapshotProcessEnv = Child
  Input.processEnv = Parent
  requestProcessEnv = Parent
}
check CliWins for 12 expect 0
check SessionWins for 12 expect 0
check EnvWins for 12 expect 0
check WorkflowWins for 12 expect 0
check ProjectWins for 12 expect 0
check UserWins for 12 expect 0
check AppWins for 12 expect 0
check ChildWins for 12 expect 0
check SnapshotStable for 12 expect 0
check FileReresolves for 12 expect 0
check InvalidAtomic for 12 expect 0
check NoEnvPersistence for 12 expect 0
check RequestSnapshotAUsed for 12 expect 0
check RequestSnapshotBUsed for 12 expect 0
check MissingRequestSnapshotIgnoresDaemonEnv for 12 expect 0
check RequestSnapshotsIsolated for 12 expect 0
check ProcessEnvironmentNotTakenFromSnapshot for 12 expect 0
check WebUiUsesDaemonWorkflowEnvironment for 12 expect 0
check RequestCwdsIsolated for 12 expect 0
check WebUiUsesDaemonCwd for 12 expect 0
run RequestCwdWitness for 12 expect 1
run RequestIsolationWitness for 12 expect 1
run ProcessSnapshotWitness for 12 expect 1
run PrecedenceWitness for 12 expect 1
run SnapshotWitness for 12 expect 1
