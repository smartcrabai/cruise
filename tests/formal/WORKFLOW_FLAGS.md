# CLI workflow-flag execution boundary

## Authority and traceability

This suite implements the 2026-10-24 planning-to-execution task in `plan.md`,
sections 1, 2.1, 2.2, 2.4, 2.5 and 2.6. Approval-time flag selection is a
separate UI boundary outside these execution models, as are TUI/WebUI, history,
background planning workers and batch selection. Execute now forwards its
approval-time choices as explicit CLI assignments, using the modeled
`cliAuthority` suppression path rather than prompting again at execution start.
Selection is per invocation, not a new persisted confirmation bit.

| Contract from the plan | Production authority | Quint / Alloy | Executed production checks |
| --- | --- | --- | --- |
| Ask once only for a fresh, single, non-dry, TTY invocation with declarations and no explicit CLI assignment | `run_cmd::should_prompt_flags`, `prompt_flag_values`, `run_single_loaded` | `promptGate` / `PromptGate`, suppression tests and witnesses | Gherkin interactive, explicit CLI, nonterminal, dry-run and real interrupted/resumed scenarios; `workflow_flag_tests` covers batch and no declarations |
| Saved enabled flags are the initial selection; Enter keeps them | `prompt_flag_values` default indices | `keepDefaultsTest`, `selectionTest`, `SelectionWitness` | Gherkin Enter retains saved defaults |
| A selection writes true/false for every declared key, retaining undeclared keys including explicit off | `flags::apply_enabled` | `explicitSelection` / `ExplicitSelection`, empty-selection test/witness, sparse saved input domain | Gherkin selected/empty state assertions plus `flags_contract` tests |
| Any `-f`, including explicit off, wins and suppresses execution-start selection; Execute now forwards approval-time choices as explicit CLI assignments | `run_single_loaded` CLI merge, `plan_cmd` ExecuteNow forwarding, `exec_cmd::run_resolved` forwarding | `cliAuthority` / `CliAuthority`, `explicitOffTest` / `ExplicitOffWitness` | Gherkin explicit CLI; `flags_cli` PTY cases for exec, ExecuteNow and force_exec plan/root/background paths |
| Reject invalid config, unusable plan, retry/compile/resume failures before asking | prepare ordering in `run_single_loaded` | abstract `valid` preflight gate and rejection test/witness | `flags_cli::run_without_usable_plan_fails_before_flag_selection`; unit selected-flags preflight validation test |
| Cancellation leaves planned session unchanged and disposes owned ephemeral exec | `prompt_flag_values`, `run_with_operation`, `dispose_exec_session_if_owned` | `persistence` / `Persistence`, cancellation tests and witnesses | Gherkin byte-for-byte state preservation, no shell effects, diagnostic and directory cleanup assertions |
| Persist selected values through existing workspace save, then pass them to engine | `run_single_loaded` workspace save and engine flag_values | `persistence` / `Persistence`, selection test/witness | Gherkin checks `state.json` and actual shell files; unit integration checks |

The models describe observable lifecycle stages, displayed prompt count, working
flag values, explicitly present keys, persisted values, session existence, and
values passed to execution. They do not duplicate Rust data structures or graph
compilation algorithms. Two declared flags and one undeclared saved key are the
finite representative domain. Both sparse absence and explicitly saved false
are represented. Input `fresh` means `current_step.is_none()`, and `single`
means `WorkspaceOverride::RespectSession`, not absence of saved flag settings.
`cli` is the set of explicitly assigned names even when `cliOn` is empty.

A single modeled invocation proceeds from preparing to prompting or ready,
then selection/cancellation, or from ready to successful workspace persistence
and engine entry. Invalid preflight rejects without a prompt. `idle` pads short
traces to the bound. Successful workspace setup, flag-dependent graph validation
and save are abstracted as the successful `start` action. Their failure modes
and actual file effects are not proved by the models. `valid` abstracts only
validation that precedes flag selection, not flag-aware graph validation.
No provider calls, topology proof, liveness, crash durability, arbitrary flag
cardinality, unbounded verification or Rust refinement proof is claimed.

## Reproduce with the repository pins

From the repository root, with Node **24.22.0** on PATH and Rust **1.99.1**
installed:

```sh
bash scripts/check_workflow_flags_specs.sh
```

This reuses `scripts/setup_formal.sh`, `tests/formal/package.json` and its
committed integrity-pinned lockfile. No second dependency installation scheme
is introduced. Exact tools:

