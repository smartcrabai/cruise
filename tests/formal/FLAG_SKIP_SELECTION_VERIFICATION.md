# Flag-dependent skip selection: execution evidence

Executed on 2026-10-26 in the task worktree on macOS arm64. Requirements authority: the session `plan.md` for `20261025084033023_d7e09b149a3f47459cdc28e7983e7d73`, especially R1-R3, sections 1.1-1.3, 2.3-2.7 and 6. See `FLAG_SKIP_SELECTION.md` for requirement-to-production-to-specification mapping and abstraction limits.

## Reproduction and evidence location

```sh
export PATH="$HOME/.cargo/bin:/Users/admin/.local/share/mise/installs/node/22.22.0/bin:$PATH"
bash scripts/check_workflow_flags_specs.sh
```

The final invocation writes fresh evidence to `target/formal-reports/workflow-flags.a196hl/`. Its `commands.log` contains exact executable argv for every check, `results.log` contains exit statuses, and each named check has a separate log. The outer capture is `target/formal-reports/flag-skip-development/post-review-aggregate.log`. These local generated paths are ignored by Git. CI uploads the same report layout, including receipts and witness JSON. Local execution does not claim remote GitHub execution.

Versions actually checked: Node **22.22.0**, Quint **0.33.0**, Apalache **0.62.1**, Alloy CLI **6.2.0**, Temurin OpenJDK **25.0.5+8-LTS**, rustc **1.99.1** (`58cf8f9c3`, 2026-10-13). Existing exact Cargo dependencies: Cucumber **0.23.0**, portable-pty **0.9.0**, tempfile **3.27.0**. The existing package lock, Cargo lock, released JAR and JDK checksum pins are reused without introducing another install system. `scripts/setup_formal.sh` succeeded and validates both cached artifacts and actual versions. Exact artifact checksums are recorded in `FLAG_SKIP_SELECTION.md` and the setup script.

## Quint

Actual commands, where `Q=tests/formal/node_modules/.bin/quint` and `R` is the absolute evidence directory:

```sh
"$Q" typecheck tests/formal/flag_skip_selection.qnt
"$Q" test tests/formal/flag_skip_selection.qnt --backend typescript --seed 20261026 --out-itf "$R/skip_test_{test}_{seq}.itf.json"
"$Q" run --seed 20261026 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$R/skip_simulation_{seq}.itf.json" tests/formal/flag_skip_selection.qnt
node scripts/formal/verify_quint.cjs "$Q" "$PWD/tests/formal/flag_skip_selection.qnt" flag_skip_selection 8 "$R/skip-counterexample.itf.json"
```

The helper executes the actual pinned CLI, with dynamically allocated local PORT:

```sh
"$Q" verify --invariant safety --max-steps 8 --main flag_skip_selection --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/skip-counterexample.itf.json" tests/formal/flag_skip_selection.qnt
```

Results: typecheck passed, **13 executable tests passed**, seeded simulation found no safety violation in **1000 samples at 8 steps**, and separate **Apalache 0.62.1 bounded verification through 8 transitions** returned `NoError` and `No violation found`. The aggregate verification took 11182 ms. Simulation is not proof, and bounded symbolic verification is not an unbounded proof or a Rust refinement proof. The final model includes partial explicit CLI assignments, explicit OFF, legacy keys, cancellation, API creation, group/after-pr pruning and known-empty versus unknown catalogs. CLI help for all four commands was captured before use.

The unchanged execution-start Quint model also passed its typecheck, 12 tests, simulation (seed **20261024**, **1000 samples**, **8 steps**) and Apalache safety verification through **8 transitions**.

## Alloy receipts and non-vacuity

```sh
java -jar target/formal-tools/alloy-6.2.0.jar version
java -jar target/formal-tools/alloy-6.2.0.jar help exec
java -jar target/formal-tools/alloy-6.2.0.jar solvers
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/skip-alloy" tests/formal/flag_skip_selection.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/flag_skip_selection.als "$R/skip-alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
```

Alloy 6.2.0's installed CLI help does **not** support a `--expect` option. Every selected model command instead uses its documented `expect 0` assertion or `expect 1` witness syntax. Explicit `--command '*'` selects every command. The independent receipt validator fails on missing, extra, incorrectly typed or unexpectedly satisfiable/unsatisfiable results rather than trusting the CLI exit status alone. All **13 validator tests passed**, including failure cases.

Actual final receipt: `target/formal-reports/workflow-flags.a196hl/skip-alloy/receipt.json`. Validator output: `skip-alloy-receipt.log`. Backend: **SAT4J**, scope **10**, exactly **7 ordered State atoms**, **1 Int bit width**, hence **6 transitions** in one approval attempt. No arithmetic relies on that bit width.

