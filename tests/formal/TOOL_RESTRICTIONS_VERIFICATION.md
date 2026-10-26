# Tool restriction verification evidence

Executed locally on **October 25, 2026**, macOS arm64, in the requested working repository. All changes remain uncommitted. No HEAD or branch ref was moved and the commit guard was not changed or bypassed.

## Reproduction and retained evidence

The exact top-level invocation was:

```sh
PATH="$HOME/.local/share/mise/installs/node/22.22.0/bin:$PATH" bash scripts/check_tool_restrictions_specs.sh
```

The final, post-fix complete suite ran from **11:31:55 UTC through 11:35:10 UTC**, exited **0**, and created:

```text
target/formal-reports/tool-restrictions.7Dq5KE/
```

`commands.log` contains exact shell-escaped commands. `results.log` records all **25 checks with exit 0** and aggregate status 0. Individual logs retain installed help, versions, Quint traces and Apalache server logs, the Alloy receipt and every SAT witness. See `TOOL_RESTRICTIONS.md` for the complete expanded command list and requirement-to-check mapping. Setup was actually executed using checksum-verified archives, not treated as a successful missing prerequisite. Earlier exploratory runs are not substituted for the final suite.

## Actual versions

| Component | Observed selection |
| --- | --- |
| Node | `v22.22.0` |
| Rust | `rustc 1.99.1 (58cf8f9c3 2026-10-13)` |
| Cucumber | exact Cargo dependency `0.23.0`, locked transitive dependencies |
| Quint | `0.33.0`, npm lock installed via `npm ci --ignore-scripts` |
| Bounded verification backend | Apalache `0.62.1`, explicitly selected, fresh loopback endpoint `127.0.0.1:54902` |
| Java | `Temurin-25.0.5+8-LTS`, OpenJDK `25.0.5` |
| Alloy | released CLI `6.2.0`, SHA256 `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d` |
| Alloy solver | explicitly selected `sat4j` |
| OpenCode CLI | `opencode v2.1.4`, npm-lock-integrity-checked Darwin arm64 native package, postinstall disabled |

## Results

| Check | Observed result |
| --- | --- |
| `quint typecheck tests/formal/tool_restrictions.qnt` | Passed |
| `quint test ... --backend typescript --seed 20261025` | **8 tests passed** |
| `quint run --seed 20261025 --max-samples 1000 --max-steps 4 ...` | No invariant violation, 1000 sampled traces, maximum 5 states including initialization |
| `quint verify --invariant safety --max-steps 4 ... --backend apalache --apalache-version 0.62.1` | **NoError**, no violation through **4 transitions** |
| Alloy `exec --command '*' --solver sat4j --type json ...` | All **13 named commands** executed |
| Independent Alloy receipt check | **5 UNSAT assertions, 8 SAT witnesses**, all expectations satisfied |
| Receipt-validator negative tests | **13 passed**, none skipped |
| Production Cucumber runner | **47 scenarios, 173 steps passed**, no undefined/pending/skipped steps |
| Cucumber failure controls | Undefined step, real policy assertion failure, malformed Gherkin, and empty suite each rejected with actual exit **101**, correct diagnostic and no retry |
| Real pre-tool CLI integration tests | **8 passed** |
| Real config/schema integration tests | **40 passed** |
| Full production library tests | **1701 passed**, 0 failed, **4 existing subprocess-only probe entrypoints ignored** by the parent run |
| Affected OpenCode runtime / SDK / state contracts | **5 + 33 + 6 passed**, none ignored |
| Affected SDK-model Cucumber fixtures | **11 scenarios, 39 steps passed** |
| Real pinned OpenCode CLI acceptance | **2 passed, none ignored**: actual explicit commit denial plus plugin turn/resume/cancellation, using local model servers |

The four existing ignored library entrypoints are `backend::jcode::tests::launch_client_captured_environment_probe`, `pr_merge::tests::inspect_pr_missing_gh_child_probe`, `worktree_pr::tests::test_ensure_gh_available_missing_path_child_probe`, and `worktree_pr::tests::test_ensure_gh_available_nonzero_child_probe`. They are deliberate child-process probes, not missing specification runners. No new specification scenario or assertion is skipped. The parent library tests exercise their corresponding subprocess boundaries.

**Simulation is not proof.** The independent Apalache check is bounded at four transitions. Alloy has exactly five ordered states (four transitions), six fixed restriction identities and seven fixed operation identities. Neither proves the Rust implementation refines the model, arbitrary shell parsing, unrestricted SDK behavior or a security boundary. Scope and enforcement assumptions are explicit in `TOOL_RESTRICTIONS.md`.

