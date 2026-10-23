# OpenCode specification execution evidence

Date: **October 23, 2026**, local execution on macOS arm64. All edits remain
uncommitted. No staging, commits, HEAD/ref moves or commit-guard changes were
performed. Existing unrelated worktree changes were preserved.

## Grounding and scope

Read the session's authoritative `plan.md`, repository `NEVER.md`, existing
formal/BDD conventions, and production attempt admission and cleanup in
`src/backend/opencode/mod.rs`, event reduction in `events.rs`, native bridge
readiness/effects in `tools.rs`, owned runtime shutdown in `runtime.rs`, stream
reduction in `src/backend/stream.rs`, and shared retry in `src/executor.rs`.
`OPENCODE.md` maps plan sections 4.3, 5.2, 5.3 and 5.5 to each property and test.

The small paired models cover checkpoint/subscription before input admission,
terminal execution evidence, no replay after effects or uncertain results, safe
fallback only after shutdown, and publication after owned-runtime cleanup.
They are not a full protocol model or a refinement proof of Rust, nor an
unbounded, liveness, crash-exactly-once or OS-sandbox proof.

## Reproducible versions

| Tool | Selected and observed version |
| --- | --- |
| Node | 22.22.0 |
| npm | 10.9.4, observed with the pinned Node installation |
| Quint | 0.33.0, locked npm package |
| Apalache | 0.62.1, build a321771, explicitly selected on a fresh loopback port |
| JDK | Temurin 25.0.5+8-LTS, release date October 20, 2026 |
| Alloy | Released 6.2.0 distribution JAR, explicit bundled `sat4j` solver |
| Rust | rustc 1.99.1 (58cf8f9c3 2026-10-13) |
| Cargo | 1.99.1 (5f94df478 2026-08-27) |
| Cucumber | 0.23.0, exact Cargo dependency with root Cargo.lock |

`bash scripts/setup_formal.sh` succeeded with pinned Node on PATH. It performed
checksum-validated Alloy/JDK installation and `npm ci --ignore-scripts` using
`tests/formal/package-lock.json`. Alloy artifact SHA256:
`6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`.
JDK archive SHA256 pins for both supported formal platforms are in that script.
A setup failure now blocks execution even if cached binaries are present.

The installed tool's actual help was executed and retained: Quint `typecheck`,
`test`, `run`, and `verify --help`, Alloy `help exec`, and Alloy `solvers`.
Alloy 6.2.0 has no `--expect` CLI option. The model uses explicit `expect 0` for
each assertion and `expect 1` for each witness, with independent receipt checking.

## Final successful specification run

Executed from repository root:

```sh
PATH="$PWD/target/formal-tools/node/bin:$PATH" bash scripts/check_opencode_specs.sh
```

The local pinned Node directory was already available. In CI, the existing
checksum-pinned `actions/setup-node` action selects Node 22.22.0 instead.
All checks returned zero, aggregate status **0**. Full command log and raw results:
`target/formal-reports/opencode.Arrv3o/{commands.log,results.log}`.

Expanded core commands, with `Q=tests/formal/node_modules/.bin/quint` and
`R=target/formal-reports/opencode.Arrv3o`:

```sh
"$Q" typecheck tests/formal/opencode.qnt
"$Q" test tests/formal/opencode.qnt --backend typescript --seed 20261023 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261023 --max-samples 1000 --max-steps 14 --invariant safety --backend typescript --out-itf "$R/simulation_{seq}.itf.json" tests/formal/opencode.qnt
"$Q" verify --invariant safety --max-steps 14 --main opencode --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint 127.0.0.1:55941 --out-itf "$R/counterexample.itf.json" tests/formal/opencode.qnt
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/opencode.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/opencode.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test opencode_bdd
node scripts/formal/check_bdd_failures.cjs "$R"
cargo +1.99.1 test --locked --all-features --test opencode_sdk_cli --test opencode_runtime_contract --test opencode_state_contract --test opencode_config_contract
```

The runner invokes `verify` through `scripts/formal/verify_quint.cjs`, which
reserves a fresh endpoint and logs the exact child argv in `quint-verify.log`.
The port above was the actual final run's endpoint, not a fixed configuration.

