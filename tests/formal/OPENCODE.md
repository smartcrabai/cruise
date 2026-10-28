# OpenCode V2 executable contract

These specifications are scoped to the owned attempt's observable admission,
completion and retry behavior from the OpenCode implementation plan dated
October 23, 2026, sections 4.3, 5.2, 5.3 and 5.5. They do not replace the existing
checkpoint specifications or claim to cover every OpenCode API.

## Requirements and production grounding

| Contract | Plan authority | Production path | Quint / Alloy | Concrete checks |
| --- | --- | --- | --- | --- |
| Persist/acknowledge the session and subscribe before submitting input | 5.2 steps 6-7, 4.3 attempt guard | `src/backend/opencode/mod.rs::run_attempt`, `execute`, `src/executor.rs::stream_to_outcome` | `admission` / `Admission`, rejected-checkpoint witness | Cucumber rejected checkpoint and disabled-plugin cases, `opencode_sdk_cli` checkpoint tests |
| Only matching execution success is completion, never HTTP acceptance, text-ended, foreign events or EOF | 5.3 | `src/backend/opencode/events.rs::Turn::apply`, `mod.rs::execute`, `reconcile` | `completion` / `Completion`, success witness | Cucumber complete, disconnect, incomplete and malformed cases assert exact output or failure |
| Never replay uncertain acceptance, missing terminal evidence or an input that caused tool effects, even without a stdout callback | 5.5 | `mod.rs::stream_agent`, `run_attempt`, `tools.rs::Bridge`, `src/backend/stream.rs::ChunkReducer`, `src/executor.rs::run_with_fallback` | `retrySafety` / `RetrySafety`, silent-effect and uncertainty witnesses | Cucumber observes exactly one actual HTTP prompt with two retry opportunities and fallback configured |
| Proven safe provider failures may use shared retry/fallback only after prior attempt cleanup | 4.3, 5.5 | `mod.rs::run_attempt`, `runtime.rs::Runtime::stop`, `src/executor.rs::run_with_fallback` | `retrySafety`, `completion`, safe-fallback witness | Cucumber safe 429 and provider-model 404 cases assert two prompts and exact answer |
| Cancellation and terminal notification await owned CLI cleanup | 4.3, 5.2 step 9 | `mod.rs::run_attempt`, `runtime.rs::Runtime::stop`, `src/executor.rs::stream_to_outcome` | `completion` and cancellation witness | Cucumber cancellation and every terminal case assert the child PID has been reaped |

Gherkin lives in `../features/opencode.feature`. Rust Cucumber definitions in
`../opencode_bdd.rs` call the real `Executor::run(PromptRun)` using the local HTTP/SSE
and owned POSIX CLI fixture in `../opencode_sdk_cli.rs`. Only the provider/CLI is
substituted, not the Executor, HTTP client, event reducer, plugin bridge, retry
engine or cleanup code. The fixture's real token-authenticated bridge records a
tool effect without visible answer text. Exact expected answers reject duplicate,
foreign-session, past-turn, reasoning and tool-result contamination. The runner
uses `fail_on_skipped()` and `run_and_exit` for assertion and undefined/pending
step failures, with scenario retries explicitly disabled. Failure scenarios
also assert their specific diagnostic category, so an unrelated startup error
cannot satisfy them. The `incomplete` case delivers full text and successfully
synchronizes the durable log without execution terminal evidence. The separate
`log-unavailable` case fails reconciliation with HTTP 503. Both must fail with
one input despite configured retry/fallback opportunities.
Two deliberate negative features in `../fixtures/opencode_specs` are run by
`scripts/formal/check_bdd_failures.cjs`. They require an undefined step and a
wrong prompt-count assertion through the real Executor to exit 101 with one
failed step and no scenario rerun. A missing executable or build failure does
not satisfy these controls. Their raw failure logs are retained with the report.
Independent black-box `cruise exec -c ...` coverage remains in
`opencode_sdk_cli` and is included in the specification runner.

## Model boundaries

`opencode.qnt` and `opencode.als` describe one logical prompt request with at most
two attempts and one representative external tool effect. The state includes
phase, attempt/prompt counts, owned-runtime lifetime, checkpoint acknowledgment,
subscription, retry barrier, terminal evidence, effect count and outcome. Events
and the previous state are observer instrumentation. Start, checkpoint,
subscribe, submit, effect, resolve, stop, publish and irrelevant-event actions
correspond to observable boundaries, rather than copying the implementation's
HTTP branches or private data structures.

Setup abstracts the validated V2/plugin/environment configuration. A success
transition represents a matching execution success after the current input's
delivery, from live SSE or reconciled durable log. An irrelevant-event step
represents filtered foreign/old events, nonterminal text/idle, or recoverable
transport loss before terminal evidence. Recovery never submits another prompt.
The uncertainty action includes lost acceptance, unrecoverable log and local
protocol/setup rejection. Safe provider failure abstracts only failures the
existing shared retry policy permits, with no tool effect or partial answer.
There is no partial-text state, so the model does not authorize retries after
partial output. Effects set a monotonic barrier. Stop abstracts successful
completion of CLI, bridge and storage cleanup before publishing or retrying.