* Quint **0.33.0**, npm `@informalsystems/quint`, locked transitive dependencies.
* Apalache **0.62.1**, explicitly requested by `verify_quint.cjs`, on a fresh
  loopback port so an unrelated server cannot satisfy verification.
* Alloy CLI **6.2.0**, released `org.alloytools.alloy.dist.jar`, SHA-256
  `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`.
* Temurin JDK **25.0.5+8-LTS**, runtime checked after archive extraction.
  Darwin arm64 archive SHA-256:
  `36dc14d797aaff6d89aab302cd053fdba6207dfdcf772052fa7b023a94b0e857`.
  Linux x64 archive SHA-256:
  `68982529869257d0ee6b824ec5aa270ff8b118bb438670da0b97ebdab178d7fb`.
* Rust **1.99.1**, Cucumber **0.23.0**, portable-pty **0.9.0**, tempfile
  **3.27.0**, with root `Cargo.lock` and `--locked --all-features`.

The setup script holds the released download locations and validates checksums
on every run, including cached artifacts. CI installs exact Node and Rust, and
setup installs Java and npm tools. No secrets, provider account, local agent CLI
or network model calls are used by the scenarios. The supported formal platforms
are macOS arm64 and Linux x64. Missing tools or unsupported platforms fail.

`Q=tests/formal/node_modules/.bin/quint`, `R` is a fresh evidence directory, and
`PORT` is the dynamically reserved endpoint. The runner records installed help
for every Quint command, Alloy `help exec`, tool versions and solver inventory.
Actual check syntax:

```sh
"$Q" typecheck tests/formal/workflow_flags.qnt
"$Q" test tests/formal/workflow_flags.qnt --backend typescript --seed 20261024 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261024 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$R/simulation_{seq}.itf.json" tests/formal/workflow_flags.qnt
"$Q" verify --invariant safety --max-steps 8 --main workflow_flags --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json" tests/formal/workflow_flags.qnt
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/workflow_flags.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/workflow_flags.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test workflow_flags_bdd
node scripts/formal/check_workflow_flags_bdd_failures.cjs "$R"
cargo +1.99.1 test --locked --all-features --test flags_contract --test flags_cli
cargo +1.99.1 test --locked --all-features --bin cruise-runtime workflow_flag_tests
```

The verification helper logs the expanded absolute argv and executes within
`R` so Apalache auxiliary files remain in the evidence directory.
**Simulation is not proof:** seed **20261024**, **1000** samples, **8** steps.
Separate symbolic verification checks `safety` through **8 transitions** with
**Apalache 0.62.1**. Alloy checks **exactly 9 ordered states**, hence **8
transitions**, with **5-bit signed integers**. Reachable counts are 0 or 1.
Five assertion commands explicitly require `expect 0`; eight meaningful
witness commands explicitly require `expect 1`. Alloy 6.2.0's actual CLI has
**no `--expect` option**, so these supported per-command expectations are
independently checked against every command in its receipt. Missing, malformed,
duplicate, extra, wrong-type and unexpected SAT/UNSAT results fail the runner.

## Gherkin and CI failure behavior

`tests/workflow_flags_bdd.rs` uses the repository's native Cucumber convention.
It invokes the real launcher/runtime and real session storage. PTY input drives
actual inquire selection and cancellation. Shell commands produce real files.
Resume is created by interrupting a running shell step, not just editing a
position field. Fixtures use isolated temp directories and fixture-only git
repositories, never source-worktree refs. Unsupported platforms panic rather
than pass a zero-scenario check.

The runner uses `fail_on_skipped`, fails on assertion/parser/hook errors,
checks executed-step statistics and serializes scenarios. Negative controls
execute an undefined step, an intentionally wrong production-output assertion,
and an empty filter. They require native exit 101 and expected diagnostics,
not timeout/compiler/platform failure. Cucumber has no pending step definitions
here. Any future skipped/pending result is rejected by `fail_on_skipped`.

`.github/workflows/ci.yml` adds the independent **workflow-flags-specs** job for
normal relevant PRs and pushes to main, with read-only contents permission and
no secret-dependent condition. Every check above runs in the job, including
receipt validator unit tests and negative controls. Independent checks continue
after failures, but the aggregate exit is nonzero. Reports always upload from
`target/formal-reports/workflow-flags.*/` for 14 days, including failures.
No remote CI result or branch-protection configuration is claimed from local
execution. See `WORKFLOW_FLAGS_VERIFICATION.md` for actual dated results.
