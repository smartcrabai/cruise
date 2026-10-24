# OpenCode effort specification execution report

Execution date: **2026-10-24**, local macOS arm64. All edits are uncommitted.
The existing task implementation was already dirty when this specification work
started and was preserved. No commits, HEAD moves, branch-ref updates, or commit
guard changes were performed in this worktree or linked worktrees.

## Reproduction and retained evidence

Run from the repository root:

```sh
export PATH="$HOME/.local/share/mise/installs/node/22.22.0/bin:$PATH"
bash scripts/check_opencode_effort_specs.sh
```

The PATH line describes this machine's installed Node location, not a CI
requirement. CI installs the pinned version directly. The complete successful
aggregate run is `target/formal-reports/opencode-effort.t71rTa/`:

- `commands.log`: every exact executed command, shell-escaped, including resolved
  executable paths. `quint-verify.log` also records the precise underlying
  `quint verify` argv and fresh backend endpoint.
- `results.log`: all 24 stages exited zero, aggregate exit status zero.
- CLI help/version logs, eight executable Quint test traces, the simulation
  trace, Apalache's `_apalache-out/` logs, Alloy receipt and witness instances,
  and concrete Rust/Cucumber/CLI execution logs are retained in that directory.

The command listing, dependency pins, production-path traceability and model
limitations are in [OPENCODE_EFFORT.md](OPENCODE_EFFORT.md). In particular the
commands actually executed include:

```sh
Q=tests/formal/node_modules/.bin/quint
"$Q" typecheck tests/formal/opencode_effort.qnt
"$Q" test tests/formal/opencode_effort.qnt --backend typescript --seed 20261024
"$Q" run --seed 20261024 --max-samples 1000 --max-steps 12 --invariant safety --backend typescript tests/formal/opencode_effort.qnt
# The wrapper supplies the absolute paths, fresh PORT and counterexample output:
"$Q" verify --invariant safety --max-steps 12 --main opencode_effort --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json" tests/formal/opencode_effort.qnt
java -jar target/formal-tools/alloy-6.2.0.jar help exec
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/opencode_effort.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/opencode_effort.als "$R/alloy/receipt.json"
cargo +1.99.1 test --locked --all-features --test opencode_bdd
env OPENCODE_REAL_CLI="$OPENCODE" cargo +1.99.1 test --locked --all-features --test opencode_real_cli -- --ignored --nocapture
```

`R`, `PORT` and `OPENCODE` are provided by the aggregate runner. Its actual test
and simulation invocations also retain `--out-itf` traces. See `commands.log` for
those full expanded commands, and the companion document for every supporting
unit/integration/negative-control command.

## Observed toolchain

| Component | Pin and observed runtime |
| --- | --- |
| Node / npm | 22.22.0 / 10.9.4 |
| Quint | 0.33.0, npm integrity-locked |
| Bounded verification backend | Apalache 0.62.1, build a321771 |
| JDK | Eclipse Temurin 25.0.5+8-LTS, OpenJDK 64-bit Server VM |
| Alloy | Released CLI JAR 6.2.0, SHA256 `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d` |
| Alloy solver | Bundled SAT4J |
| Rust / Cargo | 1.99.1, rustc `58cf8f9c3` (2026-10-13) |
| Cucumber | 0.23.0, existing locked stack-native runner |
| OpenCode | Official `@opencode/cli` 2.1.4 and native platform archive, npm SHA512 integrity-locked, actual output `opencode v2.1.4` |

Existing `scripts/setup_formal.sh` pins and validates JDK archive SHA256 for both
supported platforms. The new isolated OpenCode dependency setup uses
`npm ci --ignore-scripts` and directly selects the pinned native binary, avoiding
the wrapper package's network-fallback installer. The aggregate verifies exact
Node, JDK runtime, Quint and OpenCode versions before checking the contracts.

## Quint result and bounds

- `typecheck`: passed.
- `test`: **8 passing**, covering canonical selection, resumed metadata alias,
  ambiguous rejection, catalog API failure, native bypass, fallback rechecking,
  replay barrier, and permanent authentication failure.
