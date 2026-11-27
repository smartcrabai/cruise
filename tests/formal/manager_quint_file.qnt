// #57 owns the approval of the model bytes. The separate Manager proposal
// authorizes reuse only for the reviewed repo / Goal / task / proposal revision.
module manager_quint_file {
  import quint_formal_first as formal from "./quint_formal_first"
  type Diagram = { state: str, via: str }
  type State = { repo: int, goal: int, task: int, sourceRev: int, proposed: int,
    approved: int, digest: int, fileDigest: int, childDigest: int, childRepo: int,
    authorityStillValid: bool }
  var binding: State
  var handoffDiagram: Diagram
  pure val initial: State = { repo: 1, goal: 1, task: 1, sourceRev: 0, proposed: 0,
    approved: 0, digest: 0, fileDigest: 0, childDigest: 0, childRepo: 0,
    authorityStillValid: true }
  action init = all {
    formal::start(true, true), binding' = initial,
    handoffDiagram' = { state: "No approved Quint file", via: "start" },
  }
  action go(state: str, via: str, after: State): bool = all {
    handoffDiagram' = { state: state, via: via }, binding' = after,
  }
  action draft = all {
    handoffDiagram.state == "No approved Quint file", formal::beginFormal,
    go("Formal-first draft", "begin existing #57 flow", binding),
  }
  action candidate = all {
    handoffDiagram.state == "Formal-first draft", formal::draft,
    go("Unapproved candidate", "store candidate; no binding", binding),
  }
  action checked = all {
    handoffDiagram.state == "Unapproved candidate", formal::checkedAndRendered,
    go("Owner sees Quint PNG", "typecheck / test / render", binding),
  }
  action approveSource = all {
    handoffDiagram.state == "Owner sees Quint PNG", formal::answer("approve"),
    go("Digest-pinned approved Quint file", "approve shown #57 revision", { ...binding, sourceRev: formal::s.revision, digest: 7, fileDigest: 7 }),
  }
  action disableSource = all {
    Set("Digest-pinned approved Quint file", "Manager proposal reviewed", "Manager scope approved").contains(handoffDiagram.state),
    formal::requestChanged,
    go("Blocked", "source approval invalidated", { ...binding, authorityStillValid: false }),
  }
  action keepSource = formal::go(formal::diagram.state, formal::diagram.via, formal::s)
  action preview = all {
    Set("Digest-pinned approved Quint file", "Manager proposal reviewed").contains(handoffDiagram.state),
    binding.fileDigest == binding.digest, binding.proposed < 2,
    formal::s.approved == binding.sourceRev, binding.sourceRev > 0, keepSource,
    go("Manager proposal reviewed", "select owned digest and task", { ...binding, proposed: binding.proposed + 1 }),
  }
  action approveBinding(rev: int): bool = all {
    handoffDiagram.state == "Manager proposal reviewed", binding.proposed == rev,
    binding.digest == binding.fileDigest, binding.authorityStillValid, keepSource,
    go("Manager scope approved", "approve exact proposal revision", { ...binding, approved: rev }),
  }
  action changeSource = all {
    Set("Manager proposal reviewed", "Manager scope approved").contains(handoffDiagram.state), keepSource,
    go("Blocked", "file missing or digest changed", { ...binding, fileDigest: 0 }),
  }
  action materialize(repo: int, goal: int, task: int): bool = all {
    handoffDiagram.state == "Manager scope approved", binding.approved == binding.proposed, binding.approved > 0,
    repo == binding.repo, goal == binding.goal, task == binding.task, binding.digest == binding.fileDigest,
    binding.authorityStillValid, keepSource,
    go("Child gets pinned file reference", "scope and hash verified; no inline body", { ...binding, childDigest: binding.fileDigest, childRepo: repo }),
  }
  val safety = formal::invSafety and all {
    binding.approved > 0 implies binding.approved == binding.proposed,
    binding.childDigest > 0 implies (binding.childRepo == binding.repo and binding.childDigest == binding.digest
      and binding.approved > 0 and formal::s.approved == binding.sourceRev and binding.authorityStillValid),
  }
  action toSource = init.then(draft).then(candidate).then(checked)
  action toBinding = toSource.then(approveSource).then(preview)
  run exactModelReachesChildByReferenceTest = toBinding.then(approveBinding(1)).then(materialize(1, 1, 1))
    .expect(binding.childDigest == 7 and safety)
  run unapprovedSourceCannotReadTest = toSource.then(preview).fail()
  run invalidatedSourceCannotBindTest = toSource.then(approveSource).then(disableSource)
    .expect(binding.childDigest == 0 and safety).then(preview).fail()
  run staleProposalCannotBindTest = toBinding.then(preview).then(approveBinding(1)).fail()
  run anotherGoalOrTaskCannotReadTest = toBinding.then(approveBinding(1)).then(materialize(1, 2, 2)).fail()
  run anotherRepoCannotReadTest = toBinding.then(approveBinding(1)).then(materialize(2, 1, 1)).fail()
  run invalidationBetweenApprovalAndDispatchStopsChildTest = toBinding.then(approveBinding(1))
    .then(disableSource).expect(binding.childDigest == 0 and safety).then(materialize(1, 1, 1)).fail()
  run changedBytesStopChildTest = toBinding.then(approveBinding(1)).then(changeSource)
    .expect(binding.childDigest == 0 and safety).then(materialize(1, 1, 1)).fail()
}
