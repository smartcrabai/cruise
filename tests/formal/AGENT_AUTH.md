# Workflow-start agent authentication specifications

## Authority and scope

The authority is the session plan dated 2026-10-24, titled
`Implementation plan: workflow-start agent authentication status check`.
The requirements are its confirmed decisions in section 0 and contracts in
sections 3 and 4, checked against the current production paths, `README.md`,
`NEVER.md`, and the existing repository Cucumber/formal conventions.

This is a workflow-scoped, read-only expiry preflight, not authentication or a
refresh implementation. Unknown credentials pass. Access expiry alone must not
block a renewable account. A jcode provider blocks only when every account is
known expired. All selected failed targets must be reported before any agent
launch or execution workspace creation. Command backends, exact opt-out `1`, and
planning and execution dry runs bypass the check. CLI planning and execution, plus shared
TUI/WebUI planning and execution setup, use the same production check.

## Traceability

| Contract | Production | Quint / Alloy | Concrete executable checks |
| --- | --- | --- | --- |
| Stop only on expired access AND absent/expired refresh, equality expired, unknown passes | `src/agent_auth.rs::judge`, `inspect_file`, `judge_accounts` | `expired`, `unavailable`, `admission` / `Admission`; renewable, unknown, fresh-access and refresh-expiry witnesses | `agent_auth` injected-time unit matrix, `planning::auth_contract_tests`, Gherkin credential scenarios |
| Only workflow-used SDK/provider candidates, planning and execution isolated | `src/config.rs::planning_model_candidates`, `execution_model_candidates`, `src/agent_auth.rs::ensure_agents_authenticated` | `selected`, `required`, `scopeSafety` / `ScopeSafety`; planning-isolation witnesses | Config candidate unit tests cover arrays, SDK qualification, step overrides, after-pr, groups, parallel children, fallback-chain replacement and disabled fallback; production Gherkin checks selection at CLI boundary |
| Unknown file, unsupported provider, bare model, API auth pass | `inspect_file`, `claude_uses_other_auth`, target selection | Unknown account, unsupported candidate identity, alternative-auth bypass | Planning contracts cover missing/malformed/read-error files, missing fields, aliases, all six Claude process/workflow auth variables, empty-key rejection, opencode and API/bare providers; Gherkin reaches an observable SDK process boundary |
| Reject before effects and aggregate all failed targets | `src/planning.rs::run_plan_prompt_template`, `src/run_cmd.rs::run_single_loaded`, `src/application.rs::setup_run_blocking`; exec delegates to run | `completeBeforeDecision`, `exactDiagnostics`, `effectGate` / `CompleteBeforeDecision`, `ExactDiagnostics`, `EffectGate`; all-targets rejection witness | Gherkin checks CLI output, absent SDK invocation/workspaces; `agent_auth_cli`; planning, run and UI setup unit contracts |
| Command/opt-out/dry-run bypass, no dry-run effects | `ensure_agents_authenticated`, run dry-run guard, `src/plan_cmd.rs::run` early dry-run return | `bypass`, `effectGate`; named tests and witnesses | Gherkin and existing unit/CLI contracts, including opt-out values other than `1` |
| Read-only files, no auth subprocess, no secrets in error/Debug/telemetry | `inspect_file`, refresh-presence deserializer, `src/error.rs` | Token bytes and file writes deliberately excluded, not a model-level security proof | Gherkin credential snapshots and secret assertions; planning no-auth-command sentinel, read-only tests, fixed error telemetry tests |

The formal machines abstract the resolved candidate collections to sets. They
do not copy the config traversal or pretend to verify YAML parsing. The concrete
Rust tests are the bridge to those real configuration and entrypoint behaviors.
No formal refinement proof of Rust is claimed.

## Model boundaries

`agent_auth.qnt` and `agent_auth.als` describe the same machine. Initial state has
no reads/errors and no execution workspace. Individual selected targets are
inspected, then a complete diagnostic set authorizes or rejects the workflow.
Only authorization permits a launch. Dry runs finish without launch.
Idle transitions pad shorter traces and allow bounded checking at every prefix.

There are two representative supported targets: Claude CLI and one jcode OAuth
provider, plus an unsupported/bare/API candidate. jcode Anthropic and OpenAI
have the same account-aggregation policy and are tested separately in Rust.
Claude has one account and jcode has two account slots. An empty, missing, or
malformed pool is represented by unknown slots. More than two accounts are not
proved by this model. Account expiry is quotiented into unknown, expired
(`<= now`), and future (`> now`) at one fixed observation instant. Refresh
presence and refresh expiry are independent inputs for Claude. jcode's two
slots fix refresh expiry to unknown because its credential schemas contain no
such field. Seconds/milliseconds,
provider aliases, path resolution and environment string parsing are concrete
checks, not modeled arithmetic or string operations.