- Seeded simulation: **seed 20261024, 1000 samples, at most 12 steps**, safety
  invariant held. This is testing, not proof.
- Bounded verification: `safety`, **12 transitions**, **Apalache 0.62.1**,
  `NoError`, `No violation found`.

The shared verifier's output-directory fix was additionally regression-tested
against existing `opencode.qnt` at 14 transitions and `sdk_models.qnt` at 12
transitions. Both returned `NoError`. Logs are in
`target/formal-reports/effort-regressions/{opencode,sdk_models}-verify.log`.
Neither these results nor the new model claim unbounded proof or Rust refinement.

## Alloy receipt

The pinned CLI's actual `help exec` was inspected and saved. Alloy 6.2.0 has no
`--expect` CLI switch. Each explicitly selected model command instead declares
its supported `expect 0` or `expect 1` expectation. The runner uses
`--command '*'` and validates all receipt entries and concrete SAT instances,
not just the process exit code.

Receipt: `target/formal-reports/opencode-effort.t71rTa/alloy/receipt.json`.
Scope: **exactly 13 ordered states, 12 transitions, 4-bit signed integers**.

| Command | Expected | Actual |
| --- | --- | --- |
| check CapabilitySafety | 0 counterexamples | UNSAT |
| check ReplayAndFallbackSafety | 0 counterexamples | UNSAT |
| run CanonicalPriorityWitness | 1 result | SAT |
| run ResumeAliasWitness | 1 result | SAT |
| run RejectedResumeWitness | 1 result | SAT |
| run ApiFailureWitness | 1 result | SAT |
| run NativeBypassWitness | 1 result | SAT |
| run FallbackRechecksWitness | 1 result | SAT |
| run BarrierWitness | 1 result | SAT |
| run AuthWitness | 1 result | SAT |

The receipt validator confirmed **2 UNSAT assertions and 8 SAT witnesses**.
Its **13 tests** passed, including malformed/missing/unexpected receipt failures.
The fallback witness requires both submissions, separate capability lookups,
changed wire variant, primary cooldown and success on the spare, rather than an
empty initial-state witness.

## Production execution

| Check | Observed result |
| --- | --- |
| Cucumber production Executor + owned subprocess HTTP/SSE fixtures | **37 scenarios, 169 steps passed**; 25 task-specific examples added |
| Actual executable undefined-step negative control | Expected failure exit 101 at the undefined step, validated |
| Actual executable assertion-failure negative control | Expected failure exit 101 at the assertion step, validated |
| OpenCode unit contracts | **12 passed** |
| Retry unit contracts | **64 passed** |
| SDK CLI integration | **33 passed** |
| Config/runtime/state contract integration | **13 / 5 / 6 passed** |
| Pinned real OpenCode CLI loopback model server, provider effort, plugin, turn, resume, cancel | **1 passed**, explicitly runs the normally ignored test |

Cucumber does not merely parse features: it executes production entry points and
checks emitted selections, prompt counts, catalog counts and fallback notices.
It fails on skipped/undefined steps and assertions, with retries disabled. No
provider credentials or paid API calls were used.

## Defects fixed and regression checks

No production behavior defect was discovered or changed during this specification
pass. Existing dirty production changes implement the plan. New coverage and
supporting fixes were:

1. Shared Quint verifier wrote Apalache scratch/log files into the repository
   root. Resolve all file arguments and run in the report directory, retaining
   evidence with the correct CI artifact. Both existing models were reverified.
2. Newly exposed BDD fixture helpers triggered strict dead-code lint when the
   shared fixture was compiled as its standalone integration target. Existing
   standalone assertions now use those helpers, preserving all expectations.
3. Broad regression testing exposed ambient configuration leakage in two older
   tests: the resolver's explicit prompt-file entry point and the application's
   invalid-config reload test inherited unrelated SDK model defaults. Initial
   all-feature runs failed those tests; cleaning XDG configuration alone still
   left a resolver failure. The fixtures now use the existing process lock,
   fake-home and model-environment guards, with an isolated working directory
   for the resolver. Original assertions are unchanged. No user configuration or
   production configuration semantics were changed.

