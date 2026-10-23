# SDK-qualified model executable specifications

These checks implement the task in the session plan dated 2026-10-23, titled
“SDKを指定できるモデル候補リストの実装計画”. The authoritative requirements
are plan sections 3.1 through 3.5 and completion conditions 1 through 5, checked
against the current production paths rather than the older line numbers in the
plan. `README.md` (SDK mode and model defaults), `NEVER.md`, and the existing
checkpoint/OpenCode specification conventions also informed this work.

## Scope and requirement-to-check map

The two formal models describe the same small observable machine: a caller
selects candidates, starts an SDK attempt, optionally receives a session
checkpoint or visible output, then succeeds, fails, or cancels. A second call
within the process's cooldown window can create a fresh policy without erasing
failed candidate identities. State explicitly includes the workflow SDK,
candidate position, attempt count, output visibility, cooldown identities,
original resume owner, handed-off resume, and emitted checkpoint owner.

Representative input in both models:

```yaml
sdk: jcode # the models also quantify over claude as the base
model:
  - shared:high
  - sdk: claude # the opposite of the base SDK in this representative family
    model: shared:low
  - shared:low # same-SDK effort-only duplicate, must not be attempted again
  - next:high # must return to the base SDK
```

The input family is intentionally not a copy of Rust data structures or a model
of every configuration combination. Two SDKs and two opaque model references
suffice to distinguish cross-SDK same-name candidates from same-SDK effort
variants, restore inheritance after an override, and reject cross-SDK resume.
Existing OpenCode support is preserved and remains covered by its separate
suite. The plan does not authorize removing it to make a two-SDK model pass.

| ID | Required observable contract | Production authority | Quint / Alloy | Production-connected checks |
| --- | --- | --- | --- | --- |
| SM-01 | Strings always inherit the workflow SDK. An object changes only its own candidate. Preserve ordered cross-SDK dispatch. | `model_selection::resolve_candidates`, `Executor::run`, `executor::run_sdk_with_fallback`, `workflow::compile` | `selection` / `Selection`, cross-SDK and reverse-inheritance tests/witnesses | Gherkin mixed execution and planning scenarios invoke the actual CLI and local SDK subprocesses; `sdk_model_execution_cli`, `model_fallback` |
| SM-02 | Identity is SDK plus effort-stripped model. Same name on another SDK is eligible, same-SDK effort variants are not independent quota slots. Cooldowns survive policy recreation. | `retry::FallbackEngine::new_candidates`, `take_candidate`, `RetryPolicy::start_sdk_cooldown` | `selection`, `cooldownSafety` / `Selection`, `CooldownSafety`; cross-SDK, policy-reload and all-cooling tests/witnesses | Mixed Gherkin subprocess order and models; `retry` unit contracts in the full `--lib` suite; `model_fallback` |
| SM-03 | Pass only the original caller's resume to a same-owner SDK, never a failed attempt's newly emitted conversation to a subsequent attempt. | `executor::resume_for`, SDK config builders, `run_sdk_with_fallback` | `resumeSafety` / `ResumeSafety`; cross-SDK and cancellation tests/witnesses | Gherkin calls real `Executor::run` with saved Claude and jcode owners, inspects actual Claude subprocess arguments; mixed scenarios inspect failed-ID absence |
| SM-04 | Session checkpoint and outcome carry the actual attempt SDK, not the workflow SDK. | `run_sdk_with_fallback` callback and outcome mapping, `SdkSessionId`; `application`/`plan_cmd` checkpoint persistence | `checkpointSafety` / `CheckpointSafety`, cancellation test/witness | Gherkin asserts real callback and outcome ownership; application/session/plan unit contracts and `session_config_contract` cover persistence, legacy state and cleanup |
| SM-05 | Visible output forbids switching SDK/model for that failed turn. Cancellation does not start a subsequent attempt. | `retry::FallbackEngine::next`, `executor::stream_to_outcome`, cancellation handling in common loop | `replaySafety` / `ReplaySafety`, visible-failure and cancellation tests/witnesses | Existing executor/retry unit contracts, `model_fallback`, `model_logging_cli` |
| SM-06 | Both app formats preserve element form, SDK and order through an unrelated settings update. `command` conflicts with an SDK object at any position in either field. | `AppConfig::load_from` / `save_to`, `config::validate_config`, `validate_sdk` | Concrete-only contract, deliberately not represented as serialization syntax in the abstract models | Gherkin atomic JSON/YAML save/reload and command-conflict CLI scenarios; `sdk_model_config_contract`, `app_config_cli` |
| SM-07 | Compilation, defaults, environment overrides, ordinary/planning separation, snapshots and capabilities must preserve the plan's contracts. | `config`, `workflow_defaults`, `workflow`, `session_config`, all prompt callers | Concrete-only coverage, not a claim of the scalar models | `model_fallback`, `workflow_defaults_cli`, `session_config_contract`, `sdk_model_config_contract`, `sdk_model_execution_cli`, plus existing cross-platform full regression CI |

## Abstraction and limits

* `fail`/`Fail` means a model-missing 404 under enabled model fallback, with a
  nonzero retry budget and no other replay barrier. This is an existing
  failure classification, not a new retry policy. Other 4xx, 429, server/network
  failures, Retry-After and budgets remain concrete Rust regression checks.
* `text` abstracts text actually delivered to the caller's output sink.
  A failure after visible text terminates the call rather than switching.
* `start` abstracts admission to the existing SDK driver. No provider response
  is assumed. Success, failure and cancellation are nondeterministic outcomes.
