# Subagent model executable contract

## Authority and scope

Grounded in the subagent-model implementation plan supplied with this task
(sections 0, 2, 3, 4, 6 and 8), `README.md`, and the production paths below.
The accepted contract is a single string or SDK-qualified `ModelEntry`, only
Claude and jcode, workflow defaults with prompt-step precedence, namespace
resolution against the original workflow SDK (default jcode), and application
only to matching fallback attempts. Planning uses the workflow value. Titles,
PR metadata and workflow generation exclude the override. Claude injects both
model and FORCE=1, overriding conflicting workflow env only when configured.
jcode sets the session's `Agents ["swarm", route]` default before sending a
prompt, not the unrelated user `/subagent` setting. A failed setting must not
send that attempt's prompt. Explicit worker model choices still win in jcode.

Run the complete credential-free suite from the repository root:

```sh
bash scripts/check_subagent_model_specs.sh
```

## Requirement-to-check map

| Contract | Production authority | Formal properties | Concrete execution |
| --- | --- | --- | --- |
| Step > workflow, strings use original SDK, objects use explicit SDK, apply only on SDK match | `engine::run_prompt_step`, `model_selection::resolve_entry`, `executor::run_sdk` | `selection`, `completeness`; `PrecedenceAndSdkIsolation` | Gherkin precedence, default-jcode fallback and namespace isolation scenarios; `sdk_model_execution_cli` |
| Reject command/OpenCode, malformed/blank/effort/missing jcode model, absent applicable SDK; local overrides only on prompts | `config::validate_subagent_model_entry`, `check_subagent_model_step`, `executor::run_inner` | `rejection`; `RejectionBeforeDispatch`, rejected witnesses | Gherkin invalid-value table, valid baseline after removing only the offending field, direct Executor no-spawn marker; config unit contracts |
| Top-level SDK must occur in workflow execution/planning union, local SDK in prompt's effective candidates | `config::candidate_sdks`, `model_selection::applicable_sdks` | separate `workflowCandidates`/`candidates`; phase-applicability witnesses | config unit tests, including phase-only applicability and retry candidates |
| Both Claude env keys forced together, unrelated/omitted override preserves user env | `backend::claude::build_options` | `environment`; `BackendContract` | Gherkin real subprocess env records, Claude unit contracts, CLI env tests |
| jcode route configured before dispatch, shared runtime lookup, errors contextualized | `backend::jcode::run_client_turn`, `model_switch_request` | `ordering`, `environment`; `ConfigureBeforeSend`, `AgentsFailureWitness` | `subagent_model_jcode_cli` real SDK API frames and output depending on active setting; backend unit tests |
| Planning inherits workflow, auxiliary generation excludes overrides | `planning::run_planning`, `plan_cmd`, `worktree_pr`, `workflow_generate` PromptRun sites | `scope`; `ExcludedMetadata`, planning/metadata witnesses | Gherkin actual plan/title calls and workflow generation with validated written output; CLI title exclusion; PR/workflow-generation SDK subprocess unit tests |
| Group, parallel prompt child, after-pr, github-review, workflow-call preservation and defaults layering | `workflow::compile`, `workflow_call`, `workflow_defaults`, `github_review` | Same prompt contract after structural expansion | Gherkin group/parallel execution and after-pr compile assertions, CLI tests, existing library tests run by the aggregate suite |

## Models and limits

`subagent_model.qnt` and `subagent_model.als` describe one request with explicit
validation, attempted backend selection, configuration, send, retryable failure,
completion, and configuration-error transitions. Two attempts suffice to expose
cross-SDK leakage and restoration of matching overrides. The retry engine's
classification, ordering and cooldown policy are intentionally not re-specified:
attempt selection nondeterministically overapproximates effective candidate
SDKs. A fallback represents an eligible failure before user-visible output.
Each fresh attempt has fresh configuration options, not a reset of persistent
jcode session overrides. Clearing overrides on resumed sessions is out of scope.

SDKs are 0=jcode, 1=Claude, 2=OpenCode. An entry owner of -1 means absent and 3
means a bare string. Tokens 1 and 2 distinguish workflow and step model IDs.
Legacy env tokens 7 and 9 make preservation distinguishable from injection.
`wellformed` abstracts actual parsing, blank/effort and jcode syntax validation,
which the production tests exercise concretely. Purpose 0 is a prompt, 1 is
planning, and 2 is auxiliary generation. Group/review/parallel expansion reduces
to the same prompt request. Provider routing is an opaque token in the models,
with actual provider-to-route mapping checked through jcode API traffic.

