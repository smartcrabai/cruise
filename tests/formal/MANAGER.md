# Manager executable specifications

Run from the repository root:

```sh
bash scripts/check_manager_specs.sh
```

The runner installs the repository's checksum-pinned toolchain using `scripts/setup_formal.sh`: Node 26.13.1, Quint 0.33.0, Apalache 0.62.1, Temurin JDK 25.0.5+8, and Alloy 6.2.0. It uses Rust 1.99.1 and the locked Cargo dependencies. See the setup script for artifact digests and supported host platforms. The GitHub Actions workflow `.github/workflows/manager-specs.yml` executes the same checks on pull requests without secrets. Reports contain command lines, per-check logs and Alloy's JSON receipt in `target/formal-reports/manager.*`.

The Quint model is a bounded abstraction of approval authorization, asynchronous scheduling, dependency integration, verification, corrective approval, pause, and publication. Retry archives the failed verification, clears and audits the pending preview without authorizing work, and never resets the preview revision. Rejected retry preserves the state. Tests exercise retry with and without a preview, stale approval after another failure, and fresh approval at the next revision. A failed check abstracts a publication at the integrated tip in an eligible phase. The model represents saved preview identity by revision and failed publication history by a count. It does not model exact evidence payloads, ambiguous publication, integrated tip changes, or child task retries. The runner typechecks and runs the executable tests, simulates 1,000 traces with seed 20261120 and at most 20 steps, and checks `safety` with Apalache to 16 transitions, enough to reach fresh approval after retry. Simulation is not proof, and bounded verification only covers the modeled transitions within the stated bound. It is not a proof of the Rust implementation, Git, process recovery, or forge behavior.

`manager_reservation.qnt` separately models one reservation from the moment `retry` or dispatch saves the attempt until its child launches, while sibling children integrate and the daemon stops inside dispatch. Dispatch pins a reservation that has no published session to the current tip again and launches it once from that tip. A session published under an older pin blocks the Goal and is never re-pinned, reactivated, or launched. `legacyBlock` stands for records that earlier versions blocked regardless of publication; an explicit retry reactivates only the unpublished ones without saving another attempt. The runner applies the same typecheck, tests, and simulation, and checks `safety` with Apalache to 12 transitions. The model does not cover Git, the filesystem, capacity, pause, or the child's own result.

`manager_corrections.qnt` separately models the owner-approved corrections of one Goal: one failed task attempt and the single preview slot, with a contract amendment, a corrected resume, and an evidence run, while a sibling integrates, the child worktree changes, and the daemon restarts. A preview authorizes nothing and is approved only for the tip, contract, attempt, and worktree it was saved against. A correction reaches the child only by the one launch its approval armed; a launch refused for a changed state has to be reviewed again before any resume. A launch or evidence run that had started before a restart is reported and never replayed. An amendment advances the contract revision without changing the proposal, the checks, or the plan of an existing attempt. Evidence resolves the task only when every approved check passed and the tip, contract, and attempt are still the approved ones; it never marks the task integrated, and the final verification covers all checks at the final tip. The runner applies the same typecheck, tests, and simulation, and checks `safety` with Apalache to 12 transitions. The model does not cover Git, check commands, the plan text, capacity, pause, or more than one task.

`manager.als` treats a task as done when it is integrated or resolved by approved evidence. A dependent is dispatched and verification starts only when the tasks before them are done. A resolved task is never integrated, and resolution leaves the tip and every integration unchanged (`EvidenceIsNotIntegration` and the `EvidenceUnblocksDependent` witness). `manager.qnt` keeps one `integrated` set that stands for done tasks: with a separate set its 16-transition verification was still running after 9.5 minutes, against under 3 minutes before, so the distinction is checked in `manager_corrections.qnt` and Alloy instead.

Alloy explicitly selects all named commands with SAT4J. The model embeds `expect 0` for checks and `expect 1` for witnesses, the syntax supported by the pinned Alloy CLI (which has no `exec --expect` option). The receipt validator independently checks that every assertion is UNSAT and every witness has a concrete SAT instance. The retry witnesses cover same-tip rejection of the discarded preview, approval of the next revision, and unchanged state after rejected retry. Gherkin invokes the real Manager API and CLI against isolated Git repositories with saved failed-verification fixtures. The runner also exercises Manager CLI regression tests and the production WebUI route and render tests. These checks do not cover the full daemon or forge lifecycle.

GitHub title rejection is outside the Quint and Alloy abstractions. The `manager_verification_retry` regression exercises the production CLI and authenticated Manager API with a disposable daemon, Git refs, and forge subprocess fixtures. It checks Unicode title bounds and unchanged short titles, lossless Objective text, explicit recovery of a definite legacy rejection, adoption of a matching existing PR, and refusal on ambiguous failures, missing evidence, mismatched refs, and failed observations.

