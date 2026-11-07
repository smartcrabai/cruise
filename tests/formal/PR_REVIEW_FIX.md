# PR review fix bounded specifications

These models describe the `reviewers: all` permission boundary and the D4
termination decision, not the entire PR workflow. The production-connected
scenarios are in `tests/features/pr_review_fix.feature` and
`tests/pr_review_fix_bdd.rs`. Run `bash scripts/check_pr_review_fix_specs.sh`.
The runner requires the repository's pinned formal toolchain, validates every
Alloy receipt, runs the production BDD target, and writes fresh logs under
`target/formal-reports/pr-review-fix.*`. A setup failure is not a pass.

Both models bound the universe to three thread IDs and the iteration limit to
two. Quint checks up to eight transitions, including stuttering. Alloy checks
five ordered states (four transitions) with four-bit integers. Permissions are
fixed throughout a trace. The model treats the set of write-authorized authors'
threads as `eligible`; it does not implement GitHub permission lookup or its
failure modes. On an iteration the eligible unresolved set can change
arbitrarily, including new thread IDs, while excluded unresolved IDs are held
fixed. Each iteration passes precisely the pre-iteration eligible unresolved
IDs to the prompt.

The decision priority is success when the new eligible unresolved set is empty,
then no progress when it equals the previous nonempty set, then the iteration
limit, otherwise another iteration. Thus the no-progress equivalence applies
only immediately after an iteration and with a nonempty result. Quint checks
`successImpliesNoEligibleUnresolved`, `noPromptOnExcluded`,
`iterationsBounded`, `terminalAbsorbing`, and `noProgressIffSameSet`; Alloy
checks the corresponding contracts including `PermissionFilterBeforePrompt`
and `ExcludedNeverPrompted`. Witnesses exercise success with excluded threads,
no progress, and the limit after a change in the unresolved set.

These are bounded models, not a refinement proof of Rust or GitHub, nor a
liveness guarantee. They do not model network errors, changes in permissions,
worktrees, actual prompt contents, concurrent mutations of excluded threads,
or crashes. Production tests cover concrete behavior separately.