### Results and bounds

- Quint typecheck passed and all **6 executable tests passed**.
- Seeded simulation: **seed 20261023, 1000 samples, 14 transitions**. No violation
  found. This simulation is **not proof**.
- Separate Apalache 0.62.1 bounded verification: `safety`, the conjunction of
  admission, retry safety, completion and domain properties, held through
  **14 transitions**. Result `NoError`, no counterexample. No unbounded claim.
- Alloy: **exactly 15 ordered states, 14 transitions, signed 5-bit integers**.
  All four assertions returned UNSAT and all six witnesses returned SAT.
- Production Cucumber: **12 scenarios, 60 steps, all passed**, no skipped,
  undefined or pending steps and no scenario reruns. Tests execute real
  `Executor::run(PromptRun)`, HTTP/event reduction, retry, native bridge and
  shutdown against local V2 HTTP/SSE and owned POSIX CLI fixtures.
- BDD negative controls: deliberately undefined step and deliberately wrong
  prompt-count assertion each produced **exit 101, exactly one failed step,
  no scenario rerun**. The assertion control executes the real Executor first.
  Raw expected-failure logs are `bdd-negative-undefined.log` and
  `bdd-negative-assertion.log` under the same report directory.
- Receipt-validator tests: **13 passed**, including malformed receipt and
  counterexample/missing-command/unsatisfiable-witness negative controls.
- Additional production contracts: **40 passed** (13 configuration, 5 runtime,
  16 SDK/CLI, 6 state contracts), none skipped or ignored. This includes the
  real `cruise exec -c ...` entry point with the fixture CLI.

### Inspected Alloy receipt

Receipt: `target/formal-reports/opencode.Arrv3o/alloy/receipt.json`.
SHA256: `3043f9665cf128e2a1ca03e74932c61d8ceb3fbeb6785efeed727d1751dded55`.
Every selected command was present, with the expected kind and satisfiability.
All SAT receipts contain concrete instances, retained alongside witness JSON.

| Command | Kind | Model expectation | Receipt result |
| --- | --- | --- | --- |
| Admission | check | 0 | UNSAT |
| RetrySafety | check | 0 | UNSAT |
| Completion | check | 0 | UNSAT |
| Domains | check | 0 | UNSAT |
| SuccessWitness | run | 1 | SAT |
| SilentEffectFailureWitness | run | 1 | SAT |
| SafeFallbackWitness | run | 1 | SAT |
| UncertainWitness | run | 1 | SAT |
| RejectedCheckpointWitness | run | 1 | SAT |
| CancellationWitness | run | 1 | SAT |

The receipt omits zero-valued `expects` and UNSAT `solution` fields. The checker
requires explicit expectations in the model, checks all command results, rejects
malformed present solution fields and requires concrete SAT witness instances.
It does not trust the CLI exit status alone.

Model input hashes for this run:

```text
6ecd47ab501db85c86e48f81fe98804e1b88c942f2d744459e54c48324c51e54  tests/formal/opencode.qnt
96b97d8879f004e5e2019a4ffe796e9029e3b29c9f558efc046557ccc4bf3522  tests/formal/opencode.als
```

## Defects corrected and iteration evidence

No new production Rust defect was found or changed by this specification task.
Existing production changes were inspected rather than attributed to this task.
The concrete specification/runner defects corrected were:

1. Alloy failure witnesses could reach the right final outcome at the wrong
   boundary, such as cancellation during second-attempt setup instead of an
   active submitted turn. Witnesses now require their named first-attempt
   boundary, and checkpoint rejection requires an unacknowledged checkpoint.
   All strengthened witnesses remain SAT. No invariant/assertion was weakened.
2. The POSIX BDD executable printed a successful message on unsupported
   platforms. It now fails explicitly. Cargo marks it `test = false` for
   implicit platform-wide test discovery, while required Linux formal CI
   always invokes `--test opencode_bdd` explicitly. An unavailable-platform
   invocation is not counted as a pass.
3. Domain tag `@retry-barrier` triggered Cucumber's tag-based scenario retry.
   Scenario retries are now disabled explicitly, and a tagged negative control
   confirms an assertion failure is not rerun or hidden.