* `checkpoint` abstracts a delivered SDK session callback, not filesystem
  durability. ID 1 denotes the original caller's conversation provenance.
  IDs 2 and above denote attempt-issued provenance. They are abstract tokens,
  not a claim that session-ID strings must be globally unique between SDKs.
* `newCall` is a genuinely new caller, not an automatic retry after success,
  visible failure or cancellation. It retains cooldowns and resets only the
  selection position. Cooldown expiration is outside the modeled time window.
* Up to two calls, four input positions, two SDKs, two model identities, and six
  starts are represented. Initial resume ownership is unknown, jcode or Claude.
  No unbounded proof, liveness/fairness, provider availability, full Rust
  refinement, filesystem cleanup safety or backoff timing proof is claimed.
* Both models include idle transitions so witnesses shorter than the bound can
  be padded. Alloy uses exactly 13 ordered states (12 transitions), signed
  5-bit integers. All reachable modeled arithmetic is within that range.

## Reproducible execution

Use Node **22.22.0** on PATH and install Rust **1.99.1**. On Darwin arm64 or
Linux x86_64, from the repository root:

```sh
rustup toolchain install 1.99.1 --profile minimal
bash scripts/check_sdk_model_specs.sh
```

Other platforms are blockers for this POSIX specification runner, not skipped
passes. Existing Windows production test CI remains in place. This runner uses
`scripts/setup_formal.sh`, the existing checksum-verified install setup:

| Component | Pin |
| --- | --- |
| Quint | `@informalsystems/quint` **0.33.0**, `tests/formal/package.json` and npm lock, installed with `npm ci --ignore-scripts` |
| Node | **22.22.0**, checked by setup and pinned in GitHub Actions |
| Apalache | **0.62.1**, passed explicitly to Quint on a fresh OS-allocated loopback endpoint |
| Alloy CLI | released **6.2.0**, `org.alloytools.alloy.dist.jar`, SHA-256 `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d` |
| JDK | Temurin **25.0.5+8-LTS**, runtime checked by setup; Darwin arm64 archive SHA-256 `36dc14d797aaff6d89aab302cd053fdba6207dfdcf772052fa7b023a94b0e857`; Linux x64 archive SHA-256 `68982529869257d0ee6b824ec5aa270ff8b118bb438670da0b97ebdab178d7fb` |
| Rust / Cucumber | **1.99.1** / **0.23.0**, existing exact Cargo dependency and committed `Cargo.lock`; every Cargo invocation uses `--locked` |

`Q` below is `tests/formal/node_modules/.bin/quint`, `R` is the fresh report
directory created by the runner, and `PORT` is the verifier's isolated port.
The runner executes and retains help for all four Quint commands, Alloy
`help exec`, versions, and solver inventory before model execution.

```sh
"$Q" typecheck tests/formal/sdk_models.qnt
"$Q" test tests/formal/sdk_models.qnt --backend typescript --seed 20261023 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261023 --max-samples 1000 --max-steps 12 --invariant safety --backend typescript --out-itf "$R/simulation_{seq}.itf.json" tests/formal/sdk_models.qnt
"$Q" verify --invariant safety --max-steps 12 --main sdk_models --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json" tests/formal/sdk_models.qnt
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/sdk_models.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/sdk_models.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test sdk_model_bdd
node scripts/formal/check_sdk_model_bdd_failures.cjs "$R"
cargo +1.99.1 test --locked --all-features --test model_fallback --test sdk_model_config_contract --test sdk_model_execution_cli --test app_config_cli --test workflow_defaults_cli --test model_logging_cli --test session_config_contract
cargo +1.99.1 test --locked --all-features --lib
```

Simulation seed **20261023**, **1000 samples**, **12 transitions/sample** is
supplementary testing, not proof. The distinct Apalache check exhausts symbolic
behaviors only through **12 transitions** for the modeled state space.

Alloy 6.2.0's actual installed `help exec` has `--command` and no `--expect`
option. Therefore each named model command declares `expect 0` for assertion
checks and `expect 1` for meaningful witness runs. The explicit wildcard selects
**all 12 commands**. The independent receipt validator rejects missing/extra
commands, counterexamples, missing witness instances, malformed solutions,
wrong command types and wrong expectations. Its own negative unit tests are
part of this runner. Assertions cannot pass merely because the entire trace
relation is unsatisfiable: six named witness runs must produce concrete SAT
instances in the same scope.

The native Cucumber runner fails on skipped/undefined/pending steps, failed
assertions, and empty selections. Negative controls execute that real runner
with a deliberate undefined step and a deliberately wrong production-output
assertion. Build failures or missing platforms are not accepted as evidence
that the negative controls work. Local shell fixtures replace only the final
agent processes, never Cruise selection/dispatch/retry, and cannot contact
providers. App-config scenarios use real on-disk files and atomic writes.

## CI and evidence

`.github/workflows/ci.yml` has a separate **sdk-model-specs** Ubuntu PR/push job
running the complete aggregate script above, with pinned Node/Rust, read-only
contents permission, no secrets, and no authenticated SDK service dependency.
Every check continues after independent failures and contributes to a nonzero
aggregate status. Toolchain setup failure blocks execution instead of trusting
stale tools. The existing Linux/macOS/Windows production jobs are unchanged.

Each invocation creates a fresh `target/formal-reports/sdk-models.*` directory.
Exact commands, versions, help, statuses, traces, solver logs, negative-control
logs, all Alloy SAT instances and the complete receipt are retained. The
`verify_quint.cjs` nested command logs the exact argv and isolated endpoint.
GitHub Actions uploads these reports even after failures for 14 days.
See `SDK_MODELS_VERIFICATION.md` for actual local results and any limitations.