| Explicit command | Receipt result | Expected |
| --- | --- | --- |
| ApplicationCreate | UNSAT | 0 counterexamples |
| CandidateSemantics | UNSAT | 0 counterexamples |
| OrderAndPruning | UNSAT | 0 counterexamples |
| AtomicPersistence | UNSAT | 0 counterexamples |
| CommittedSelection | UNSAT | 0 counterexamples |
| ExecuteForwarding | UNSAT | 0 counterexamples |
| ResumeUnfiltered | UNSAT | 0 counterexamples |
| ApplicationCreateWitness | SAT | 1 result |
| InvalidCreateWitness | SAT | 1 result |
| AbsentOffWitness | SAT | 1 result |
| OrAndAfterPRWitness | SAT | 1 result |
| GroupPruningWitness | SAT | 1 result |
| SkipCancellationWitness | SAT | 1 result |
| FlagCancellationWitness | SAT | 1 result |
| ExecuteWitness | SAT | 1 result |

Receipt validation passed: **7 UNSAT assertions and 8 SAT witnesses**. Witness solution JSON is retained beside the receipt. InvalidCreateWitness means a trace with an invalid request exists while creation cannot proceed, not that invalid creation succeeds. The unchanged execution-start Alloy model separately passed its **5 UNSAT assertions and 8 SAT witnesses**, with its own receipt.

## Real production Gherkin and regression checks

```sh
cargo +1.99.1 test --locked --all-features --test workflow_flags_bdd
node scripts/formal/check_workflow_flags_bdd_failures.cjs "$R"
cargo +1.99.1 test --locked --all-features --test flags_contract --test flags_cli --test flag_skip_list_cli
cargo +1.99.1 test --locked --all-features --lib --bin cruise-runtime
```

The final real Cucumber execution passed **15 scenarios and 70 steps**. The six added scenarios use actual CLI approval/Execute now entry points through PTYs, actual YAML group/after-pr definitions, actual session files and actual skip history. They cover default filtering, selected flag execution without reprompt, explicit OFF suppression, cancellation of each chooser, and pruning stored history. Definitions are neither parser-only nor mock-only. The runner rejects skipped/undefined/pending steps, false assertions and zero scenarios. Negative controls run against the compiled native runner and must reject undefined steps, deliberately false assertions and empty selection.

The final post-review aggregate completed in **279.08 seconds**, exit **0**. Every recorded check in `results.log` exited **0**. Native undefined-step, deliberately false assertion and empty-filter negative controls each produced the required native rejection and exit **101**, and their supervising check passed. CLI contracts passed: **1** list PTY test, **27** flag CLI tests and **38** shared contract tests. Production targets passed: library **1678 passed, 3 pre-existing ignored**, runtime **2401 passed, 3 pre-existing ignored**, **0 failed**. All nine previously failing fixture tests passed after isolation, in the normal parallel test harness. The ignored existing tests are not counted as passes and no formal or Gherkin check was skipped. No required runner or platform prerequisite remains missing on this local supported platform.

## Post-lint follow-through

After the diagnostic-only test changes and equivalent TUI `clone_from` correction:

```sh
cargo +1.99.1 fmt --all -- --check
cargo +1.99.1 clippy --locked --all-targets --all-features -- -D warnings -A clippy::large_futures
git diff --check
bash -n scripts/check_workflow_flags_specs.sh
```

All four checks passed. The `large_futures` exception is the existing CI command's documented exception, not a new allowance added for this task. Formatter and lint logs are `target/formal-reports/flag-skip-development/final-fmt.log` and `final-clippy.log`. Extracted the list PTY spawn setup into a helper to satisfy the function-size lint without deleting any assertion or interaction.

## Defects found and corrections

- Running the repository's CI Clippy command locally found missing `#[must_use]` on the task's new public pure `skip_candidates` and `drop_disabled_skips` helpers. Added those annotations. Additional affected fixture diagnostics used prohibited `unwrap`/`expect` calls or empty-value assertions. Replaced them with diagnostic `unwrap_or_else` and exact empty-vector equality, without suppressing lints or weakening assertions. Changed the shared application test helper to borrow its flag map and used `clone_from` for the existing TUI default-copy operation, retaining identical values.
- The expanded regression command exposed nine existing test fixtures that inherited user application model defaults: six regeneration fixtures and three TUI config discovery/rendering fixtures. `workflow_call::finish_resolved_config` fills omitted models from AppConfig, so SDK-qualified host models conflicted with the fixtures' command backend. Added fake-home guards under the existing process lock to those test fixtures only. A report-local XDG override alone did not fix the in-process fixture contamination. No production model-selection behavior or expected assertion was relaxed.
- During model development, corrected the resume abstraction to include only unfiltered main expanded IDs, not group parents or after-pr IDs. Added API-create coverage and partial explicit CLI authority. Final production-path review corrected the explicit CLI entry boundary: assignments are already saved before approval, so cancellation retains that baseline. Strengthened `cliAuthority` from preservation of unassigned keys to preservation of the whole saved map and added `explicitCancellationTest`. Fixed a Quint parser error caused by using built-in `keys` as a local identifier. These were specification defects or missing coverage, not changes to weaken safety.
- Preserve the inherited `CRUISE_GUARD_COMMON_DIR` in real BDD and list-PTY child environments while continuing to remove unrelated CRUISE runtime overrides. The full regression also exposed the same context-stripping bug in `tests/agent_auth_cli.rs` and the telemetry subprocess helpers in `tests/telemetry_cli.rs`; their sanitization corrections retain the inherited guard rather than disabling it. No commit guard was disabled or bypassed.

