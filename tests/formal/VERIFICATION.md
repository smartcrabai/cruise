# Local verification evidence, October 23, 2026

## Outcome

The checkpoint formal scope and affected production checks passed locally after diagnosis and repair. **The overall task is not fully verified:** one unrelated GitHub Action fixture assertion is blocked by the protected inherited commit-hook override. It failed twice and was not skipped, weakened or relabeled as passed. Remote GitHub Actions and non-native OS jobs were not executed.

Host: macOS aarch64. Final root HEAD observed: `7ea1509db5e8bd0b3ddbefeadf76ca9a8eed17e3`. All edits remain uncommitted. No root or linked-worktree commits or ref changes were made. The effective inherited guard remains `core.hooksPath=/Users/admin/.local/share/cruise/commit-guard/hooks`.

## Files and contracts

The incoming implementation already changed `.github/workflows/ci.yml`, `Cargo.toml`, `Cargo.lock`, `scripts/setup_formal.sh`, `scripts/check_formal.sh`, `tests/checkpoint_bdd.rs`, `tests/features/checkpoint.feature`, and the model/npm files under `tests/formal/`. Those existing changes were preserved. Cargo lock inspection found Cucumber additions and dependency-name disambiguation, not unrelated version upgrades. The npm lock pins Quint and its transitives, including the declared `adm-zip` override.

This verification step changed:

- `scripts/check_formal.sh`: explicitly select the Apalache backend, retain installed CLI help and Alloy version, and record exact commands plus nested verifier argv.
- `src/tui/app.rs`: fix a confirmed live-run/stale-display palette defect and add deterministic production-connected cancellation regression coverage.
- `tests/formal/README.md` and this report: add contract mappings, commands, assumptions, CI retention and observed evidence.

No model invariant, Alloy assertion/witness, Gherkin assertion, strict-mode setting or meaningful bound was removed or weakened. Neither model required semantic correction. The requirement matrix in `README.md` maps **CP-01 through CP-08** to production paths and existing tests: accepted pair accounting, pending outcomes, checkpoint gating, unbudgeted skip fallback, checkpoint/runtime authority, incompatible-input rejection, group exhaustion/snapshots, stable topology and completed automatic resume. These are reconstructed repository IDs, not recovered original audit labels.

Concrete coverage invokes `build_graph`, `execute_steps_with_graph`, `accept_transition`, `step_loop_iteration`, `save_graph`, `load_graph`, `prepare_resume`, `ExecutionGraph::restore`, runtime/snapshot restoration and application graph display. Fixtures compile actual workflows and execute real shell/file effects with Rust assertions.

## Pinned tools and inspected help

Observed versions: Node `22.22.0`, Quint `0.33.0`, Apalache `0.62.1`, Alloy `6.2.0`, Temurin `25.0.5+8-LTS`, Rust `1.99.1`, and Cargo-locked Cucumber `0.23.0`.

The final runner retained successful `quint typecheck --help`, `test --help`, `run --help`, `verify --help`, and `java -jar target/formal-tools/alloy-6.2.0.jar help exec`. An exploratory `exec --help` invocation was rejected by this Alloy version, so the supported `help exec` syntax was used thereafter. Its help explicitly permits wildcard command selection. Actual `--command '*'` execution printed and produced receipts for all 12 intended commands. No `quint lint` command was invoked.

## Final formal execution

Final reports: **`target/formal-reports/run.5VoCQE/`**, aggregate exit **0**. Its `commands.log` contains expanded paths and all actual command lines. `quint-verify.log` also contains the exact child-process argv, including the isolated endpoint `127.0.0.1:63870`.

The executed operations below are shown with `Q=tests/formal/node_modules/.bin/quint` and `R=target/formal-reports/run.5VoCQE` for readability. The report logs retain their absolute-path expansions:

```sh
"$Q" typecheck tests/formal/checkpoint.qnt
"$Q" test tests/formal/checkpoint.qnt --main checkpoint --backend typescript --seed 20261023 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run tests/formal/checkpoint.qnt --main checkpoint --init init --step step --invariant safety --backend typescript --seed 20261023 --max-samples 100 --max-steps 12 --out-itf "$R/simulation_{seq}.itf.json"
"$Q" verify tests/formal/checkpoint.qnt --main checkpoint --init init --step step --invariant safety --invariants cp01_accounting cp02_pending cp03_checkpoint_gate cp04_skip cp05_resume cp06_reject cp07_group cp08_completed --max-steps 12 --backend apalache --apalache-version 0.62.1 --server-endpoint 127.0.0.1:63870 --out-itf "$R/counterexample.itf.json"
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --type json --output "$R/alloy" tests/formal/checkpoint.als
cargo +1.99.1 test --locked --test graph_state --test checkpoint_bdd
```