Candidate sets are arbitrary independently for planning/execution, after actual
retry policy resolution. `otherAuth` means nonempty Claude alternative authentication in the environment applicable to the checked call: process or workflow environment for ordinary planning/execution, but process environment only for foreground CLI approval's SDK title generation. The opt-out boolean represents exact value `1`.
The `workspace` bit denotes permission to create an execution workspace after
preflight, not every filesystem operation performed by session creation or
planning. Failure may persist session error metadata, so rejection does not
promise byte-identical session state. Dry-run session/workspace preservation is
asserted concretely. No startup-wide scan, network validation, refresh-token
revocation detection, Keychain access, step-env auth override, arbitrary-account
proof, liveness guarantee, or unbounded proof is claimed.

## Reproducible tools and exact commands

From the repository root on macOS arm64 or Linux x86_64, put Node **26.12.0** on
PATH and install Rust **1.99.1**. Then run:

```sh
bash scripts/check_agent_auth_specs.sh
```

This uses the existing reproducible install setup without introducing another
parallel dependency stack:

- `tests/formal/package.json` and `package-lock.json`: Quint **0.33.0**, exact
  transitive npm lock, `adm-zip` override **0.6.1**, `npm ci --ignore-scripts`.
- `scripts/setup_formal.sh`: released Alloy **6.2.0** CLI JAR SHA256
  `6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`.
- The same setup script pins Temurin **25.0.5+8-LTS**, checksum-verifies the
  platform archive, extracts it afresh and validates the runtime. macOS arm64
  SHA256 `36dc14d797aaff6d89aab302cd053fdba6207dfdcf772052fa7b023a94b0e857`,
  Linux x86_64 SHA256
  `68982529869257d0ee6b824ec5aa270ff8b118bb438670da0b97ebdab178d7fb`.
- `scripts/formal/verify_quint.cjs`: Apalache **0.62.1**, explicitly selected
  backend on a fresh loopback endpoint, using that pinned JDK.
- `Cargo.toml` and root `Cargo.lock`: Cucumber **0.23.0** and tempfile **3.27.0**,
  native Rust step definitions. No additional BDD framework is installed.

The aggregate runner captures versions and the pinned installed commands' actual
help before invoking checks. With `Q=tests/formal/node_modules/.bin/quint`,
`R` the freshly created `target/formal-reports/agent-auth.*` directory, and
`PORT` an OS-allocated fresh endpoint, the substantive commands are:

```sh
bash scripts/setup_formal.sh
source target/formal-tools/env.sh
"$Q" typecheck tests/formal/agent_auth.qnt
"$Q" test tests/formal/agent_auth.qnt --backend typescript --seed 20261024 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261024 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript --out-itf "$R/simulation_{seq}.itf.json" tests/formal/agent_auth.qnt
"$Q" verify --invariant safety --max-steps 8 --main agent_auth --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json" tests/formal/agent_auth.qnt
java -jar target/formal-tools/alloy-6.2.0.jar help exec
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/agent_auth.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/agent_auth.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test agent_auth_bdd
node scripts/formal/check_agent_auth_bdd_failures.cjs "$R"
cargo +1.99.1 test --locked --all-features --test agent_auth_cli
cargo +1.99.1 test --locked --all-features --lib
```

Alloy 6.2.0's documented `exec` has **no `--expect` CLI flag**. The supported
expectation syntax is `expect 0` on each assertion command and `expect 1` on each
witness command in the model. Explicit `--command '*'` selects every command.
The shared receipt validator rejects absent/extra/duplicate commands, malformed
solutions, wrong expectations and unexpected SAT/UNSAT outcomes. Merely exiting
Alloy with zero does not count as passing.

Simulation is not proof: its fixed seed is **20261024**, maximum **1000 samples**
of **8 transitions** each. Bounded symbolic verification checks `safety` with
Apalache **0.62.1** through **8 transitions**. Alloy uses SAT4J and exactly **9
ordered states**, hence **8 transitions**, three account records and three
candidate identities. There is no integer arithmetic in the Alloy contract.

## CI and evidence

`.github/workflows/ci.yml` adds `agent-auth-specs` on ordinary pull requests and
main pushes. It has read-only repository permissions, no secrets or provider
accounts, pinned Node/Rust and immutable action revisions. The single aggregate
command runs every check above, including receipt-validator self-tests and real
BDD negative controls. Independent failures accumulate a nonzero exit status.
Setup failure or an unsupported platform is a blocker, never a pass.

Each run creates a new report directory, capturing exact commands, stdout/stderr,
exit statuses, CLI help, runtime versions, Quint test/simulation traces, Alloy
`receipt.json` and SAT witness instances. On invariant failure, any emitted ITF
counterexample is retained. Absence of a counterexample after a successful
verification is normal. CI uploads evidence with `if: always()` for 14 days and
fails if the report directory is missing. Local execution is not a claim of a
completed hosted CI run. See `AGENT_AUTH_VERIFICATION.md` for observed results.