The first post-lint full-suite attempt incorrectly reused `flag-skip-development/xdg-data` from older diagnostics. It failed with **11 runtime errors**, including `(reused)` worktrees pointing at deleted temporary fixture repositories and deliberately malformed leftover session JSON. That run is **not a pass**: its log is `target/formal-reports/flag-skip-development/final-cargo-all-features.log`. The reproducible CI specification runner already allocates fresh state on every invocation. The full-suite retry uses the same fresh-state discipline rather than deleting diagnostics or modifying source assertions.

A subsequent fresh full-suite attempt (`flag-skip-regression.mTLxgI/cargo-all-features.log`) passed library/runtime targets but stopped at the auth fixture's missing guard context. After that correction, `flag-skip-regression.nKu7M4/cargo-all-features.log` passed the auth target's **5 tests** and all preceding integration targets, then stopped at the telemetry fixture's same missing-context error. These full command invocations exited **101**, not zero, and are not reported as full-suite passes. The corrected telemetry target and remaining targets are rerun separately below.

Earlier aggregate runs correctly failed on the nine fixture errors. Two diagnostic native runs were cancelled after macOS process samples showed only `_dyld_start` before Rust entry, including one `/bin/sh` child. Those cancelled diagnostics are not passes. They are retained under `target/formal-reports/flag-skip-development/` and are superseded only by completed checks, not by compilation or a timeout.

## Broader regression follow-through

The broad command actually attempted was `cargo +1.99.1 test --locked --all-features`, with the direct pinned Rustup/Node PATH above and fresh report-local XDG directories. Its final pre-telemetry-fix invocation is preserved at `target/formal-reports/flag-skip-regression.nKu7M4/cargo-all-features.log`. It exited **101** after all earlier targets passed and the telemetry fixture failed. It was not rerun as a single full command after the last telemetry-only sanitization correction. Instead, the corrected target and every remaining target were completed explicitly:

```sh
# Allocate fresh R, never reuse a previous fixture's data directory.
R=$(mktemp -d "$PWD/target/formal-reports/flag-skip-regression-tail.XXXXXX")
export XDG_CONFIG_HOME="$R/config" XDG_DATA_HOME="$R/data" XDG_STATE_HOME="$R/state"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME"
cargo +1.99.1 test --locked --all-features --test telemetry_cli --test tui_conpty_smoke --test tui_pty_smoke --test update_cli --test webui_shutdown --test workflow_call_compat --test workflow_defaults_cli --test workflow_packages_cli
cargo +1.99.1 test --locked --all-features --doc
```

Actual final evidence: `target/formal-reports/flag-skip-regression-tail.3I39Mt/remaining-tests.log` and `doc-tests.log`. This follow-through completed in **46.22 seconds**, exit **0**, following another successful full-target strict Clippy and formatter check.

| Retested or remaining target | Result |
| --- | --- |
| telemetry_cli | 15 passed, 0 failed |
| tui_pty_smoke | 21 passed, 0 failed |
| update_cli | 12 passed, 0 failed |
| webui_shutdown | 2 passed, 0 failed |
| workflow_call_compat | 3 passed, 0 failed |
| workflow_defaults_cli | 16 passed, 0 failed |
| workflow_packages_cli | 13 passed, 0 failed |
| tui_conpty_smoke | 0 tests on macOS, Windows-only coverage not claimed |
| documentation target | 0 existing doctests, not counted as executed cases |

Thus **82 actual remaining integration tests passed**, including the corrected telemetry fixture. The auth fixture's **5 tests** passed in the preceding full run. These partitioned observations are not represented as a zero-exit single full-suite run. No requested formal or Gherkin runner is missing or skipped, no known task-relevant failure remains, and no unsupported Windows verification is claimed.

## CI and unchanged worktree protections

Changed provider-native CI file: **`.github/workflows/ci.yml`**. Its existing read-only, pinned Node/Rust `workflow-flags-specs` job now invokes the expanded `scripts/check_workflow_flags_specs.sh` checks for both model pairs, all strict BDD/failure controls, CLI contracts and production unit targets. The shared `scripts/setup_formal.sh`, npm lock and Cargo pins remain authoritative. The job needs no secrets or provider credentials on pull requests and uploads evidence with `if: always()`. Missing tools, unsupported platforms, setup failures and unexpected receipts all fail closed.

All edits remain **uncommitted**. No source-worktree commits, HEAD moves, local branch-ref moves or commit-guard bypasses were performed. Pre-existing task changes are preserved.
