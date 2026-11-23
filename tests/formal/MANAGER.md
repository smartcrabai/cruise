# Manager executable specifications

Run from the repository root:

```sh
bash scripts/check_manager_specs.sh
```

The runner installs the repository's checksum-pinned toolchain using `scripts/setup_formal.sh`: Node 26.13.1, Quint 0.33.0, Apalache 0.62.1, Temurin JDK 25.0.5+8, and Alloy 6.2.0. It uses Rust 1.99.1 and the locked Cargo dependencies. See the setup script for artifact digests and supported host platforms. The GitHub Actions workflow `.github/workflows/manager-specs.yml` executes the same checks on pull requests without secrets. Reports contain command lines, per-check logs and Alloy's JSON receipt in `target/formal-reports/manager.*`.

The Quint model is a bounded abstraction of approval authorization, asynchronous scheduling, dependency integration, verification, corrective approval, pause, and publication: approval names the reviewed proposal revision and creates no reservation or child launch, but the subsequent coordinator actions may schedule authorized work. A failed check blocks the Goal; `cruise manager retry GOAL` re-verifies a fully integrated Goal, a saved corrective preview authorizes nothing, and only approval of the current preview revision adds the corrective task. Stale proposal and corrective revisions are disabled actions, checked by `fail()` tests. The model does not include integrated tip changes, child task retries, or Manager-owned child prompts. The runner typechecks and runs the executable tests, simulates 1,000 traces with seed 20261120 and at most 20 steps, and checks `safety` with Apalache to 12 transitions. Simulation is not proof, and bounded verification only covers the modeled transitions within the stated bound. It is not a proof of the Rust implementation, Git, process recovery, or forge behavior.

Alloy explicitly selects all named commands with SAT4J. The model embeds `expect 0` for checks and `expect 1` for witnesses, the syntax supported by the pinned Alloy CLI. The receipt validator independently checks that every assertion is UNSAT and every witness has a concrete SAT instance. The approval Gherkin scenarios invoke the real Manager API against isolated Git repositories; they do not claim coverage of the full daemon, Git integration, or forge lifecycle. Existing Manager integration tests cover additional production boundaries.

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

After adding proposal revisions, failed verification, verification retry, and corrective preview/approval to the Quint model, `bash scripts/check_manager_specs.sh` passed on macOS arm64 with the same commands; reports are in `target/formal-reports/manager.RXeFiV/`. The Quint tests reported eight passes, including three `fail()` tests for stale proposal approval, stale corrective approval, and scheduling the corrective task before approval. Simulation and Apalache (12 transitions) found no violation; Alloy and Gherkin results were unchanged. An earlier run found that `retry GOAL` keeps an unapproved corrective preview, as `manager_publication::retry_verification` does; the invariant now requires only that such a preview cannot be approved until the Goal is blocked again. No production defect was fixed.