Final static and lint checks passed:

```sh
git diff --check
cargo +1.99.1 fmt --all -- --check
cargo +1.99.1 clippy --locked --all-targets --all-features -- -D warnings -A clippy::large_futures
actionlint .github/workflows/ci.yml
shellcheck scripts/check_opencode_effort_specs.sh
bash -n scripts/check_opencode_effort_specs.sh
```

**Repository-wide required-check parity remains blocked.** The normal command
`cargo +1.99.1 test --locked --all-features` passed the library (**1612 passed,
3 ignored**) and launcher (**9 passed**), then failed in `cruise-runtime`
(**2290 passed, 9 failed, 3 ignored**). The two isolated library fixtures now
pass. Evidence: `target/formal-reports/effort-regressions/clippy-final.log` and
`cargo-test-all-final.log` in that same directory.

An additional whole-suite rerun with an empty XDG configuration directory and
inherited model overrides removed produced the same nine runtime failures:

```sh
mkdir -p target/formal-reports/effort-regressions/clean-config-final
env -u CRUISE_MODEL -u CRUISE_PLAN_MODEL -u CRUISE_SDK XDG_CONFIG_HOME="$PWD/target/formal-reports/effort-regressions/clean-config-final" cargo +1.99.1 test --locked --all-features
```

Its log is `target/formal-reports/effort-regressions/cargo-test-all-clean-final.log`.
The remaining failing tests are:

- `plan_cmd::tests::test_regenerate_plan_for_session_awaiting_approval_stays_awaiting_approval`
- `plan_cmd::tests::test_regenerate_plan_for_session_awaiting_input_transitions_to_awaiting_approval`
- `plan_cmd::tests::test_regenerate_plan_for_session_clears_plan_error_on_success`
- `plan_cmd::tests::test_regenerate_plan_for_session_draft_transitions_to_awaiting_approval`
- `plan_cmd::tests::test_regenerate_plan_for_session_planned_stays_planned`
- `plan_cmd::tests::test_regenerate_plan_for_session_refreshes_title_from_plan`
- `tui::app::tests::valid_config_path_is_resolved_before_leaving_config_step`
- `tui::ui::tests::config_candidate_list_scrolls_to_the_selected_late_entry`
- `tui::ui::tests::config_step_shows_auto_and_cli_candidates_at_minimum_and_wide_sizes`

The six planner fixtures fail with `command cannot be combined with
SDK-qualified model candidates`. The TUI failures are respectively Config
instead of Task, missing `config-09.yaml`, and missing the CLI candidate at width
80. Their complete root causes were not established in this task. Those planner
and TUI paths were not modified, their assertions were not changed or skipped,
and the broad run's failure is not counted as a pass. Cargo stops at this failing
binary target, so this is not a claim that later full-suite targets executed.
All task-specific integration targets were executed independently by the new
aggregate runner and passed. Investigation of these nine unrelated planner/TUI
regressions is left outside the effort specification scope.

After the fixture fixes, the entire new specification runner was rerun against
the final model/source files: `opencode-effort.t71rTa`, all 24 stages passed.

## CI integration and prerequisites

Changed provider-native CI file: **`.github/workflows/ci.yml`**. New job:
**`opencode-effort-specs`**, Ubuntu, timeout 30 minutes, read-only contents
permission, pinned Node/Rust, lock-aware caches, one aggregate command, and
always-uploaded execution evidence for 14 days. It runs on the workflow's existing
PR and main-push triggers, without secrets. The existing full test and lint jobs
also cover the two hermetic fixture fixes. Models, witness commands, receipt
validation, BDD negative controls, production tests and real CLI smoke all run
within the new job. There is no parser-only or dry-run substitute.

**Missing local prerequisites: none.** Pinned dependencies were installed and
executed successfully on macOS arm64. Linux installation/selection and GitHub
workflow syntax are configured and checked, but a remote GitHub Actions run has
not been triggered here. This report does not claim remote CI success or that
repository branch-protection settings require the new job. Unsupported platforms
and missing tool/backend/binary downloads cause a nonzero failure, never a pass.
