# Environment override resolution model

`config_env_override.qnt` is a test-only finite abstraction of the planned
workflow override contract. It is not connected to production and does not
prove the Rust implementation conforms to the plan.

The scalar values stand for absent, blank, invalid, and two distinct valid
values. Resolution models builtin, app model, user defaults, project defaults,
workflow, environment, and a final CLI override. The session cleanup override
is combined with the final override, not separately modeled. Command and SDK
are abstract booleans. Invalid env input and simultaneous command/SDK env
input fail without changing the modeled configuration. A frozen snapshot does
not re-resolve when inputs change, whereas a file-backed session can resolve
again. Named tests provide witnesses for precedence, blank and invalid values,
mutual exclusion, and both session modes.

The `safety` invariant combines env and CLI precedence, blank no-op, atomic
failure, command/SDK exclusivity and snapshot stability. Simulation and
bounded verification cover only the explicitly enumerated transitions and eight
steps. They are not an unbounded proof or a test of the parsers, actual session
persistence, locale inference, app configuration, or individual Rust fields.
Use the concrete Rust tests for those production boundaries.

Run `bash scripts/check_config_env_specs.sh` on Darwin arm64 or Linux x64. The runner uses
`scripts/setup_formal.sh` and the repository's pinned Quint 0.33.0 and Apalache
0.62.1, saves commands, logs and traces under a fresh
`target/formal-reports/config-env.*` directory, and returns nonzero for any
failed check. Other platforms report BLOCKED.

## Alloy, Gherkin and CI

The runner executes `config_env_override.als` with Alloy 6.2.0, SAT4J and
scope 12. Assertions cover precedence, snapshot and file re-resolution,
parent-child precedence, atomicity and app persistence. Two witnesses require
concrete precedence and snapshot outcomes. These are bounded abstractions, not
a refinement proof of Rust. Alloy 6.2.0's `help exec` does not provide an
`--expect` option. Expectations are declared on every model command, and the
runner validates every command in the JSON receipt, failing on mismatches.

The executable Gherkin feature `tests/features/config_env.feature` uses the
production CLI in isolated temporary directories, complementing existing Rust
integration tests. The runner uses checksum-pinned Temurin JDK 25.0.5+8 and
Alloy 6.2.0, Node 26.12.1, Quint 0.33.0, Apalache 0.62.1 and Rust 1.99.1.
`.github/workflows/config-env-specs.yml` runs it without secrets and uploads
commands, logs and receipts. Principal commands, in addition to setup:

```sh
tests/formal/node_modules/.bin/quint typecheck tests/formal/config_env_override.qnt
tests/formal/node_modules/.bin/quint test tests/formal/config_env_override.qnt --main config_env_override --backend typescript --seed 20261107
tests/formal/node_modules/.bin/quint run tests/formal/config_env_override.qnt --main config_env_override --init init --step step --invariant safety --backend typescript --seed 20261107 --max-samples 1000 --max-steps 8
# The helper invokes quint verify --invariant safety --max-steps 8 using Apalache 0.62.1.
node scripts/formal/verify_quint.cjs tests/formal/node_modules/.bin/quint tests/formal/config_env_override.qnt config_env_override 8 target/formal-reports/counterexample.itf.json
java -jar target/formal-tools/alloy-6.2.0.jar help exec
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output REPORT/alloy tests/formal/config_env_override.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/config_env_override.als REPORT/alloy/receipt.json
cargo +1.99.1 test --locked --all-features --test config_env_bdd
```

Simulation is sampling, not proof. Apalache verification is bounded to 8
steps, and Alloy results to scope 12. Actual command lines and results are
recorded in each fresh report directory.

## Local execution receipt, 2026-11-07

On macOS arm64, `bash scripts/check_config_env_specs.sh` exited 0. Report:
`target/formal-reports/config-env.ufhOaP/`. Quint typechecking and all seven
tests passed. TypeScript simulation used seed 20261107, 1000 samples and eight
steps without finding a violation. Apalache 0.62.1 bounded verification of
`safety` found no violation through eight steps. Alloy's receipt recorded all
12 assertion checks UNSAT and both `PrecedenceWitness` and `SnapshotWitness`
SAT, as expected. The receipt validator and its tests passed. Production
Gherkin ran six scenarios and 31 steps successfully with strict skipped-step
handling. No production defects were fixed in this specification task. Linux
CI is configured but has not been executed locally; its result is not implied
by the macOS receipt. No external service credentials are required.