Observed results:

- Quint typecheck: exit 0. Named executable tests: **4 passing**.
- Seeded simulation: exit 0, no violation, 100 samples requested, 12 transitions maximum, observed trace lengths 13 states.
- Actual Apalache bounded verification: exit 0, **NoError / No violation found**, through step 12, checking `safety` and all eight named invariants together. This is verification, not substituted simulation.
- Alloy assertions `CP01Accounting`, `CP02Pending`, `CP03CheckpointGate`, `CP04Skip`, `CP05Resume`, `CP06Reject`, `CP07Group`, `CP08Completed`: all **UNSAT, expect 0 passed**.
- Alloy witnesses `RetryResumeWitness`, `SkipCompletionWitness`, `GroupRuntimeWitness`, `GroupExhaustionWitness`: all **SAT, expect 1 passed**.
- Alloy receipt validation: exit 0, with every named command and its declared expectation checked. `alloy.log`, `alloy-expectations.log`, `alloy/receipt.json` and the SAT witness JSON files were inspected.
- Strict native Cucumber: **8 scenarios passed, 24 steps passed**, no undefined/pending/skipped steps. Existing `graph_state` integration tests: **17 passed**.

Bounds: two executable identities plus a terminal marker, one retry pair, one forward pair, one group site, policy/count limits 0..2 and effect/runtime tokens 0..4. Quint checks 12 transitions; Alloy uses 13 ordered states and signed 4-bit integers with SAT4J. Successful saves are abstractly atomic; invalid-input rejection and runtime preservation are abstracted and concretely exercised by Cucumber. There is no arbitrary-topology, unbounded, fairness, fsync/durability or effect-plus-save exactly-once proof. Manual reposition and `usize` overflow have concrete regression coverage rather than a scalar-model proof. See `README.md` for the complete assumptions.

## Production and configuration checks actually run

Logs are under **`target/formal-reports/local-production/`**. `final-commands.log` records the sequential commands. `post-format-formal-static.log` retains the final successful rerun chain. `final-verified-inputs.sha256` identifies the tested source, model, runner, lock and CI inputs.

| Command | Observed final result |
| --- | --- |
| `actionlint .github/workflows/ci.yml` | exit 0 |
| `shellcheck scripts/check_formal.sh scripts/setup_formal.sh` | exit 0 |
| `taplo lint Cargo.toml` | exit 0 |
| `taplo fmt --check --option reorder_keys=true Cargo.toml` | exit 0 |
| `cargo +1.99.1 fmt -- --check` | exit 0 after exact layout repair |
| `bash scripts/verify_vendored_crate.sh` | exit 0, upstream checksum matched |
| `cargo +1.99.1 check --locked --all-targets --all-features` | exit 0 |
| `cargo +1.99.1 clippy --locked --all-targets --all-features -- -D warnings -A clippy::large_futures` | exit 0 |
| `cargo +1.99.1 test --locked --all-features --no-fail-fast` | exit 0, 4,009 passing Rust test results across 30 summary groups, plus 8 Cucumber scenarios / 24 steps |
| `RUSTUP_TOOLCHAIN=1.99.1 python3 scripts/test_update_smoke.py --build --output-dir "$JCODE_SCRATCH_DIR/formal-native-smoke-final-20261023"` | exit 0, real A/B binaries 0.2.16 to 0.2.17, native aarch64-apple-darwin |
| `bash scripts/test_docker_config.sh` | exit 0, 37 passed |
| `bash scripts/test_action_comments.sh` | exit 0, 53 passed |
| `bash scripts/test_action_config_install.sh` | exit 0, 119 passed |
| `bash scripts/test_action_gate.sh` | exit 0, 81 passed |
| `bash scripts/test_action_provider_config.sh` | exit 0, 84 passed |
| `bash scripts/test_action_token.sh` | exit 0, 50 passed |
| `bash scripts/test_action_run.sh` | **exit 1 twice, 186 passed and 1 failed**, protected-hook limitation below |

