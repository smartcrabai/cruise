// Bounded host Git handoff. Trees, heads, evidence, and the remote PR are
// separate so stale workspace/evidence races remain observable.
module after_pr_host_git {
  type State = {
    phase: int, remoteHead: int, remoteTree: int,
    workspaceHead: int, workspaceParentHead: int, workspaceTree: int,
    candidateHead: int, candidateTree: int,
    evidenceHead: int, evidenceTree: int, evidenceStatus: int,
    evidenceNoEgress: bool, evidenceIndexClean: bool,
    bindingHead: int, bindingTrusted: bool,
    initialConflictPaths: int, committedPaths: int, strayUntracked: bool,
    publications: int, blockedAt: int,
    ciGreen: bool, retriedHeads: Set[int], retryHead: int, retryOutcome: int,
    failureKind: int, unacquiredReruns: int -> int,
    readinessHead: int, readinessAsserted: bool, readinessReadOnly: bool,
    readinessBehind: bool, readinessConflict: bool,
    nativeMerged: bool, waiting: bool
  }
  var s: State
  var before: State
  var event: str

  // Phases: 0 unbound, 1 bound, 2 conflict, 3 conflict committed,
  // 4 wait CI, 5 CI failed, 6 candidate, 7 verified, 8 merge ready,
  // 9 retry pending, 10 terminal, 11 merged. Evidence: 0 none, 1 verified,
  // 2 unresolved/failed. Each head may consume one retry, reserved pre-request.
  // Retry outcomes: 0 success, 1 failed, 2 unknown, 3 wait.
  // Failure kinds: 0 genuine, 1 only jobs no runner acquired, 2 both.
  // The host reruns kind 1 at most MAX_RERUNS times per head, counted
  // pre-request in session state that resuming keeps.
  pure val MAX_RERUNS = 2
  pure val initial: State = {
    phase: 0, remoteHead: 0, remoteTree: 0,
    workspaceHead: 0, workspaceParentHead: 0, workspaceTree: 0,
    candidateHead: -1, candidateTree: -1,
    evidenceHead: -1, evidenceTree: -1, evidenceStatus: 0,
    evidenceNoEgress: false, evidenceIndexClean: false,
    bindingHead: -1, bindingTrusted: false,
    initialConflictPaths: 0, committedPaths: 0, strayUntracked: false,
    publications: 0, blockedAt: 0,
    ciGreen: false, retriedHeads: Set(), retryHead: -1, retryOutcome: -1,
    failureKind: -1, unacquiredReruns: Map(0 -> 0, 1 -> 0, 2 -> 0),
    readinessHead: -1, readinessAsserted: false, readinessReadOnly: false,
    readinessBehind: false, readinessConflict: false,
    nativeMerged: false, waiting: false
  }
  action init = all { s' = initial, before' = initial, event' = "init" }
  action record(updated: State, name: str): bool = all {
    before' = s, s' = updated, event' = name }

  // The observed PR head must match the remote before the binding is trusted.
  action bindCurrent(observedHead: int): bool = all {
    (s.phase == 0 or s.phase == 4), observedHead == s.remoteHead,
    record({ ...s, phase: 1, bindingHead: observedHead,
      bindingTrusted: true, readinessAsserted: false,
      readinessReadOnly: false, readinessHead: -1,
      readinessBehind: false, readinessConflict: false }, "bind") }
  action syncWorkspace = all {
    s.phase == 1, s.bindingTrusted, s.bindingHead == s.remoteHead,
    record({ ...s, workspaceHead: s.remoteHead,
      workspaceParentHead: s.remoteHead, workspaceTree: s.remoteTree }, "sync") }
  action startWaitCI = all {
    s.phase == 1, s.bindingTrusted, s.bindingHead == s.remoteHead,
    s.workspaceHead == s.remoteHead, s.workspaceTree == s.remoteTree,
    record({ ...s, phase: 4, ciGreen: false }, "waitCI") }
  action ciPass = all {
    s.phase == 4, s.bindingTrusted, s.bindingHead == s.remoteHead,
    s.workspaceHead == s.remoteHead, s.workspaceTree == s.remoteTree,
    record({ ...s, ciGreen: true }, "ciPass") }
  action ciFail(kind: int): bool = all {
    s.phase == 4, s.bindingTrusted, s.bindingHead == s.remoteHead,
    Set(0, 1, 2).contains(kind),
    record({ ...s, phase: 5, ciGreen: false, failureKind: kind }, "ciFail") }

  // Only jobs no runner acquired failed, and the head has reruns left.
  def rerunDue(st: State): bool = all {
    st.phase == 5, st.failureKind == 1,
    st.unacquiredReruns.get(st.remoteHead) < MAX_RERUNS }
  // The host reruns such a head instead of asking the repair agent.
  action rerunUnacquired = all {
    rerunDue(s), s.bindingTrusted, s.bindingHead == s.remoteHead,
    record({ ...s, phase: 4, ciGreen: false,
      unacquiredReruns: s.unacquiredReruns.setBy(s.remoteHead, n => n + 1) },
      "rerunUnacquired") }

  // Conflict handling starts only from a trusted, unchanged PR binding.
  action observeConflict(paths: int): bool = all {
    s.phase == 1, s.bindingTrusted, s.bindingHead == s.remoteHead,
    s.workspaceHead == s.remoteHead, s.workspaceTree == s.remoteTree,
    Set(1, 2, 3).contains(paths),
    record({ ...s, phase: 2, initialConflictPaths: paths,
      committedPaths: 0, strayUntracked: false }, "conflict") }
  // Resolver verification may create, rewrite, or prune ignored caches. The
  // host stages only conflict paths, so this never reaches the commit.
  action resolverIgnoredChurn = all {
    s.phase == 2, record(s, "ignoredChurn") }
  // A new or removed untracked, non-ignored path outside the conflict.
  action resolverStrayUntracked = all {
    s.phase == 2, record({ ...s, strayUntracked: true }, "strayUntracked") }
  action commitConflict(paths: int, tree: int): bool = all {
    s.phase == 2, s.bindingTrusted, s.bindingHead == s.remoteHead,
    s.workspaceHead == s.remoteHead, s.remoteHead < 2, not(s.strayUntracked),
    paths == s.initialConflictPaths, Set(0, 1, 2, 3).contains(tree),
    record({ ...s, phase: 3, workspaceParentHead: s.remoteHead,
      workspaceHead: s.remoteHead + 1, workspaceTree: tree,
      committedPaths: paths }, "conflictCommit") }
  // This models a scoped branch push of precisely the initially conflicted paths.
  action pushConflict = all {
    s.phase == 3, s.bindingTrusted, s.bindingHead == s.remoteHead,
    s.workspaceParentHead == s.remoteHead, s.workspaceHead == s.remoteHead + 1,
    s.committedPaths == s.initialConflictPaths,
    record({ ...s, phase: 4, remoteHead: s.workspaceHead,
      remoteTree: s.workspaceTree, publications: s.publications + 1,
      bindingTrusted: false, ciGreen: false }, "conflictPush") }

  // A model suggestion changes the workspace tree only; it is not publishable.
  action suggestRepair(tree: int): bool = all {
    s.phase == 5, not(rerunDue(s)), s.bindingTrusted, s.bindingHead == s.remoteHead,
    s.workspaceHead == s.remoteHead, s.workspaceParentHead == s.remoteHead,
    s.workspaceTree == s.remoteTree,
    Set(0, 1, 2, 3).contains(tree),
    record({ ...s, phase: 6, candidateHead: s.remoteHead,
      candidateTree: tree, workspaceTree: tree,
      evidenceHead: -1, evidenceTree: -1, evidenceStatus: 0,
      evidenceNoEgress: false, evidenceIndexClean: false }, "suggest") }
  // Verification attests the exact candidate tree and its source head.
  action verifyCandidate(atHead: int, tree: int, noEgress: bool,
      indexClean: bool): bool = all {
    s.phase == 6, s.bindingTrusted, s.bindingHead == s.remoteHead,
    s.workspaceHead == s.remoteHead, s.workspaceParentHead == s.remoteHead,
    s.candidateHead == s.remoteHead,
    atHead == s.candidateHead, tree == s.candidateTree,
    s.workspaceTree == s.candidateTree, noEgress, indexClean,
    record({ ...s, phase: 7, evidenceHead: atHead, evidenceTree: tree,
      evidenceStatus: 1, evidenceNoEgress: noEgress,
      evidenceIndexClean: indexClean }, "verify") }
  action failedVerification(atHead: int, tree: int, noEgress: bool,
      indexClean: bool): bool = all {
    s.phase == 6,
    record({ ...s, phase: 10, evidenceHead: atHead, evidenceTree: tree,
      evidenceStatus: 2, evidenceNoEgress: noEgress,
      evidenceIndexClean: indexClean, blockedAt: s.publications }, "verifyFailed") }
  action unresolvedEvidence = all {
    s.phase == 6,
    record({ ...s, phase: 10, evidenceStatus: 2,
      blockedAt: s.publications }, "unresolvedEvidence") }

  // The successful scoped push advances both local and remote heads, then
  // returns to CI waiting on that new head; old evidence stays visibly stale.
  action pushRepair = all {
    s.phase == 7, s.remoteHead < 2, s.bindingTrusted,
    s.bindingHead == s.remoteHead, s.candidateHead == s.remoteHead,
    s.workspaceHead == s.remoteHead, s.workspaceParentHead == s.remoteHead,
    s.workspaceTree == s.candidateTree,
    s.evidenceStatus == 1, s.evidenceHead == s.candidateHead,
    s.evidenceTree == s.candidateTree, s.evidenceNoEgress,
    s.evidenceIndexClean,
    record({ ...s, phase: 4, workspaceParentHead: s.remoteHead,
      workspaceHead: s.remoteHead + 1, remoteHead: s.remoteHead + 1,
      remoteTree: s.workspaceTree, publications: s.publications + 1,
      bindingTrusted: false, ciGreen: false,
      readinessAsserted: false, readinessReadOnly: false,
      readinessHead: -1, readinessBehind: false, readinessConflict: false },
      "repairPush") }

  // A workspace mutation or a newer PR head invalidates the current proof.
  action workspaceDrift(tree: int): bool = all {
    s.phase == 7, Set(0, 1, 2, 3).contains(tree),
    tree != s.workspaceTree,
    record({ ...s, phase: 4, workspaceTree: tree,
      readinessAsserted: false, readinessReadOnly: false,
      readinessHead: -1, readinessBehind: false, readinessConflict: false },
      "workspaceDrift") }
  action advanceRemote(newHead: int, tree: int): bool = all {
    Set(1, 2, 3, 4, 5, 6, 7, 8, 9).contains(s.phase),
    newHead > s.remoteHead, newHead <= 2, Set(0, 1, 2, 3).contains(tree),
    record({ ...s, phase: 4, remoteHead: newHead, remoteTree: tree,
      bindingTrusted: false, ciGreen: false,
      readinessAsserted: false, readinessReadOnly: false,
      readinessHead: -1, readinessBehind: false, readinessConflict: false },
      "remoteAdvance") }

  // The per-head retry marker is recorded before the request is made.
  action reserveRetry = all {
    s.phase == 5, not(rerunDue(s)), s.bindingTrusted, s.bindingHead == s.remoteHead,
    not(s.retriedHeads.contains(s.remoteHead)),
    record({ ...s, phase: 9,
      retriedHeads: s.retriedHeads.union(Set(s.remoteHead)),
      retryHead: s.remoteHead, retryOutcome: -1 }, "reserveRetry") }
  action retryResult(outcome: int): bool = all {
    s.phase == 9, s.retriedHeads.contains(s.retryHead),
    s.retryHead == s.remoteHead, s.bindingTrusted,
    s.bindingHead == s.remoteHead, Set(0, 1, 2, 3).contains(outcome),
    record({ ...s, phase: if (outcome == 0) 4 else 10,
      ciGreen: false, retryOutcome: outcome,
      blockedAt: if (outcome == 0) s.blockedAt else s.publications }, "retryResult") }

  // A `wait` decision stops the run without merging. Resuming it does not
  // replay the decision: it returns to CI waiting and must rebind the head.
  action waitDecision = all {
    s.phase == 5, not(rerunDue(s)), s.bindingTrusted, s.bindingHead == s.remoteHead,
    record({ ...s, phase: 10, waiting: true, blockedAt: s.publications },
      "waitDecision") }
  action resumeWait = all {
    s.phase == 10, s.waiting,
    record({ ...s, phase: 4, waiting: false, bindingTrusted: false,
      ciGreen: false, readinessAsserted: false, readinessReadOnly: false,
      readinessHead: -1, readinessBehind: false, readinessConflict: false },
      "resumeWait") }

  // Readiness is an explicit read-only assertion, independently tied to the
  // current remote head. Any negative read routes back to wait rather than merge.
  // Native merge follows an asserted, read-only check of the current head.
  action assertReadiness(observedHead: int, behind: bool,
      conflict: bool): bool = all {
    s.phase == 4, s.ciGreen, s.bindingTrusted,
    s.bindingHead == s.remoteHead, observedHead == s.remoteHead,
    s.workspaceHead == s.remoteHead, s.workspaceTree == s.remoteTree,
    not(behind), not(conflict),
    record({ ...s, phase: 8, readinessHead: observedHead,
      readinessAsserted: true, readinessReadOnly: true,
      readinessBehind: false, readinessConflict: false }, "readiness") }
  action rejectReadiness(observedHead: int, readOnly: bool,
      behind: bool, conflict: bool): bool = all {
    s.phase == 4, s.ciGreen, s.bindingTrusted,
    s.bindingHead == s.remoteHead,
    not(readOnly and observedHead == s.remoteHead and not(behind) and not(conflict)),
    record({ ...s, readinessHead: observedHead,
      readinessAsserted: false, readinessReadOnly: false,
      readinessBehind: behind, readinessConflict: conflict }, "readinessRejected") }
  action nativeMerge = all {
    s.phase == 8, s.readinessAsserted, s.readinessReadOnly,
    not(s.readinessBehind), not(s.readinessConflict),
    s.readinessHead == s.remoteHead, s.bindingTrusted,
    s.bindingHead == s.remoteHead,
    record({ ...s, phase: 11, nativeMerged: true }, "nativeMerge") }

  action hardError = all {
    not(Set(10, 11).contains(s.phase)),
    record({ ...s, phase: 10, blockedAt: s.publications }, "hardError") }
  action step = {
    nondet nextHead = Set(1, 2).oneOf()
    nondet tree = Set(0, 1, 2, 3).oneOf()
    nondet paths = Set(1, 2, 3).oneOf()
    nondet observed = Set(0, 1, 2).oneOf()
    nondet outcome = Set(0, 1, 2, 3).oneOf()
    nondet flag = Set(true, false).oneOf()
    nondet kind = Set(0, 1, 2).oneOf()
    any {
      bindCurrent(observed), syncWorkspace, startWaitCI, ciPass, ciFail(kind),
      rerunUnacquired,
      observeConflict(paths), resolverIgnoredChurn, resolverStrayUntracked,
      commitConflict(paths, tree), pushConflict,
      suggestRepair(tree), verifyCandidate(observed, tree, flag, flag),
      failedVerification(observed, tree, flag, flag), unresolvedEvidence,
      pushRepair, workspaceDrift(tree), advanceRemote(nextHead, tree),
      reserveRetry, retryResult(outcome), waitDecision, resumeWait,
      assertReadiness(observed, flag, flag),
      rejectReadiness(observed, flag, flag, flag), nativeMerge, hardError
    }
  }

  val safety = all {
    Set(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11).contains(s.phase),
    0.to(2).contains(s.remoteHead), 0.to(2).contains(s.workspaceHead),
    0.to(2).contains(s.workspaceParentHead),
    0.to(3).contains(s.remoteTree), 0.to(3).contains(s.workspaceTree),
    Set(-1, 0, 1, 2).contains(s.candidateHead),
    Set(-1, 0, 1, 2, 3).contains(s.candidateTree),
    Set(-1, 0, 1, 2).contains(s.evidenceHead),
    Set(-1, 0, 1, 2, 3).contains(s.evidenceTree),
    Set(0, 1, 2).contains(s.evidenceStatus),
    Set(-1, 0, 1, 2).contains(s.bindingHead),
    Set(0, 1, 2, 3).contains(s.initialConflictPaths),
    Set(0, 1, 2, 3).contains(s.committedPaths),
    0.to(2).contains(s.publications), 0.to(2).contains(s.blockedAt),
    s.retriedHeads == s.retriedHeads.intersect(Set(0, 1, 2)),
    Set(-1, 0, 1, 2, 3).contains(s.retryOutcome),
    Set(-1, 0, 1, 2).contains(s.readinessHead),
    s.phase == 10 implies s.publications == s.blockedAt,
    s.waiting implies s.phase == 10,
    s.evidenceStatus == 2 implies s.phase == 10,
    s.evidenceStatus == 1 implies (s.evidenceHead == s.candidateHead
      and s.evidenceTree == s.candidateTree and s.evidenceNoEgress
      and s.evidenceIndexClean),
    (s.phase == 2 or s.phase == 3) implies (s.bindingTrusted
      and s.bindingHead == s.remoteHead),
    s.phase == 2 implies (s.workspaceHead == s.remoteHead
      and s.workspaceTree == s.remoteTree),
    s.phase == 3 implies (s.workspaceParentHead == s.remoteHead
      and s.workspaceHead == s.remoteHead + 1),
    s.phase == 8 implies (s.readinessAsserted and s.readinessReadOnly
      and not(s.readinessBehind) and not(s.readinessConflict)
      and s.readinessHead == s.remoteHead and s.bindingTrusted
      and s.bindingHead == s.remoteHead),
    s.nativeMerged implies (s.phase == 11 and s.readinessAsserted
      and s.readinessReadOnly and not(s.readinessBehind)
      and not(s.readinessConflict) and s.readinessHead == s.remoteHead),
    before.retriedHeads.intersect(s.retriedHeads) == before.retriedHeads,
    // Never more reruns than the limit; resuming or a new head never recounts.
    0.to(2).forall(h => s.unacquiredReruns.get(h) <= MAX_RERUNS
      and before.unacquiredReruns.get(h) <= s.unacquiredReruns.get(h)),
    // A rerun never hides a genuine failure: only a failure made solely of
    // jobs no runner acquired is rerun, exactly once per counted attempt.
    event == "rerunUnacquired" implies (before.phase == 5
      and before.failureKind == 1 and s.phase == 4 and not(s.ciGreen)
      and s.unacquiredReruns.get(before.remoteHead)
        == before.unacquiredReruns.get(before.remoteHead) + 1
      and s.unacquiredReruns.get(before.remoteHead) <= MAX_RERUNS
      and s.remoteHead == before.remoteHead
      and s.publications == before.publications),
    (event == "suggest" or event == "reserveRetry" or event == "waitDecision")
      implies not(rerunDue(before)),
    s.phase == 9 implies (s.retriedHeads.contains(s.retryHead)
      and s.retryHead == s.remoteHead and s.bindingTrusted
      and s.bindingHead == s.remoteHead),
    event == "conflict" implies (before.phase == 1 and before.bindingTrusted
      and before.bindingHead == before.remoteHead
      and before.workspaceHead == before.remoteHead
      and s.initialConflictPaths != 0),
    event == "conflictCommit" implies (before.phase == 2
      and before.bindingTrusted and before.bindingHead == before.remoteHead
      and not(before.strayUntracked)
      and s.committedPaths == before.initialConflictPaths
      and s.workspaceParentHead == before.remoteHead
      and s.workspaceHead == before.remoteHead + 1),
    event == "conflictPush" implies (before.phase == 3
      and before.bindingTrusted and before.bindingHead == before.remoteHead
      and before.committedPaths == before.initialConflictPaths
      and s.remoteHead == before.workspaceHead
      and s.publications == before.publications + 1 and s.phase == 4),
    event == "verify" implies (before.phase == 6 and s.phase == 7
      and s.evidenceHead == before.candidateHead
      and s.evidenceTree == before.candidateTree and s.evidenceNoEgress
      and s.evidenceIndexClean),
    event == "verifyFailed" or event == "unresolvedEvidence"
      implies (s.phase == 10 and s.publications == before.publications),
    event == "repairPush" implies (before.phase == 7
      and before.evidenceStatus == 1
      and before.evidenceHead == before.candidateHead
      and before.evidenceTree == before.candidateTree
      and before.workspaceTree == before.candidateTree
      and before.workspaceHead == before.remoteHead
      and before.bindingHead == before.remoteHead
      and s.remoteHead == before.remoteHead + 1
      and s.remoteTree == before.workspaceTree and s.phase == 4
      and s.publications == before.publications + 1),
    event == "remoteAdvance" implies (s.phase == 4
      and s.remoteHead > before.remoteHead and not(s.bindingTrusted)
      and not(s.readinessAsserted) and s.workspaceHead == before.workspaceHead
      and s.evidenceHead == before.evidenceHead
      and s.evidenceTree == before.evidenceTree
      and s.publications == before.publications),
    event == "workspaceDrift" implies (before.phase == 7 and s.phase == 4
      and s.workspaceTree != before.workspaceTree
      and s.evidenceTree == before.evidenceTree
      and s.publications == before.publications),
    event == "reserveRetry" implies (before.phase == 5
      and not(before.retriedHeads.contains(before.remoteHead))
      and s.retriedHeads == before.retriedHeads.union(Set(before.remoteHead))
      and s.retryHead == before.remoteHead and s.bindingTrusted
      and s.bindingHead == before.remoteHead),
    event == "retryResult" implies (before.phase == 9
      and before.retriedHeads.contains(before.retryHead)
      and before.retryHead == before.remoteHead
      and 0.to(3).contains(s.retryOutcome)
      and (if (s.retryOutcome == 0)
        s.phase == 4 and s.remoteHead == before.remoteHead
          and s.candidateHead == before.candidateHead
          and s.workspaceTree == before.workspaceTree
          and s.publications == before.publications
        else s.phase == 10 and s.publications == before.publications)),
    event == "readiness" implies (before.phase == 4 and before.ciGreen
      and s.readinessAsserted and s.readinessReadOnly
      and not(s.readinessBehind) and not(s.readinessConflict)
      and s.readinessHead == before.remoteHead),
    event == "nativeMerge" implies (before.phase == 8
      and before.readinessHead == before.remoteHead
      and not(before.readinessBehind) and not(before.readinessConflict)
      and s.nativeMerged),
    event == "hardError" implies (s.phase == 10
      and s.blockedAt == before.publications),
    event == "waitDecision" implies (before.phase == 5 and s.phase == 10
      and s.publications == before.publications and not(s.nativeMerged)),
    event == "resumeWait" implies (before.phase == 10 and before.waiting
      and s.phase == 4 and not(s.bindingTrusted) and not(s.ciGreen)
      and s.publications == before.publications)
  }

  run hardErrorStopsFurtherPublicationTest = init.then(bindCurrent(0))
    .then(observeConflict(1)).then(commitConflict(1, 1)).then(pushConflict)
    .then(bindCurrent(1)).then(startWaitCI).then(hardError)
    .expect(s.phase == 10 and s.publications == 1 and s.blockedAt == 1 and safety)
  run unresolvedEvidenceStopsFurtherPublicationTest = init.then(bindCurrent(0))
    .then(observeConflict(1)).then(commitConflict(1, 1)).then(pushConflict)
    .then(bindCurrent(1)).then(syncWorkspace).then(startWaitCI)
    .then(ciFail(0)).then(suggestRepair(2))
    .then(unresolvedEvidence)
    .expect(s.phase == 10 and s.evidenceStatus == 2
      and s.publications == 1 and s.blockedAt == 1 and safety)
  run failedVerificationStopsFurtherPublicationTest = init.then(bindCurrent(0))
    .then(observeConflict(1)).then(commitConflict(1, 1)).then(pushConflict)
    .then(bindCurrent(1)).then(syncWorkspace).then(startWaitCI)
    .then(ciFail(0)).then(suggestRepair(2))
    .then(failedVerification(1, 3, false, true))
    .expect(s.phase == 10 and s.evidenceStatus == 2
      and s.publications == 1 and s.blockedAt == 1 and safety)
  run conflictUsesTrustedBindingAndScopedPushTest = init.then(bindCurrent(0))
    .then(observeConflict(1)).then(commitConflict(1, 1)).then(pushConflict)
    .expect(s.phase == 4 and s.remoteHead == 1 and s.publications == 1
      and s.committedPaths == s.initialConflictPaths and safety)
  run ignoredCacheChurnStillPublishesConflictTest = init.then(bindCurrent(0))
    .then(observeConflict(1)).then(resolverIgnoredChurn)
    .then(commitConflict(1, 1)).then(pushConflict)
    .expect(s.phase == 4 and s.publications == 1 and safety)
  run strayUntrackedPathBlocksConflictCommitTest = init.then(bindCurrent(0))
    .then(observeConflict(1)).then(resolverIgnoredChurn)
    .then(resolverStrayUntracked).then(commitConflict(1, 1)).fail()
  run freshConflictAfterPublicationUsesNewBindingTest = init.then(bindCurrent(0))
    .then(observeConflict(1)).then(commitConflict(1, 1)).then(pushConflict)
    .then(bindCurrent(1)).then(observeConflict(2)).then(commitConflict(2, 3)).then(pushConflict)
    .expect(s.remoteHead == 2 and s.remoteTree == 3 and s.publications == 2 and safety)
  run supersededCandidateReturnsToWaitTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(0)).then(suggestRepair(1))
    .then(verifyCandidate(0, 1, true, true)).then(advanceRemote(1, 2))
    .expect(s.phase == 4 and s.remoteHead == 1 and s.candidateHead == 0
      and s.evidenceHead == 0 and s.publications == 0
      and s.retriedHeads == Set() and safety)
  run workspaceRaceReturnsToWaitTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(0)).then(suggestRepair(1))
    .then(verifyCandidate(0, 1, true, true)).then(workspaceDrift(2))
    .expect(s.phase == 4 and s.workspaceTree == 2 and s.evidenceTree == 1
      and s.publications == 0 and s.retriedHeads == Set() and safety)
  run verifiedRepairPushLoopsToNewHeadTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(0)).then(suggestRepair(1))
    .then(verifyCandidate(0, 1, true, true)).then(pushRepair)
    .expect(s.phase == 4 and s.remoteHead == 1 and s.workspaceHead == 1
      and not(s.bindingTrusted) and s.publications == 1 and safety)
  run failedRetryIsTerminalTest = init.then(bindCurrent(0)).then(startWaitCI)
    .then(ciFail(0)).then(reserveRetry).then(retryResult(1))
    .expect(s.phase == 10 and s.retriedHeads.contains(0)
      and s.retryOutcome == 1 and s.publications == 0 and safety)
  run ambiguousRetryIsTerminalTest = init.then(bindCurrent(0)).then(startWaitCI)
    .then(ciFail(0)).then(reserveRetry).then(retryResult(2))
    .expect(s.phase == 10 and s.retriedHeads.contains(0)
      and s.retryOutcome == 2 and s.publications == 0 and safety)
  run waitingRetryIsTerminalTest = init.then(bindCurrent(0)).then(startWaitCI)
    .then(ciFail(0)).then(reserveRetry).then(retryResult(3))
    .expect(s.phase == 10 and s.retriedHeads.contains(0)
      and s.retryOutcome == 3 and s.publications == 0 and safety)
  run successfulRetryWaitsOnSameHeadTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(0)).then(reserveRetry).then(retryResult(0))
    .expect(s.phase == 4 and s.remoteHead == 0 and s.candidateHead == -1
      and s.candidateTree == -1 and s.retriedHeads.contains(0)
      and s.publications == 0 and safety)
  run retryNeedsNewFailureAndIsPerHeadTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(0)).then(reserveRetry).then(retryResult(0))
    .then(ciFail(0)).then(suggestRepair(1)).then(verifyCandidate(0, 1, true, true))
    .then(pushRepair).then(bindCurrent(1)).then(syncWorkspace).then(startWaitCI).then(ciFail(0))
    .then(reserveRetry)
    .expect(s.phase == 9 and s.remoteHead == 1
      and s.retriedHeads == Set(0, 1) and s.retryHead == 1 and safety)
  run nativeMergeNeedsReadOnlyReadinessTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciPass).then(assertReadiness(0, false, false))
    .then(nativeMerge).expect(s.nativeMerged and s.phase == 11 and safety)
  run rejectedReadinessCannotMergeTest = init.then(bindCurrent(0)).then(startWaitCI)
    .then(ciPass).then(rejectReadiness(0, false, false, false))
    .expect(s.phase == 4 and not(s.readinessAsserted)
      and not(s.nativeMerged) and safety)
  run conflictReadinessCannotMergeTest = init.then(bindCurrent(0)).then(startWaitCI)
    .then(ciPass).then(rejectReadiness(0, true, false, true))
    .expect(s.phase == 4 and s.readinessConflict and not(s.readinessAsserted)
      and not(s.nativeMerged) and safety)
  run waitDecisionStopsWithoutMergeTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(0)).then(waitDecision)
    .expect(s.phase == 10 and s.waiting and not(s.nativeMerged) and safety)
  run resumedWaitRechecksCurrentHeadTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(0)).then(waitDecision).then(resumeWait)
    .then(bindCurrent(0)).then(startWaitCI).then(ciPass)
    .then(assertReadiness(0, false, false)).then(nativeMerge)
    .expect(s.nativeMerged and s.phase == 11 and safety)
  run resumedWaitNeedsGreenCiTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(0)).then(waitDecision).then(resumeWait)
    .then(nativeMerge).fail()
  run unacquiredRerunThenGreenCiMergesTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(1)).then(rerunUnacquired).then(ciPass)
    .then(assertReadiness(0, false, false)).then(nativeMerge)
    .expect(s.nativeMerged and s.unacquiredReruns.get(0) == 1
      and s.publications == 0 and safety)
  run unacquiredRerunStopsAtLimitTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(1)).then(rerunUnacquired)
    .then(ciFail(1)).then(rerunUnacquired).then(ciFail(1))
    .then(rerunUnacquired).fail()
  run unacquiredRerunLimitLeavesWaitTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(1)).then(rerunUnacquired)
    .then(ciFail(1)).then(rerunUnacquired).then(ciFail(1)).then(waitDecision)
    .expect(s.phase == 10 and s.unacquiredReruns.get(0) == MAX_RERUNS
      and not(s.nativeMerged) and safety)
  run genuineOrMixedFailureIsNeverRerunTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(2)).then(rerunUnacquired).fail()
  run rerunDueSkipsRepairAgentTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(1)).then(waitDecision).fail()
  run unacquiredRerunCountSurvivesResumeTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(1)).then(rerunUnacquired)
    .then(ciFail(1)).then(rerunUnacquired).then(ciFail(1)).then(waitDecision)
    .then(resumeWait).then(bindCurrent(0)).then(startWaitCI).then(ciFail(1))
    .then(rerunUnacquired).fail()
  run newHeadGetsItsOwnRerunsTest = init.then(bindCurrent(0))
    .then(startWaitCI).then(ciFail(1)).then(rerunUnacquired)
    .then(ciFail(1)).then(rerunUnacquired).then(advanceRemote(1, 1))
    .then(bindCurrent(1)).then(syncWorkspace).then(startWaitCI).then(ciFail(1))
    .then(rerunUnacquired)
    .expect(s.unacquiredReruns.get(0) == MAX_RERUNS
      and s.unacquiredReruns.get(1) == 1 and safety)
}
