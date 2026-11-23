# Manager executable specifications

Run from the repository root:

```sh
bash scripts/check_manager_specs.sh
```

The runner installs the repository's checksum-pinned toolchain using `scripts/setup_formal.sh`: Node 26.13.1, Quint 0.33.0, Apalache 0.62.1, Temurin JDK 25.0.5+8, and Alloy 6.2.0. It uses Rust 1.99.1 and the locked Cargo dependencies. See the setup script for artifact digests and supported host platforms. The GitHub Actions workflow `.github/workflows/manager-specs.yml` executes the same checks on pull requests without secrets. Reports contain command lines, per-check logs and Alloy's JSON receipt in `target/formal-reports/manager.*`.

The Quint model is a bounded abstraction of approval authorization, asynchronous scheduling, dependency integration, verification, corrective approval, pause, and publication. Retry archives the failed verification, clears and audits the pending preview without authorizing work, and never resets the preview revision. Rejected retry preserves the state. Tests exercise retry with and without a preview, stale approval after another failure, and fresh approval at the next revision. A failed check abstracts a publication at the integrated tip in an eligible phase. The model represents saved preview identity by revision and failed publication history by a count. It does not model exact evidence payloads, ambiguous publication, integrated tip changes, or child task retries. The runner typechecks and runs the executable tests, simulates 1,000 traces with seed 20261120 and at most 20 steps, and checks `safety` with Apalache to 16 transitions, enough to reach fresh approval after retry. Simulation is not proof, and bounded verification only covers the modeled transitions within the stated bound. It is not a proof of the Rust implementation, Git, process recovery, or forge behavior.

Alloy explicitly selects all named commands with SAT4J. The model embeds `expect 0` for checks and `expect 1` for witnesses, the syntax supported by the pinned Alloy CLI (which has no `exec --expect` option). The receipt validator independently checks that every assertion is UNSAT and every witness has a concrete SAT instance. The retry witnesses cover same-tip rejection of the discarded preview, approval of the next revision, and unchanged state after rejected retry. Gherkin invokes the real Manager API and CLI against isolated Git repositories with saved failed-verification fixtures. The runner also exercises Manager CLI regression tests and the production WebUI route and render tests. These checks do not cover the full daemon or forge lifecycle.

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