4. Generic `is_err()` assertions could accept unrelated startup failures.
   Scenarios now require their intended diagnostic categories. The first
   strengthened run, `opencode.WPRXp4`, correctly failed because the old
   `incomplete` fixture returned HTTP 503 during log recovery, not a synchronized
   log without terminal evidence. Rather than relaxing the invariant or adding
   a catch-all diagnostic, the fixtures now separately exercise full-text plus
   successful log sync without a terminal, and unavailable-log recovery. Both
   fail with exactly one input and the appropriate distinct diagnostic.
5. Malformed Alloy receipt `solution` fields could be interpreted as UNSAT.
   They are now rejected, with explicit negative controls. Witness records
   without concrete instances are rejected as well.
6. Existing task-related dependency additions in Cargo.toml failed the current
   Taplo formatting check. Only their ordering/wrapping was corrected.
7. Strict Clippy caught the added log-unavailable fixture variant being used
   only by BDD, not the standalone SDK integration target. Added a meaningful
   standalone unavailable-log/no-replay regression and strengthened the
   existing missing-terminal diagnostic assertion, rather than suppressing
   the dead-code lint. Final Clippy and the complete specification rerun pass.

The initial draft run `opencode.tE9xrc` passed. The strengthened intermediate run
above failed as described. After fixing the actual fixture/runner defects, the
complete final run `opencode.Arrv3o` passed, including all affected checks.

## Shared tooling and repository regression

`PATH="$PWD/target/formal-tools/node/bin:$PATH" bash scripts/check_formal.sh`
passed with aggregate status 0. Reports: `target/formal-reports/run.v1ftQX`.
This reruns the existing checkpoint models and production BDD/graph contracts
using the same pinned formal dependency installation.

The earlier full test invocation passed in
`target/formal-reports/opencode-regression.pWoSEH`, but that aggregate run
failed because Clippy found the fixture coverage issue described above.
After the standalone regression was added, final formatting and strict Clippy
passed. The complete specification suite also passed again in `opencode.Arrv3o`.
The final run in `target/formal-reports/opencode-final.ZFW59o` finished with
**aggregate status 0**. Its exact commands and individual exit codes are in
`commands.log` and `results.log`. The following all passed:

```sh
cargo +1.99.1 fmt -- --check
taplo lint Cargo.toml
taplo fmt --check --option reorder_keys=true Cargo.toml
git diff --check
cargo +1.99.1 clippy --locked --all-targets --all-features -- -D warnings -A clippy::large_futures
env PATH="$PWD/target/formal-tools/node/bin:$PATH" bash scripts/check_opencode_specs.sh
cargo +1.99.1 test --locked --all-features
```

The full Cargo run reports **4093 passed, 0 failed, 7 ignored** across 35 Rust
test harness summaries, in addition to its harness-free checkpoint BDD target.
The seven ignored registrations are six existing child-process probes
(three each in the library and main test binaries) and the explicitly opt-in
installed OpenCode acceptance test. They are not counted as passing checks.
The separate required OpenCode BDD target ran explicitly as shown above and
passed all 12 scenarios. Its `test = false` setting only affects implicit Cargo
discovery, not this required execution.

## CI and remaining boundaries

CI file changed: **`.github/workflows/ci.yml`**. The read-only `formal` job runs
`bash scripts/check_opencode_specs.sh` on pull requests and pushes to main, even
if the preceding independent checkpoint step fails. It uses pinned Node/Rust,
installs the pinned formal tools, runs every model, witness, BDD and negative
control above, and always uploads `target/formal-reports/` for 14 days. No
provider credentials, secret-dependent PR permission, paid calls or installed
OpenCode executable are required.

No prerequisite is missing for the required local specification checks.
Supported formal installation platforms are macOS arm64 and Linux x64.
Unsupported platforms fail with a blocker rather than claiming a passing check.
Windows execution and remote GitHub Actions were not run or claimed. Actual
installed OpenCode/plugin/provider acceptance
is separate from these deterministic fixture checks. The ignored installed-CLI
acceptance test was not opted into or counted as a pass here.
