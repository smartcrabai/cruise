# Subagent model verification receipt

Executed locally on **October 25, 2026**, macOS aarch64. Final aggregate ran
11:26:19 through 11:28:22 UTC, exit **0**, after all fixture corrections.
No commits were created, HEAD/branch refs were not moved, and pre-existing
working-tree changes were preserved.

## Installed versions and reproducible entry point

- Node `v22.22.0` (bootstrapped locally from the official release archive and
  checked against that release's SHA256 manifest).
- Quint `0.33.0`, installed by the committed npm lock with `npm ci --ignore-scripts`.
- Apalache `0.62.1`, build `a321771`, explicitly requested on an isolated endpoint.
- Alloy `6.2.0`, SHA256
  `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`.
- Temurin OpenJDK `25.0.5+8-LTS`, checksum-validated by shared setup.
- Rust `1.99.1 (58cf8f9c3 2026-10-13)` and Cucumber `0.23.0`.

The CI-equivalent command was:

```sh
export PATH="$PWD/target/formal-bootstrap/node-v22.22.0-darwin-arm64/bin:$PATH"
bash scripts/check_subagent_model_specs.sh
```

CI supplies the same Node version with `actions/setup-node`, not this local
bootstrap path. Exact individual commands are in `SUBAGENT_MODEL.md` and the
final report's `commands.log`. The bounded verifier's complete nested argv is
in `quint-verify.log`, including actual endpoint `127.0.0.1:54653`.

Final raw evidence directory, relative to repository root:

```text
target/formal-reports/subagent-model.7Gzt5U/
```

It contains `results.log`, `commands.log`, every CLI help/version log, test and
simulation ITF files, Apalache output, all Cucumber negative-control logs, and
`alloy/receipt.json` with concrete witness JSON. A preceding full aggregate also
passed in `subagent-model.5bkG61`; the final directory above is authoritative
because it reruns the strengthened SDK-qualified exclusion fixtures.

## Results

| Check | Observed result |
| --- | --- |
| Verified tool installation and version checks | All exit 0 |
| Actual installed Quint `typecheck`, `test`, `run`, `verify` help | All exit 0, retained |
| Alloy `help exec`, version and solver inventory | All exit 0, retained |
| `quint typecheck tests/formal/subagent_model.qnt` | Exit 0 |
| `quint test` | **10 tests passed** |
| `quint run --seed 20261025 --max-samples 1000 --max-steps 12` with `--invariant safety --backend typescript` | No violation in **1000 samples**, trace length 13 states (12 transitions) |
| `quint verify --invariant safety --max-steps 12` with `--backend apalache --apalache-version 0.62.1` | **NoError**, no violation through **12 transitions** |
| Alloy explicit `exec --command '*' --solver sat4j --type json` | All **13 selected commands** returned expected results |
| Strict Alloy receipt validator | **5 UNSAT assertions, 8 SAT witnesses** |
| Receipt-validator negative/unit tests | **13 passed** |
| Production Gherkin | **26 scenarios, 92 steps passed**, no skipped/undefined/pending steps |
| Real-runner negative controls | Undefined step, actual `haiku` versus deliberately wrong model assertion, and empty selection all rejected with expected exit **101** |
| `sdk_model_execution_cli` | **19 passed**, zero ignored |
| `subagent_model_jcode_cli` | **3 passed** plus one subprocess-only fixture entry point, which the tests launch explicitly |
| Full library regression suite | **1700 passed**, zero failures, three subprocess-only child probes |
| Aggregate status | **0**, all 22 named checks succeeded |

The ignored entries are helper entry points, not missing platform runners or
credential-dependent tests. The three library child probes are the existing
missing/nonzero forge-CLI child processes. jcode's fixture entry point is invoked
using `--ignored --exact fixture_jcode_runtime` by the actual CLI transport
contracts. No task check was replaced with a dry run or a skipped test.

**Simulation is not proof.** Apalache verification is a separate bounded check.
The Alloy scope is exactly **13 ordered State atoms**, **5-bit signed Int**,
**SAT4J**, equivalent to 12 transitions. No unbounded or full-Rust refinement
proof is claimed.

## Inspected Alloy receipt

The final raw receipt reports solver `sat4j` and timestamp `1792927592429`.
Every named command was selected explicitly with `--command '*'`. The pinned
CLI has no `--expect` flag, as confirmed by its saved help. Expectations are
therefore declared on each Alloy command and independently enforced against the
receipt, not inferred from the Java process's exit status.

| Receipt command | Kind | Declared expectation | Actual |
| --- | --- | --- | --- |
| RejectionBeforeDispatch | check | 0 | UNSAT |
| PrecedenceAndSdkIsolation | check | 0 | UNSAT |
| ExcludedMetadata | check | 0 | UNSAT |
| BackendContract | check | 0 | UNSAT |
| ConfigureBeforeSend | check | 0 | UNSAT |
| PrecedenceFallbackWitness | run | 1 | SAT, concrete instance |
| BarePlanningWitness | run | 1 | SAT, concrete instance |
| MetadataWitness | run | 1 | SAT, concrete instance |
| LegacyEnvironmentWitness | run | 1 | SAT, concrete instance |
| AgentsFailureWitness | run | 1 | SAT, concrete instance |
| RejectedWitness | run | 1 | SAT, concrete instance |
| OtherPhaseApplicabilityWitness | run | 1 | SAT, concrete instance |
| LocalApplicabilityRejectionWitness | run | 1 | SAT, concrete instance |

## Additional validation

All passed:

```sh
cargo +1.99.1 clippy --locked --all-targets --all-features -- -D warnings -A clippy::large_futures
cargo +1.99.1 fmt --all -- --check
shellcheck scripts/check_subagent_model_specs.sh
actionlint .github/workflows/ci.yml
taplo lint Cargo.toml
taplo fmt --check --option reorder_keys=true Cargo.toml
git diff --check
```

The Clippy `large_futures` exception matches existing provider-native CI policy,
not a new lint bypass. Strict Clippy output is retained in
`target/formal-reports/subagent-clippy.log`.

## Changes, corrections and prerequisites

- CI file changed: `.github/workflows/ci.yml`, new `subagent-model-specs` job.
  It runs the entire aggregate on ordinary relevant PRs/main pushes without
  secrets and uploads fresh evidence even after failure.
- Added paired Quint/Alloy models, the native Cucumber feature/definitions,
  Cargo test registration, negative controls, aggregate runner, traceability
  documentation and this receipt. Existing locked formal and Cucumber tooling
  was reused, so no dependency-version or lockfile churn was necessary.
- Strengthened production SDK subprocess unit tests in
  `src/workflow_generate.rs` and `src/worktree_pr.rs` to observe exclusion at
  actual dispatch, including repair turns and successful metadata output.
- During review, corrected those two new fixtures from bare jcode-owned values
  to explicit Claude-owned entries. Otherwise SDK isolation could hide an
  accidental forwarding bug. The final aggregate reran both corrected tests.
  Negative controls additionally require the actual model mismatch after a
  successful CLI run, rather than accepting an unrelated fixture failure.
- Refined the model to distinguish workflow-wide planning/execution candidate
  membership from prompt-local membership, with executable witnesses for both.
  No invariant or assertion was weakened to get a passing result.
- **No production implementation defect was found by these checks.** No existing
  production behavior was changed in this specification task.
- **Missing prerequisites: none on the tested host.** Public dependency downloads
  and all pinned runners succeeded. Unsupported hosts fail explicitly. Native
  GitHub CI was configured and linted, but no remote CI execution is claimed.
