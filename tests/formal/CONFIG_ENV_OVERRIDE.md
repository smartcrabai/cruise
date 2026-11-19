# Environment override resolution model

`config_env_override.qnt` and `config_env_override.als` are test-only finite
abstractions of workflow override precedence and the daemon's per-request
environment boundary. Each CLI request carries a separate workflow override
snapshot and cwd-selected base configuration; no-context requests use the
daemon process environment, and distinct snapshots remain isolated.
Existing grammar, precedence, failure atomicity, command/SDK exclusion, and
session freeze/re-resolution guarantees remain part of the models.

The abstraction is not a refinement proof of the Rust implementation and does
not model the concrete parsers, cwd path resolution, persistence, or each API
load site.
In production, `RequestContext::capture` in `src/request_context.rs`
captures cwd and `WORKFLOW_ENV_KEYS`; `ConfigEnv::{Process, Snapshot}` and
`WorkflowConfig::apply_env_overrides_with` in `src/config.rs` select the source.
CLI request handling applies snapshots across session creation, planning, run,
per-step hot reload, run-all and create-PR paths. WebUI requests without context
use the daemon process environment for workflow overrides; `PROCESS_ENV_NAMES`
are excluded from snapshots and keep their process-level source. Autostart
removes workflow override variables in `src/daemon/lifecycle.rs`; explicit
`daemon start` and `serve` preserve them.

The `safety` invariant combines precedence, blank no-op, atomic failure,
command/SDK exclusivity and session snapshot stability. Simulation and bounded
verification cover only explicitly enumerated transitions and eight steps.
Use the concrete Rust tests and executable Gherkin for production boundaries.

Run `mise exec node@26.13.1 -- bash scripts/check_config_env_specs.sh` on Darwin
arm64 or Linux x64. The runner uses `scripts/setup_formal.sh` and the
repository's pinned Quint 0.33.0 and Apalache 0.62.1, saves commands, logs and
traces under a fresh `target/formal-reports/config-env.*` directory, and
returns nonzero for any failed check. Other platforms report BLOCKED.


## Alloy, Gherkin and CI

The runner executes `config_env_override.als` with Alloy 6.2.0, SAT4J and
scope 12. Assertions cover precedence, snapshot and file re-resolution,
parent-child precedence, atomicity, app persistence, per-request snapshot and
cwd isolation, and process-vs-snapshot selection. Five witnesses require
concrete precedence, snapshot, request-isolation, cwd-isolation and
process-selection outcomes. These are bounded abstractions, not a refinement
proof of Rust. Alloy 6.2.0's `help exec` does not provide an `--expect` option.
Expectations are declared on every model command, and the runner validates
every command in the JSON receipt, failing on mismatches.

The executable Gherkin feature `tests/features/config_env.feature` drives the
production CLI against an isolated daemon. It verifies that a workflow
override reaches only its request and that CLI requests ignore a value held
only in the daemon process environment. The runner uses checksum-pinned
Temurin JDK 25.0.5+8, Alloy 6.2.0, Node 26.13.1, Quint 0.33.0, Apalache
0.62.1 and Rust 1.99.1. `.github/workflows/config-env-specs.yml` runs it
without secrets and uploads commands, logs and receipts. Principal commands,
in addition to setup:

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

## Historical local receipt, 2026-11-07 (before request-context modeling)

This run covers the earlier model revision and does not verify the per-request
environment cases added after it.

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

## Current local execution receipt, 2026-11-18

On macOS arm64, `mise exec node@26.13.1 -- bash scripts/check_config_env_specs.sh`
exited 0. The report is `target/formal-reports/config-env.6vegNP/`. Quint
typechecking and all nine named tests passed. Seeded simulation used 1000
samples and eight steps without a violation; Apalache's bounded verification
of `safety` found no violation through eight transitions. Alloy recorded all
20 assertions UNSAT and all five witnesses SAT; receipt validation and its
tests passed. Strict production Gherkin passed eight scenarios and 48 steps,
including the two daemon request-context cases. Linux CI has not been run.
