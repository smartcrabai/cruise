// Bounded publication recovery lifecycle, matching pr_failed.qnt.
module pr_failed
open util/ordering[State]
abstract sig Phase {}
one sig Planned, Running, Failed, PrFailed, Completed extends Phase {}
abstract sig Bool {}
one sig Yes, No extends Bool {}
abstract sig Action {}
one sig Init, Workflow, WorkflowError, Publish, Skip, PublishError,
  Retry, RetrySkip, RetryError, Cancel, Reset, Delete, Cleanup extends Action {}
sig State {
  phase: one Phase, url, tree, deleted, skipped: one Bool,
  prs, runs: one Int, action: one Action
}
pred init[s: State] {
  s.phase = Planned and s.url = No and s.tree = Yes and s.deleted = No
  and s.skipped = No and s.prs = 0 and s.runs = 0 and s.action = Init
}
pred transition[a,b: State] {
  a.deleted = No
  b.action != Init
  b.runs = (b.action = Workflow => plus[a.runs, 1] else a.runs)
  b.deleted = (b.action = Delete => Yes else a.deleted)
  b.tree = (b.action in Delete + Cleanup => No else a.tree)
  b.url in (b.action in Publish + Retry => Yes else
    (b.action = Reset => No else
      (b.action in PublishError + RetryError => a.url + Yes else a.url)))
  b.prs in (b.action in Publish + Retry => 1 else
    (b.action in PublishError + RetryError => a.prs + 1 else a.prs))
  b.skipped = (b.action in Skip + RetrySkip => Yes else
    (b.action in Publish + Retry + Reset => No else a.skipped))
  (b.action = Workflow implies a.phase in Planned + Failed and a.tree = Yes and b.phase = Running)
  (b.action = WorkflowError implies a.phase = Running and b.phase = Failed)
  (b.action in Publish + Skip implies a.phase = Running and b.phase = Completed)
  (b.action = PublishError implies a.phase = Running and b.phase = PrFailed)
  (b.action in Retry + RetrySkip implies a.phase = PrFailed and b.phase = Completed)
  (b.action in RetryError + Cancel implies a.phase = PrFailed and b.phase = PrFailed)
  (b.action in Reset + Delete implies a.phase = PrFailed and b.phase = Planned)
  (b.action = Cleanup implies a.phase = Completed and a.url = Yes and a.tree = Yes and b.phase = Completed)
  // A newly created PR updates its URL and count together, while an
  // already existing PR remains after reset even when the URL is cleared.
  (b.action in PublishError + RetryError implies
    ((b.url = a.url and b.prs = a.prs) or (b.url = Yes and b.prs = 1)))
}
pred idle[a,b: State] {
  b.phase = a.phase and b.url = a.url and b.tree = a.tree and b.deleted = a.deleted
  and b.skipped = a.skipped and b.prs = a.prs and b.runs = a.runs and b.action = a.action
}
fact Trace { init[first] all a: State - last | transition[a,next[a]] or idle[a,next[a]] }
assert PrFailedNotRunnable { all a: State | a.phase = PrFailed implies a.phase not in Planned + Running + Failed }
assert RetryDoesNotRunWorkflow { all a: State - first | a.action in Retry + RetrySkip + RetryError + Cancel implies a.runs = prev[a].runs }
assert CancelKeepsPrFailed { all a: State | a.action = Cancel implies a.phase = PrFailed }
assert UrlImpliesPr { all a: State | a.url = Yes implies a.prs = 1 }
assert AtMostOnePr { all a: State | a.prs >= 0 and a.prs <= 1 }
assert WorktreeRetained { all a: State | a.phase = PrFailed implies a.tree = Yes }
assert CompletedHasPrOrSkip { all a: State | a.phase = Completed implies a.url = Yes or a.skipped = Yes }
check PrFailedNotRunnable for 15 but exactly 9 State, 5 Int expect 0
check RetryDoesNotRunWorkflow for 15 but exactly 9 State, 5 Int expect 0
check CancelKeepsPrFailed for 15 but exactly 9 State, 5 Int expect 0
check UrlImpliesPr for 15 but exactly 9 State, 5 Int expect 0
check AtMostOnePr for 15 but exactly 9 State, 5 Int expect 0
check WorktreeRetained for 15 but exactly 9 State, 5 Int expect 0
check CompletedHasPrOrSkip for 15 but exactly 9 State, 5 Int expect 0
pred RetryWitness { some a: State | a.action = Retry and some b: prevs[a] | b.action = PublishError }
pred CancelWitness { some a: State | a.action = Cancel }
pred SkipWitness { some a: State | a.action = RetrySkip }
pred PersistWitness { some a: State | a.action = RetryError and a.url = Yes and some b: nexts[a] | b.action = Retry }
run RetryWitness for 15 but exactly 9 State, 5 Int expect 1
run CancelWitness for 15 but exactly 9 State, 5 Int expect 1
run SkipWitness for 15 but exactly 9 State, 5 Int expect 1
run PersistWitness for 15 but exactly 9 State, 5 Int expect 1