The bound is **14 transitions**, including complete safe fallback to a second
attempt. Alloy has **exactly 15 ordered states**, signed **5-bit integers**, and
idle padding for shorter traces. No reachable arithmetic overflows this domain.
Four assertions require `expect 0`; six meaningful witnesses require `expect 1`.
The uncertainty and active-cancellation witnesses require the transition from an
already submitted input in the first attempt. Checkpoint rejection requires the
setup boundary with no acknowledged checkpoint. Merely reaching a compatible
final failure on a later attempt does not satisfy these witnesses.
Six corresponding Quint tests cover success, silent effect failure, safe fallback,
uncertainty, checkpoint rejection and cancellation. No fairness/liveness,
unbounded proof, full protocol refinement, process-tree/OS isolation proof or
crash-exactly-once guarantee is asserted. Concrete process behavior is tested,
not proved by the abstract `alive` flag.

## Reproducible tools and commands

Reuse the existing formal setup and lockfiles, without adding a production
Node/Java dependency. Node **24.22.0**, Quint **0.33.0** (npm lock), Apalache
**0.62.1**, Temurin JDK **25.0.5+8-LTS**, Alloy **6.2.0**, Rust **1.99.1** and
Cucumber **0.23.0** are selected. `scripts/setup_formal.sh` validates the released
Alloy JAR SHA256
`6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`
and platform-specific JDK archive hashes, installs Quint with `npm ci
--ignore-scripts`, and validates runtime versions. The backend version is explicit
on every verification invocation, with an isolated loopback endpoint to avoid
silently attaching to an existing different backend. The setup supports macOS
arm64 and Linux x64. The dedicated runner fails with `BLOCKED` elsewhere rather
than treating unavailable POSIX BDD or formal tools as passing checks.

From the repository root with the pinned Node on PATH:

```sh
bash scripts/check_opencode_specs.sh
```

The runner records the installed Quint command help, Alloy `help exec`, versions,
exact command lines, all results, ITF traces and Alloy receipts in a fresh
`target/formal-reports/opencode.*` directory. Expanded core commands (`Q` is
`tests/formal/node_modules/.bin/quint`, `R` is that new report directory):

```sh
"$Q" typecheck tests/formal/opencode.qnt
"$Q" test tests/formal/opencode.qnt --backend typescript --seed 20261023 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261023 --max-samples 1000 --max-steps 14 --invariant safety --backend typescript --out-itf "$R/simulation_{seq}.itf.json" tests/formal/opencode.qnt
"$Q" verify --invariant safety --max-steps 14 --main opencode --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json" tests/formal/opencode.qnt
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/opencode.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/opencode.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test opencode_bdd
node scripts/formal/check_bdd_failures.cjs "$R"
cargo +1.99.1 test --locked --all-features --test opencode_sdk_cli --test opencode_runtime_contract --test opencode_state_contract --test opencode_config_contract
```

Simulation uses **seed 20261023, 1000 samples, 14 steps**. It is not proof.
Apalache bounded verification separately checks `safety`, the conjunction of all
four properties, through **14 transitions**. The helper
`scripts/formal/verify_quint.cjs` selects a fresh port and logs its exact argv.
Alloy 6.2.0 has no `--expect` CLI option. Its documented `--command '*'` selects
all commands, and explicit `expect 0`/`expect 1` are on each command in the model.
The solver is explicitly `sat4j`, bundled in the checksum-pinned Alloy artifact.
The receipt checker validates complete command coverage, kind, expectation and
actual satisfiability. Alloy omits zero-valued `expects` from JSON; the checker
handles that documented-by-observation encoding without dropping model checks.
Negative controls verify rejection of counterexamples, missing/extra commands,
unsatisfiable witnesses, malformed solution fields, missing concrete witness
instances and wrong or missing expectations. A failed setup is a blocker, even
if previously cached tools exist.

## CI

`.github/workflows/ci.yml` adds **OpenCode V2 Quint, Alloy and production BDD
contracts** to the existing read-only `formal` job on pull requests and main
pushes. It runs independently even if the preceding checkpoint step fails,
requires no secrets, installed OpenCode or provider authentication, and retains
reports with the existing always-run artifact upload. Cargo also registers
`opencode_bdd` as an explicit native harness-free integration target with
`test = false`: the supported Linux formal job always invokes it by name,
while the general Windows suite does not implicitly launch POSIX fixtures.
An explicit invocation on non-Unix fails rather than printing a passing skip.
The actual full
scenario count, final run receipts and any blockers are recorded separately in
`OPENCODE_VERIFICATION.md`. Local results do not imply remote CI or real-provider
OpenCode compatibility, which the plan treats as a distinct acceptance activity.
