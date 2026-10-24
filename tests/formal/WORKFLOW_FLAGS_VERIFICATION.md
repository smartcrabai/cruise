# Workflow-flag specification execution, 2026-10-24

## Result and environment

The exact new CI entry point was executed locally from the working repository:

```sh
bash scripts/check_workflow_flags_specs.sh
```

**Final aggregate exit status: 0.** Fresh final evidence directory:

```text
target/formal-reports/workflow-flags.s4FKH7/
```

All 20 recorded checks returned 0. `commands.log` contains exact expanded
commands, `results.log` contains each exit status, and `quint-verify.log`
contains the nested verification argv and isolated backend endpoint. The prior
failed aggregate run is retained at `workflow-flags.yIL0g5`, not reused as
passing evidence.

Executed on **macOS arm64** with these actual versions:

| Component | Observed version |
| --- | --- |
| Node | `v22.22.0` |
| Quint | `0.33.0` |
| Apalache | `0.62.1`, explicitly selected, fresh isolated loopback endpoint |
| Java | Temurin `25.0.5+8-LTS`, OpenJDK 64-bit Server VM |
| Alloy CLI | `6.2.0`, SAT4J solver |
| Rust | `rustc 1.99.1 (58cf8f9c3 2026-10-13)` |
| Cucumber / portable-pty | `0.23.0` / `0.9.0`, existing exact Cargo pins and lock |

For this local invocation, the previously downloaded Node 22.22.0 `bin` from
the adjacent `20261023083156544_0daa2d7292f648da9249e1030a524445` worktree's
`target/formal-tools/node/bin` was prepended to PATH. Only ignored, downloaded
Alloy/JDK archives were copied into this worktree's `target/formal-tools` cache.
The setup script then independently SHA-256 validated both cached artifacts,
extracted Java, checked the runtime version, ran `npm ci --ignore-scripts`,
and checked Quint's version. No cached test result was substituted. npm audit
reported zero vulnerabilities. Exact artifact checksums and reproduction
commands are in `WORKFLOW_FLAGS.md` and `scripts/setup_formal.sh`.

## Quint commands and observed results

Here `Q=tests/formal/node_modules/.bin/quint` and
`R=target/formal-reports/workflow-flags.s4FKH7`:

```sh
"$Q" typecheck tests/formal/workflow_flags.qnt
"$Q" test tests/formal/workflow_flags.qnt --backend typescript --seed 20261024 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261024 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$R/simulation_{seq}.itf.json" tests/formal/workflow_flags.qnt
```

Typecheck passed. All **12 executable tests** passed, with no failed or ignored
tests: selection, empty selection, explicit off, dry run, batch, resume,
nonterminal, no declarations, keeping defaults, planned cancellation, exec
cancellation, and preflight rejection.

The seeded bounded simulation found no violation with **seed 20261024**,
**1000 samples** and **8 steps per sample**. **Simulation is not proof.**

The runner additionally invoked the real verifier, via the existing helper:

```sh
node scripts/formal/verify_quint.cjs "$Q" tests/formal/workflow_flags.qnt workflow_flags 8 "$R/counterexample.itf.json"
```

The runner supplies absolute paths to the helper and runs it inside `R`. Its
actual final verification argv selected endpoint `127.0.0.1:54842`:

```sh
quint verify --invariant safety --max-steps 8 --main workflow_flags --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint 127.0.0.1:54842 --out-itf "$R/counterexample.itf.json" tests/formal/workflow_flags.qnt
```

Apalache returned **`NoError`**, exit 0, no invariant violation through **8
transitions**, including state 8. `safety` is the conjunction of `promptGate`,
`explicitSelection`, `cliAuthority`, `persistence` and `domains`. The bound is
not an unbounded proof, nor a proof that every Rust execution refines the model.

## Alloy commands and inspected receipt

The runner first executed and retained `java -jar ... help exec`, `version`
and `solvers`, checking the installed release's syntax rather than defaults.
Alloy 6.2.0 has no CLI `--expect` flag. Its supported `expect 0` and `expect 1`
are explicit on every model command and independently compared to the receipt.
All commands were selected, not just the default command:

```sh
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/workflow_flags.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/workflow_flags.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
```

Inspected receipt:
`target/formal-reports/workflow-flags.s4FKH7/alloy/receipt.json`.
Scope: **exactly 9 ordered states**, **8 transitions**, **5-bit signed
integers**, all three fixed flag atoms. Every witness includes actual lifecycle
progress, not an unconstrained empty structure.