The publication fixture uses the recorded `gh failed: pull request create failed: GraphQL: ...` form and the older unwrapped form. A near-match with an appended timeout remains blocked; the wrapped rejection with a moved remote base reaches and fails the unchanged base check.

Approved base retargeting is covered at the production CLI, daemon, and Git boundaries in `manager_verification_retry`; it is not modeled in Quint or Alloy. The review pins the same branch's fast-forward SHA, original integrated tip, contract, verified publication and authenticated GitHub identity. Approval saves a two-parent integration intent and rechecks it before applying or adopting it. At the new tip every check reruns in order before either remote write. The regression also checks stale previews, conflicts, non-fast-forward ancestry, unusable PR observations, remote drift, and restart after uncertain push and create intents without repeating either write.

The production regression also accepts a finalized but unintegrated result superseded by a later integrated attempt. An unintegrated latest attempt is rejected even if an older attempt was integrated; unresolved finalization, pending or interrupted resumes, and pending corrections are rejected without creating a preview.

`manager_goal_projection.qnt` models the redacted snapshot boundary for Manager conversation and proposal drafting. Completed Goals expose only ID, objective, and status to the provider; unfinished Goals retain their context, and durable records remain intact. The Manager CI runner checks the model and captures the production prompt in a fake SDK regression.

`manager_image_binding.qnt` models durable private input, no replay across restart, and task-scoped delivery at the approved revision. Manager CI checks the model and runs an isolated CLI, daemon, fake SDK, and child-workflow regression with real image bytes.

## Local execution, 2026-11-20

`bash scripts/check_manager_specs.sh` passed on macOS arm64. Its exact commands and output are in `target/formal-reports/manager.q9JVSV/commands.log` and the adjacent logs. The invocation ran the following substantive checks (the runner expands `Q` to the pinned binary and uses an isolated temporary backend endpoint):

```sh
Q=tests/formal/node_modules/.bin/quint
$Q typecheck tests/formal/manager.qnt
$Q test tests/formal/manager.qnt --backend typescript --seed 20261120
$Q run --seed 20261120 --max-samples 1000 --max-steps 20 --backend typescript --invariant safety tests/formal/manager.qnt
node scripts/formal/verify_quint.cjs "$Q" tests/formal/manager.qnt manager 12 target/formal-reports/manager.q9JVSV/counterexample.itf.json
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output target/formal-reports/manager.q9JVSV/alloy tests/formal/manager.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/manager.als target/formal-reports/manager.q9JVSV/alloy/receipt.json
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test manager_approval_bdd
```

All checks passed. The Quint tests reported three passes; simulation found no violation; Apalache reported no violation through 12 transitions. The Alloy receipt contained seven UNSAT assertions and five SAT witnesses. Gherkin executed two scenarios and seven steps. No production defect was fixed. The formal models do not cover all plan requirements, including exact target identity, real recovery effects and child session ownership, and the Gherkin scenarios cover the proposal/approval boundary only. These gaps require additional specifications and production-boundary scenarios rather than being inferred from a green result.

## Local execution, 2026-11-23

After adding proposal revisions, failed verification, verification retry, and corrective preview/approval to the Quint model, `bash scripts/check_manager_specs.sh` passed on macOS arm64 with the same commands; reports are in `target/formal-reports/manager.RXeFiV/`. The Quint tests reported eight passes, including three `fail()` tests for stale proposal approval, stale corrective approval, and scheduling the corrective task before approval. Simulation and Apalache (12 transitions) found no violation; Alloy and Gherkin results were unchanged. An earlier run found that `retry GOAL` kept an unapproved corrective preview, and this run only required that it could not be approved until the Goal was blocked again. No production defect was fixed in that run. The behavior and invariant are superseded by the fix below.

## Local execution, 2026-11-23: retry discards corrective previews

Fixed the production defect in `manager_publication::retry_verification`: retry now clears an unapproved preview and preserves it in `corrective_discarded`. The discarded revision cannot be approved, even after another failure at the same tip, and a fresh preview uses a higher revision.

`bash scripts/check_manager_specs.sh` passed on macOS arm64 with the pinned toolchain. Reports are in `target/formal-reports/manager.JVqENh/`. All ten Quint runs passed, including stale approval after retry and successful approval of a fresh revision. Simulation found no violation across 1,000 traces, and Apalache found none through 12 transitions. Alloy returned seven UNSAT assertions and five SAT witnesses; all thirteen receipt-validator tests passed. Gherkin passed two scenarios and seven steps.