## Inspected Alloy receipt

Receipt: `target/formal-reports/tool-restrictions.7Dq5KE/alloy/receipt.json`

Receipt SHA256: `2fe749426207d51b1ad8d91f695f180faf8c4c43e02f130c12986c6bf0b80d58`

```text
DenyOnly                         UNSAT  expect 0 passed
ExactDecisions                   UNSAT  expect 0 passed
McpScope                         UNSAT  expect 0 passed
CommandAdmission                 UNSAT  expect 0 passed
EffectGate                       UNSAT  expect 0 passed
UnionBlockedWitness              SAT    expect 1 passed
SafeShellWitness                 SAT    expect 1 passed
EmptyOverrideWitness             SAT    expect 1 passed
ExplicitSurvivesCommitWitness    SAT    expect 1 passed
RestrictedWitness                SAT    expect 1 passed
AllMcpWitness                    SAT    expect 1 passed
CommandRejectWitness            SAT    expect 1 passed
CommandImplicitIgnoredWitness   SAT    expect 1 passed
```

The installed CLI's actual `help exec` was inspected and retained as `alloy-exec-help.log`. It supports wildcard `--command`, but **does not support `--expect`**. Each model command therefore carries its documented `expect 0`/`expect 1` declaration. The validator checks the actual receipt for every command, not just CLI exit status. Nonempty concrete instances exist for all eight witnesses, so assertion success is not based on an unsatisfiable overall trace contract.

## Repository acceptance checks

These actual commands also passed after the fixes. Clippy is **0.1.99 (58cf8f9c35)**, ShellCheck is **0.11.0**, and Actionlint is **1.7.12**. The `large_futures` setting is the existing CI command's setting, not a new lint exemption. Clippy and formatting remain covered by existing repository CI.

```sh
cargo +1.99.1 fmt --all -- --check
cargo +1.99.1 clippy --locked --all-targets --all-features -- -D warnings -A clippy::large_futures
bash -n scripts/check_tool_restrictions_specs.sh
shellcheck scripts/check_tool_restrictions_specs.sh
actionlint .github/workflows/ci.yml
git diff --check
```

## Changes, defects and prerequisites

Added `tool_restrictions.qnt`, `tool_restrictions.als`, `tests/features/tool_restrictions.feature`, real Rust step definitions, the complete executable check runner, negative controls, Cargo test registration, and traceability/evidence documentation. Reused the repository's already pinned npm/Cargo/JDK/Alloy setup rather than adding another dependency stack.

Changed CI file: **`.github/workflows/ci.yml`**, adding **`tool-restrictions-specs`**. It installs pinned Node/Rust, runs the whole suite on relevant PRs and main pushes without secrets, and always uploads fresh report directories. Existing lint/test CI also sees the registered Rust BDD target. Remote GitHub Actions execution was not triggered from this session.

No production runtime defect was found by the specification checks, and no production behavior was changed. A follow-up run of the repository's strict all-target Clippy command did expose task-related CI lint failures in existing implementation tests and the new BDD target. These were fixed: nine policy defaults now name `ToolPolicy::default()` explicitly in five SDK test files, the 166-line jcode subprocess probe was split into setup and assertion helpers with its imports moved before statements, and empty shell-rule/stdout assertions now print the compared values without weakening the assertion. Cucumber's necessarily owned capture arguments use narrowly scoped, reasoned `#[expect(clippy::needless_pass_by_value)]` attributes following the existing BDD convention. An intermediate stdout equality needed an explicit `u8` vector type because `serde_json` also implements byte/value comparisons. The final strict Clippy command and Rustfmt check passed. Initial and intermediate failed lint logs are retained alongside `clippy-acceptance.log` in the first report directory, `target/formal-reports/tool-restrictions.iPyzTp/`. That initial 20-check suite also passed before lint follow-up. After all fixes and adding the affected SDK regressions to CI, the final **25-check suite above was rerun in full** and passed.

Existing implementation edits present at session start were preserved except for these task-related, behavior-preserving test fixes. During review, the new BDD assertions were made independent of the production implicit-prefix constant, subprocess environment/home/cwd were isolated, and the scenario title was corrected to distinguish resolved workflow policy from loading `default.yaml`. The formal input scope explicitly records existing command-backend permission validation. No invariant or assertion was relaxed to obtain a pass.

**Missing prerequisite: none on the exercised macOS arm64 platform.** The formal runner intentionally fails on unsupported platforms or missing/wrong pinned tools. Linux x64 CI is configured but was not executed locally. Local checks do not stand in for an unobserved remote CI result.