| Receipt command | Observed result | Required expectation |
| --- | --- | --- |
| PromptGate | UNSAT, no counterexample | 0 |
| ExplicitSelection | UNSAT, no counterexample | 0 |
| CliAuthority | UNSAT, no counterexample | 0 |
| Persistence | UNSAT, no counterexample | 0 |
| Domains | UNSAT, no counterexample | 0 |
| SelectionWitness | SAT, concrete instance emitted | 1 |
| EmptyWitness | SAT, concrete instance emitted | 1 |
| ExplicitOffWitness | SAT, concrete instance emitted | 1 |
| CancellationWitness | SAT, concrete instance emitted | 1 |
| ExecCancellationWitness | SAT, concrete instance emitted | 1 |
| PreflightWitness | SAT, concrete instance emitted | 1 |
| SuppressedWitness | SAT, concrete instance emitted | 1 |
| DryWitness | SAT, concrete instance emitted | 1 |

Receipt validation passed: **5 UNSAT assertions, 8 SAT witnesses**. All **13
receipt-validator unit tests** passed, including missing/malformed results,
unexpected satisfiability and absence of assertions/witnesses. SAT instance
JSON files are retained alongside the receipt.

## Production Gherkin and regression checks

```sh
cargo +1.99.1 test --locked --all-features --test workflow_flags_bdd
node scripts/formal/check_workflow_flags_bdd_failures.cjs "$R"
cargo +1.99.1 test --locked --all-features --test flags_contract --test flags_cli
cargo +1.99.1 test --locked --all-features --bin cruise-runtime workflow_flag_tests
```

* **9 Gherkin scenarios / 39 steps passed**, no undefined, pending or skipped
  production scenarios. Real launcher/runtime, inquire in a PTY, real shell
  effects, real on-disk planned sessions and a genuinely interrupted/resumed
  shell step were used. No external model backend or mock engine was used.
* Negative controls all passed: undefined step, intentionally false assertion
  after real nonterminal execution, and an empty name filter each produced the
  expected native failure and **exit 101**. No scenario rerun, timeout, compiler
  error or missing platform was accepted as success.
* **22 CLI tests passed**, including exec prompt/suppression, ExecuteNow flag
  forwarding, force_exec plan/root/background paths, cancellation, no usable
  plan, sparse saved defaults and nonterminal behavior.
* **38 flag contract tests passed**, including undeclared-key preservation,
  explicit off, engine effects, validation and session round trips.
* **16 run-boundary unit/integration tests passed**, including each eligibility
  gate and selected flag values reaching flag-dependent preflight validation.

Additional repository checks:

```sh
bash -n scripts/check_workflow_flags_specs.sh
cargo +1.99.1 fmt -- --check
git diff --check
cargo +1.99.1 clippy --locked --all-targets --all-features -- -D warnings -A clippy::large_futures
```

All passed. Clippy uses the repository's existing CI policy for `large_futures`,
not a newly introduced exemption. The complete unrelated Cargo test suite was
not run by this task. All new checks and the affected production contracts were.

## Changes, fixes and limits

CI changed: **`.github/workflows/ci.yml`**, adding the independent read-only
`workflow-flags-specs` job. It installs pinned Node/Rust, calls the exact suite
above, and always uploads fresh `workflow-flags.*` evidence for 14 days. It has
no secret-dependent PR condition. Normal relevant PR/push triggers are reused.
CI execution on GitHub and branch-protection settings were not changed or
claimed as verified locally.

Added models, Gherkin feature/runner, negative controls, aggregate script,
traceability/reproduction documentation and Cargo test registration. Existing
pinned dependencies and installers were reused without redundant lockfile
changes. The eight pre-existing modified implementation/documentation/test
files were preserved. This specification task made no additional production
source changes.

Genuine specification defects fixed during execution: draft actions/parameters
used Quint built-in names `select`, `next`, and later `keys`. They were renamed
to `choose`, `updated`, and `storedKeys` respectively. The first integrated run
correctly failed all four Quint checks on the `keys` parse error while retaining
passing independent checks. After the fix, the **entire aggregate runner was
rerun and passed**. No safety invariant or Alloy assertion was weakened to get
a pass. Prompt accounting was aligned with display at prepare, and the model's
initial domain was expanded to include saved explicit false keys before the
final run. No new production defect was found by this suite.

There is **no concrete missing prerequisite** for the completed local checks.
Linux execution is wired into CI but was not locally exercised on this macOS
host. Unsupported platforms fail explicitly. The finite abstraction and bounds
are documented in `WORKFLOW_FLAGS.md`; no provider, arbitrary topology,
crash-durability, liveness or unbounded proof is implied.

All source edits remain **uncommitted**. No source-worktree commit, HEAD/ref
movement or commit-guard bypass was performed. Gherkin creates commits only in
its disposable fixture repositories, as required by the existing real CLI
execution convention, never in the working repository or linked worktrees.