## Local execution, 2026-11-23: retry contract across all three specifications

`bash scripts/check_manager_specs.sh` exited 0 on macOS arm64 using Node 26.13.1, Quint 0.33.0, Apalache 0.62.1, Temurin JDK 25.0.5+8, Alloy 6.2.0, Rust 1.99.1, and Cucumber 0.23.0. See `target/formal-reports/manager.BrMIN1/commands.log` for exact arguments and `quint-verify.log` for the expanded verification command. No additional production defects were found beyond the discard and revision-reuse bug fixed above.

```sh
bash scripts/setup_formal.sh
source target/formal-tools/env.sh
Q=tests/formal/node_modules/.bin/quint
RUN=$(mktemp -d "$PWD/target/formal-reports/manager.replay.XXXXXX")
$Q typecheck tests/formal/manager.qnt
$Q test tests/formal/manager.qnt --backend typescript --seed 20261120
$Q run --seed 20261120 --max-samples 1000 --max-steps 20 --backend typescript --invariant safety tests/formal/manager.qnt
node scripts/formal/verify_quint.cjs "$Q" tests/formal/manager.qnt manager 16 "$RUN/counterexample.itf.json"
java -jar target/formal-tools/alloy-6.2.0.jar help exec
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$RUN/alloy" tests/formal/manager.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/manager.als "$RUN/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test manager_approval_bdd
cargo +1.99.1 test --locked --all-features --test manager_corrective_approval
cargo +1.99.1 test --locked --all-features --lib webui::tests::manager::manager_retry
```

Quint passed 11 tests, simulation found no violation, and bounded verification reported `NoError` through 16 transitions. Alloy's `alloy/receipt.json` records 10 UNSAT assertions and 8 SAT witnesses, including the three concrete retry traces. All 13 receipt-validator tests passed. Cucumber executed 5 scenarios and 21 steps, with no skipped steps. All 4 CLI and 4 WebUI regressions passed. No local prerequisite was missing. The GitHub Actions workflow is wired but was not dispatched from the worktree.

`cargo +1.99.1 fmt --all -- --check` passed. Strict `cargo +1.99.1 clippy --all-targets --all-features --locked -- -D warnings` reported repository-wide `large_futures` errors. The same command with the existing CI exception `-A clippy::large_futures` passed.

## Local execution, 2026-11-25: reservation pinned at dispatch

Fixed the production defect in `manager_coordinator::reserve_and_claim`: a reservation saved by `retry GOAL TASK` kept its base while another child was integrated, so dispatch blocked the Goal with `interrupted Manager reservation no longer matches Goal tip` and every later task retry failed with `a Manager reservation is already pending`. Dispatch now pins a reservation with no published session to the current tip again, and `retry GOAL TASK` reactivates a Goal that an earlier version blocked this way.

`bash scripts/check_manager_specs.sh` passed on macOS arm64 with the pinned toolchain; reports are in `target/formal-reports/manager.D3eruG/`. `manager_reservation.qnt` passed 9 tests and 1,000 simulated traces, and Apalache found no violation through 12 transitions. Results for `manager.qnt`, Alloy, Gherkin, CLI, and WebUI were unchanged. Three daemon tests in `tests/manager_resume_recovery.rs` check the same contract through the real CLI: a sibling integrating after `retry`, a persisted blocked record, and reservations that may have started.

## Local execution, 2026-11-25: approved corrections

Added contract amendments, corrected resumes, and evidence resolution in `manager_coordinator::corrections` and `manager_publication::run_evidence`. No defect in earlier behavior was fixed.

`bash scripts/check_manager_specs.sh` passed on macOS arm64 with the pinned toolchain; reports are in `target/formal-reports/manager.yVcAjL/`. `manager_corrections.qnt` passed 26 tests and 1,000 simulated traces, and Apalache found no violation through 12 transitions. Seven single-line faults seeded into copies of that model each violated `safety` in simulation: launching without the reviewed-state comparison, adopting evidence without it, resolving on a partial result, replaying a started launch, approving a stale amendment, starting dependents early, and rerunning an unresolved run. `manager.als` returned 11 UNSAT assertions and 9 SAT witnesses. Results for `manager.qnt`, `manager_reservation.qnt`, Gherkin, CLI, and WebUI were unchanged.

Eight daemon tests in `tests/manager_corrections.rs` and two in `tests/manager_resume_recovery.rs` check the same contract through the real CLI, daemon, and Git: stale tips, other attempts, concurrent approval, restarts before and after a launch, red, interrupted, and overtaken evidence, a record written by an earlier version, and the final verification of a Goal with an evidence-resolved task.
