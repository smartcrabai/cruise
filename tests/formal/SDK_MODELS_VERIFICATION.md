# SDK model specification verification, 2026-10-23

## Environment and exact entry point

Executed locally from the working repository on macOS arm64, using:

* Node `v22.22.0` (temporarily prepended the already downloaded pinned Node's
  `bin` directory to PATH, without altering global installs).
* Quint `0.33.0`, installed from `tests/formal/package-lock.json` by `npm ci
  --ignore-scripts` through the repository setup script. Audit: zero known
  vulnerabilities reported by that install.
* Apalache `0.62.1`, selected explicitly and started on a fresh isolated
  loopback endpoint, using Temurin `25.0.5+8-LTS`.
* Alloy CLI `6.2.0`, artifact SHA-256 checked by `setup_formal.sh`, solver
  `sat4j`, with the same Temurin JDK.
* `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, Cucumber `0.23.0` from the existing
  exact Cargo dependency and committed lockfile.

The exact CI entry point was run, not only its individual commands:

```sh
bash scripts/check_sdk_model_specs.sh
```

It completed with **aggregate exit status 0**. Fresh evidence directory:

```text
target/formal-reports/sdk-models.YfL1NC/
```

`commands.log` lists every exact command. `quint-verify.log` also contains the
nested verifier's exact argv, backend version and actual isolated endpoint.
All prerequisite setup, version checks, installed CLI help commands and solver
inventory checks returned zero. The exact expanded commands and pins are also
listed in `SDK_MODELS.md` for reproduction without this local report path.

## Quint outcomes

| Check | Observed result |
| --- | --- |
| `quint typecheck tests/formal/sdk_models.qnt` | Exit 0 |
| `quint test tests/formal/sdk_models.qnt --backend typescript --seed 20261023 ...` | 6 tests passed, no ignored/failed tests |
| `quint run --seed 20261023 --max-samples 1000 --max-steps 12 --invariant safety --backend typescript ... tests/formal/sdk_models.qnt` | Exit 0, no violation found |
| `quint verify --invariant safety --max-steps 12 --main sdk_models --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint <isolated endpoint> ... tests/formal/sdk_models.qnt` | Exit 0, Apalache outcome `NoError`, no violation through 12 transitions |

The executable tests are `crossSdkFallbackTest`, `reverseInheritanceTest`,
`visibleFailureTest`, `cooldownSurvivesPolicyTest`, `cancellationTest`, and
`allCoolingTest`. The checked `safety` conjunction includes `selection`,
`resumeSafety`, `checkpointSafety`, `cooldownSafety`, `replaySafety`, and
`domains`, including the independent reachable-state assertion that the
same-SDK effort-only duplicate is never attempted.

**Simulation is not proof.** Its seed is **20261023**, with **1000 samples**
and **12 steps per sample**. The separate symbolic verification has a bound
of **12 transitions**, backend **Apalache 0.62.1**. No unbounded verification
or proof that all Rust execution refines this model is claimed.

## Alloy receipt inspection

Ran the installed CLI's **`help exec`** first. Its documented syntax has no
`--expect` flag. Expectations therefore use its supported `expect 0` / `expect
1` model-command declarations, independently checked against the complete
receipt. All commands were explicitly selected:

```sh
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/sdk_models.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/sdk_models.als "$R/alloy/receipt.json"
```

Here `R` is the fresh evidence directory above. Both commands exited zero.
Scope: **exactly 13 ordered states, 12 transitions, signed 5-bit integers**.
All arithmetic stays in range. Actual receipt:
`target/formal-reports/sdk-models.YfL1NC/alloy/receipt.json`.

| Receipt command | Result | Required expectation |
| --- | --- | --- |
| Selection | UNSAT, no counterexample | 0 |
| ResumeSafety | UNSAT, no counterexample | 0 |
| CheckpointSafety | UNSAT, no counterexample | 0 |
| CooldownSafety | UNSAT, no counterexample | 0 |
| ReplaySafety | UNSAT, no counterexample | 0 |
| Domains | UNSAT, no counterexample | 0 |
| CrossSdkWitness | SAT, concrete instance emitted | 1 |
| ReverseInheritanceWitness | SAT, concrete instance emitted | 1 |
| VisibleFailureWitness | SAT, concrete instance emitted | 1 |
| PolicyReloadWitness | SAT, concrete instance emitted | 1 |
| CancellationWitness | SAT, concrete instance emitted | 1 |
| AllCoolingWitness | SAT, concrete instance emitted | 1 |

The receipt validator confirmed **6 UNSAT assertions and 6 SAT witnesses**,
including instance presence, command types and complete command coverage.
All six `*-solution-0.json` witness files are in that receipt directory.
`node --test scripts/formal/check_alloy_receipt.test.cjs` passed **13/13**
positive and fail-closed negative tests.

## Gherkin and production outcomes

```sh
cargo +1.99.1 test --locked --all-features --test sdk_model_bdd
node scripts/formal/check_sdk_model_bdd_failures.cjs "$R"
```

**11 scenarios, 39 steps passed**, no skipped/undefined/pending steps and no
scenario retries. Real production CLI execution and planning dispatched actual
local SDK subprocesses in `jcode → claude → jcode` order. A separate same-name
scenario preserved cross-SDK eligibility but omitted the repeated same-SDK
effort variant. Direct real `Executor::run` calls passed the saved resume only
to its SDK owner and reported actual Claude ownership in callback and outcome.
Real app JSON/YAML files survived atomic unrelated-setting saves without losing
entry form/order. Command conflicts failed before the shell effect or SDK
subprocess could start.

The negative controls each exercised the real runner:

* Undefined step: one rejected step, exit **101**.
* Deliberately incorrect SDK dispatch assertion after actual successful Claude
  execution: one rejected step, exit **101**. The inspected failure compared
  recorded `"claude\n"` to expected `"jcode\nclaude\njcode\n"`.
* Empty `--name '^NO_SUCH_SDK_MODEL_SCENARIO$'` selection: the executed-step
  guard rejected zero checks, exit **101**.

These expected failures are successful **negative controls**, not passing
production scenarios. Their logs show real Cucumber failure evidence, not a
compiler/platform failure being mistaken for an assertion rejection.

The aggregate additionally ran these exact task-specific integrations:

```sh
cargo +1.99.1 test --locked --all-features --test model_fallback --test sdk_model_config_contract --test sdk_model_execution_cli --test app_config_cli --test workflow_defaults_cli --test model_logging_cli --test session_config_contract
cargo +1.99.1 test --locked --all-features --lib
```

All **77 integration tests passed** (8 app config, 9 model fallback, 3 model
logging, 12 SDK configuration, 9 SDK execution, 20 session config, 16 workflow
defaults). The complete library suite passed **1592 tests**, with **3 existing
ignored child-process probes**, zero failures. Ignored probes are not counted
as passed specification checks.

## Additional repository checks

Passing local checks:

```sh
git diff --check
cargo +1.99.1 fmt -- --check
taplo lint Cargo.toml
taplo fmt --check --option reorder_keys=true Cargo.toml
shellcheck scripts/check_sdk_model_specs.sh
actionlint .github/workflows/ci.yml
cargo +1.99.1 clippy --locked --all-targets --all-features -- -D warnings -A clippy::large_futures
```

The full **`cargo +1.99.1 test --locked --all-features`** regression also
completed with **exit 0**, after Clippy passed. Its 37 native test summaries
report **4170 passed, zero failed, 7 ignored**, plus **8 passing checkpoint
Cucumber scenarios**. The ignored results are the three existing child-process
probes compiled in both library and runtime test targets, plus the explicitly
opt-in installed OpenCode V2 test. These ignored checks were not treated as
passes or substituted for any SDK specification check. The new SDK Gherkin is
registered with `test = false` for POSIX platform control and was separately
executed explicitly by the successful specification runner above.

Exact extra commands and exit statuses are retained in `extra-commands.log`
and `extra-results.log`, with complete output in `clippy.log` and
`full-regression.log` in the same evidence directory. Formatting, whitespace,
shellcheck and actionlint were rerun after the full regression and passed.

## Changes, defects and limitations

Added the paired `sdk_models.qnt` / `sdk_models.als`, the native Cucumber test
and feature, two negative-control features, the fail-closed SDK runner and its
negative-control script, this evidence report and `SDK_MODELS.md`. Registered
the runner in `Cargo.toml` using the repository's existing exact Cucumber
version. Reused the existing pinned npm lock and checksum-verified formal
setup rather than introducing a second incompatible toolchain.

CI changed: **`.github/workflows/ci.yml`**, adding **sdk-model-specs**. Every
new model/test/receipt check is invoked by its PR/push aggregate job, without
secrets, with pinned runtimes and always-uploaded evidence. Existing
Linux/macOS/Windows production CI and the older formal suites remain intact.
Local actionlint passes. No remote GitHub Actions run is claimed.

No new production defect was exposed by these checks, so no production source
was changed for this specification task. The production implementation and
other dirty files were already present when this task started and were
preserved. During model authoring, Quint rejected `fail` as a reserved built-in
name. The action was renamed `modelMissing` without changing its transition or
requirements, and every Quint check was rerun. The effort-dedup safety property
was strengthened, not weakened, before the successful complete aggregate run.

There is **no missing prerequisite blocking these checks on this host**. The
initial default Node version differed, resolved by using pinned Node 22.22.0
locally. Linux remote CI and Windows production execution were not performed
locally. The SDK specification runner explicitly fails on unsupported
platforms rather than reporting a skipped check as a pass. Paid SDK APIs and
live provider availability are intentionally outside this reproducible suite.

**No commits were created, no HEAD or branch refs moved, and no commit guard
was disabled or bypassed. All repository changes remain uncommitted.**