The six libtest ignored entries in the full run are three existing child-process probes appearing in both library/runtime test binaries. Their parent tests passed and explicitly execute those probes with `--ignored` under isolated PATHs via `src/test_support.rs::run_isolated_test_with_path`. They were not new skips introduced by this work. Windows-only empty test groups are not presented as Windows execution on this macOS host.

The native smoke covered activation, no-op, digest rejection, rollback, four recovery checkpoints, reinstallation fallback, external-install gating and runtime-override gating. Its source copies and binaries were created outside this checkout under the session scratch directory. No repository refs were used for A/B versioning.

## Diagnosed failures and real fix

1. **Initial update capability assertion:** the test expected `cruise-update-test-utils-v1` but received `No update transaction is in progress.` Concurrent default-feature formal builds and all-feature production tests shared `target/debug` and replaced the sibling runtime binary used by integration tests. The capability branch is feature-gated in `src/update_cmd.rs`. Checks were rerun sequentially, and `update_test_capability_query_is_read_only_and_does_not_contact_release_server` passed in the final complete suite. No assertion or feature requirement was weakened. The shared-target sequencing requirement is documented.
2. **Real stale-display production defect:** the sequential full suite exposed `suspended_session_can_be_resumed_through_the_pty`. Its concrete terminal trace showed a live run with a cached Planned action palette: `Run in Worktree` was selected while `Cancel` was appended below. Enter attempted another run, produced `Session already has active work`, and never suspended the existing run. `src/tui/app.rs::open_palette` now treats a live `OperationKind::Run` as authoritative and offers cancellation instead of stale start/resume/mutation actions. The new deterministic `palette_live_run_overrides_stale_phase_and_cancels` test checks four stale phases, actual cancellation-token effects, restoration after claim release and preservation of non-run planning/input capabilities. The existing PTY test was retained unchanged. Both tests passed in the final full suite. This is an additional production UI authority regression, not a claim that the checkpoint models prove all TUI concurrency behavior.
3. **Formatting:** the first post-fix batch reported one rustfmt layout mismatch. The exact layout was repaired before the successful full production test and rebuilt smoke executions. Rustfmt then passed, and the entire formal/BDD/graph suite plus production check/clippy and shell/CI validators were rerun successfully after that repair. The earlier batch's historical aggregate exit 1 remains in `final-results.log`; it is not represented as a successful invocation. The final post-format chain itself exited 0.
4. **Protected-hook fixture limitation:** `test_action_run.sh` repeatedly failed only `co-author hook: prepare-commit-msg itself adds the trailer to a commit with no -m trailer`, missing `Co-authored-by: alice <12345+alice@users.noreply.github.com>`. The Action code installs its fixture coauthor hook via local `core.hooksPath`, but this session injects command-scope `GIT_CONFIG_COUNT=1`, `GIT_CONFIG_KEY_0=core.hooksPath`, `GIT_CONFIG_VALUE_0=/Users/admin/.local/share/cruise/commit-guard/hooks`. That higher-precedence setting prevents the fixture's prepare-commit-msg hook from taking effect. `commit-guard-evidence.log` records the effective setting. The guard was not disabled, bypassed or changed to obtain a green result. This unrelated fixture cannot be validated here under the user's explicit guard-preservation constraint.

## CI artifacts and remaining limitations

The provider-native `formal` job is wired to normal relevant `pull_request` and `push` events to `main`. It fails on any aggregate runner failure. The upload step uses `if: always()`, uploads `target/formal-reports/` as `formal-reports` and retains it for 14 days. Fresh per-run directories retain individual stdout/stderr, exact commands, versions/help, test traces, Alloy receipts/witnesses and verifier logs; counterexample ITF files are retained if actually emitted. Successful runs emitted no counterexample ITF, and none is claimed to exist. Failure upload behavior was inspected in the workflow and validated with actionlint, not observed in a remote CI run.

No remote CI run or remote artifact is claimed. Linux/Windows-specific jobs were not executed locally. The exact remaining local blocker is the protected inherited hook override preventing the one coauthor fixture assertion. The checkpoint formal checks, strict Gherkin, full native production suite, static/configuration checks and rebuilt native smoke otherwise have observed successful final results. Overall verification remains qualified by that blocker.