Quint simulation is **not proof**: seed **20261025**, **1000 samples**, at most
**12 transitions** each, TypeScript backend. Verification separately checks
`safety` through **12 transitions** using **Apalache 0.62.1**. Alloy uses
**13 exactly ordered states**, hence **12 transitions**, signed **5-bit Int**,
and **SAT4J**. Idle transitions pad short traces. These are bounded abstract
checks, not an unbounded proof, liveness guarantee or machine-checked refinement
of the Rust implementation. No external model availability is assumed.

## Reproducible tooling and exact commands

Reuse the existing shared pins rather than adding a second installation stack:
Node **26.12.0**, Quint **0.33.0** (`tests/formal/package-lock.json`), Apalache
**0.62.1**, Alloy **6.2.0**, Temurin JDK **25.0.5+8-LTS**, Rust **1.99.1**,
and Cucumber **0.23.0** (`Cargo.lock`). The shared `setup_formal.sh` verifies
Alloy SHA256 `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`
and platform-specific JDK archive SHA256, checks the runtime, and runs
`npm ci --prefix tests/formal --ignore-scripts`. No new dependency is needed.
Supported local hosts are Darwin arm64 and Linux x64. Other hosts or missing
runtimes cause a blocking nonzero result, never a skipped pass.

The aggregate captures actual installed CLI help for all Quint subcommands and
`java -jar target/formal-tools/alloy-6.2.0.jar help exec`, along with versions and
solver inventory. Alloy 6.2.0 has **no `--expect` CLI flag**. Its documented
model-command `expect 0`/`expect 1` declarations are used instead. The receipt
validator requires every assertion to have no counterexample and every witness
to have a concrete result. Missing, extra or malformed results fail the suite.

With `Q=tests/formal/node_modules/.bin/quint`, `R` a fresh report directory, and
`PORT` a fresh OS-allocated loopback port, actual command forms are:

```sh
bash scripts/setup_formal.sh
source target/formal-tools/env.sh
"$Q" typecheck tests/formal/subagent_model.qnt
"$Q" test tests/formal/subagent_model.qnt --backend typescript --seed 20261025 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261025 --max-samples 1000 --max-steps 12 --invariant safety --backend typescript --out-itf "$R/simulation_{seq}.itf.json" tests/formal/subagent_model.qnt
"$Q" verify --invariant safety --max-steps 12 --main subagent_model --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json" tests/formal/subagent_model.qnt
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/subagent_model.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/subagent_model.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test subagent_model_bdd
node scripts/formal/check_subagent_bdd_failures.cjs "$R"
cargo +1.99.1 test --locked --all-features --test sdk_model_execution_cli --test subagent_model_jcode_cli
cargo +1.99.1 test --locked --all-features --lib
```

The verifier wrapper `scripts/formal/verify_quint.cjs` invokes the actual Quint
`verify` command above, isolates the pinned Apalache endpoint, logs exact argv,
and preserves the subprocess exit code. All state invariants are conjoined in
`safety`. No simulator result is substituted for verification.

## CI and failure semantics

`.github/workflows/ci.yml` adds `subagent-model-specs` on normal relevant PRs
and main pushes, without secrets. The job installs pinned Node/Rust and invokes
the aggregate runner, which installs verified Java/Alloy and locked Quint.
Independent checks continue after failure but accumulate a nonzero exit code.
Fresh `target/formal-reports/subagent-model.*/` directories retain exact commands,
versions, logs, results, ITF traces, Alloy receipt and witness JSON. CI uploads
them even after failure and errors if reports are missing.

The native Cucumber runner uses real YAML validation, compilation, Executor and
CLI entry points. Only external agent executables are replaced at the transport
boundary. Assertions check actual dispatch order, environment and outputs, plus
a shell marker proving invalid command execution never spawned. Undefined or
pending/skipped steps fail with `fail_on_skipped()`, assertion failures propagate,
and zero executed steps fail explicitly. Negative controls exercise the actual
runner with an undefined step, deliberately wrong model assertion, and empty
selection, rejecting compiler failures or timeouts as substitutes.

Dated execution evidence, results and any blockers are in
`SUBAGENT_MODEL_VERIFICATION.md`.
