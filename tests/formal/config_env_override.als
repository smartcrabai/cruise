module config_env_override

abstract sig Value {}
one sig Builtin, App, User, Project, Workflow, Env, Session, Cli, Parent, Child extends Value {}
abstract sig Flag {}
one sig Yes, No extends Flag {}
one sig Input {
  app, user, project, workflow, env, session, cli: lone Value,
  retryParent, retryChild: lone Value,
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
run PrecedenceWitness for 12 expect 1
run SnapshotWitness for 12 expect 1
