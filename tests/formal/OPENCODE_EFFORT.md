# OpenCode common effort and no-route contract

## Reproduce

From the repository root, with Node **24.22.0** on PATH and Rust **1.99.1** installed:

```sh
bash scripts/check_opencode_effort_specs.sh
```

This installs the existing checksum-pinned formal toolchain through
`scripts/setup_formal.sh`, runs every check below, and accumulates a nonzero
status for any failure. Unsupported platforms and unavailable tools are blockers,
not skips. Supported local platforms are macOS arm64 and Linux x64. CI runs the
Linux path in `.github/workflows/ci.yml`, job `opencode-effort-specs`, for pull
requests and main pushes. It requires no credentials, model account, preinstalled
OpenCode CLI or paid inference. The runner installs integrity-locked OpenCode
**2.1.4** and runs the real CLI against an isolated loopback model server,
including provider request `reasoning_effort`, native plugin, resume and cancel.
HTTP/SSE fixtures also drive the production Executor and owned subprocess
lifecycle, not a mock Executor. CI always uploads
fresh run reports under `target/formal-reports/opencode-effort.*/` for 14 days.

## Authority and traceability

The authority is the session plan titled "OpenCode common effort support and fallback fixes for unavailable models",
Requirements and Design sections 1-4. The request to add formal checks supersedes
that plan's earlier `formal_methods=off` execution setting. Repository `NEVER.md`
continues to apply: switching is failure-driven, not cost routing. The models do
not introduce a new provider adapter, configuration field, cache or retry policy.

| Required observable behavior | Production paths | Quint / Alloy | Concrete checks run in CI |
| --- | --- | --- | --- |
| Ready before catalog and create/resume selection, resolve before prompt | `src/backend/opencode/mod.rs::run_attempt`, `client.rs::models` | `capabilitySafety` / `CapabilitySafety` | Cucumber common effort new/resumed, SDK CLI readiness/location observations |
| Canonical compatible variant wins, otherwise exactly one metadata match, never guess or downgrade | `parse_model_ref`, `resolve_effort`, catalog DTOs in `types.rs` | canonical/alias profiles, canonical and conflicting-canonical witnesses | Cucumber canonical/metadata, unit/SDK CLI resolver cases |
| Unsupported, ambiguous, unlisted, HTTP/auth/transport/protocol failure sends no prompt and makes no model update | same setup path, `stream_agent` retry barrier | rejection transitions, rejected-resume/API witnesses | Cucumber 14 failure examples on new/resumed sessions, SDK CLI malformed DTO cases |
| Native variant, plain, unknown colon and no model bypass catalog | same parser and conditional catalog call | plain/native kinds, native bypass witness | Cucumber four compatibility cases, parser/SDK CLI punctuation and alias cases |
| Strict suffix grammar, tierless suffix and effort/native conflict rejected | shared `src/backend/effort.rs` plus OpenCode parser | invalid request abstraction, not copied string parsing | OpenCode unit tests and SDK CLI invalid selections |
| Genuine no-route switches once in configured order, cools base identity, no same-model retry | `src/retry.rs::classify_retryable_lower`, `FallbackEngine::next`, `src/executor.rs` | `fallbackSafety` / `ReplayAndFallbackSafety`, fallback rechecks witness | Cucumber no-route fallback and notices, retry unit cooldown assertions |
| Authentication, no policy, disabled fallback, no candidate, zero budget, visible output/tool effects/uncertain acceptance prohibit replay | same classifier/engine, Executor retry barrier | permanent/barrier transitions and all four policy gates | Cucumber tool/zero-budget plus existing uncertain/effect scenarios, retry unit exclusion matrix |
| Effort is preserved through wildcard expansion, identity ignores suffix but not SDK, exact chain key has priority | `ModelReferenceFormat::{split,suffix}`, `chain_for_formats` | fixed request kind across fallback, base-identity cooldown abstraction | retry unit tests, SDK CLI workflow/plan candidate arrays and CLI step selection |

The runner explicitly invokes all affected OpenCode unit tests, all retry unit
tests, and `opencode_sdk_cli`, `opencode_runtime_contract`,
`opencode_state_contract`, `opencode_config_contract`, alongside Cucumber. Thus
lexical/HTTP/classifier details intentionally abstracted by the models are still
executable checks connected to CI, not unverified prose requirements.

## Small model and bounds

Both models use two distinct ordered base-model identities, one representative
requested effort tier, and seven finite catalog profiles: unlisted, no variants,
canonical plus compatible alias, unique alias, ambiguous aliases, contradictory
canonical plus unique alias, and API failure. Opaque variant IDs represent
catalog-backed choices, not provider-specific mapping tables. Native selection
is a separate request kind. Create and resume update are distinct observable
events with the same admission contract. Plain/no-model/unknown-colon requests
share the no-catalog abstraction. Invalid syntax is a pre-setup rejection.

The state records readiness, current model, chosen wire variant, catalog count,
selection mutations, prompt submissions, replay barrier and cooled base models.
Counters start at this request, not at the beginning of a retained conversation.
Resume scenarios establish a session first and assert deltas from that baseline.
Transitions expose setup readiness, capability resolution/rejection, create or
resume update, prompt submission, barrier observation, genuine no-route,
permanent auth/request failure, success, and gated fallback. Fallback clears
resolution and readiness, so the next model must undergo its own capability
check. A base-model cooldown applies to all its suffix forms. The modeled
`cooled` set observes only identities abandoned during a switch, not the entire
production cooldown map. Cooling a missing model when no switch is possible is
covered by the retry unit tests, not asserted absent by either model. Policy/budget/
fallback-enable/candidate gates are nondeterministic initial inputs, not facts
that force successful switching. Catalog profiles are independent per model.
The modeled fallback preserves one request kind and tier, representing common
effort carried through provider-wildcard expansion. It does not require arbitrary
configured candidate arrays to share an effort. Different per-candidate tiers
are covered by the production workflow/plan array integration tests.

Quint bounded verification checks `safety` through **12 transitions**, using
**Apalache 0.62.1**. The independent simulation uses seed **20261024**,
**1000 samples**, **12 steps**. Simulation is not proof. Alloy checks exactly
**13 ordered states**, hence **12 transitions**, signed **4-bit integers**, with
bundled **SAT4J**. Counters cannot exceed two. Idle steps permit shorter traces
without making witnesses vacuous. The fallback witness requires two prompts,
two catalog requests, different wire variants, final success on the spare, and
cooldown on the primary. Each assertion expects UNSAT and each witness SAT.

These are bounded executable contracts, not unbounded proofs or a refinement
proof connecting Rust to a transition system. They do not prove liveness,
network reliability, arbitrary catalog size, crash durability, arbitrary-length
fallback chains or actual provider availability. Diagnostic status precedence,
exact-key/wildcard string rebuilding and numeric/case aliases are exercised
against production Rust rather than copying those algorithms into a model.

## Pins and exact commands

Existing reproducible dependencies are reused without adding another toolchain:
Node **24.22.0**, npm **11.x** (bundled with Node 24.22.0), Quint **0.33.0** in
`tests/formal/package{,-lock}.json`, Apalache **0.62.1** selected explicitly by
`scripts/formal/verify_quint.cjs`, Temurin **25.0.5+8-LTS**, Alloy **6.2.0**,
Rust/Cargo **1.99.1**, Cucumber **0.23.0** in `Cargo.toml` and `Cargo.lock`.
The official `@opencode/cli` **2.1.4** and all platform archives are independently
integrity-locked in `tests/formal/opencode-cli/package-lock.json`. This separate
install does not enlarge the other formal jobs. The runner uses
`npm ci --ignore-scripts`, never the package's network-fallback postinstall,
and selects `@opencode/cli-darwin-arm64/bin/opencode` on macOS or portable
`@opencode/cli-linux-x64-baseline/bin/opencode` on Linux. A missing binary or
wrong reported version fails the suite, with no globally installed CLI fallback.
`scripts/setup_formal.sh` validates the released Alloy JAR SHA256
`6b8c1cb5bc93bedfc7c61435c4e1ab6e688a242dc702a394628d9a9801edb78d`
and both platform JDK archive SHA256 pins before extraction. It runs
`npm ci --ignore-scripts`, including the repository's locked `adm-zip` override.

The runner captures actual installed CLI help and versions. Alloy **6.2.0**
uses `help exec` and has **no `--expect` CLI option**. Its documented equivalent
is `expect 0`/`expect 1` on explicitly selected model commands. The receipt
validator checks all command kinds, expectations and concrete witness instances,
rejecting counterexamples, missing/extra results and malformed receipts, even
if Alloy exits zero. Its 13 negative/positive tests also run in this suite.
Cucumber uses `fail_on_skipped()` and `run_and_exit`, with scenario retries
disabled. The actual executable's undefined-step and assertion-failure negative
controls must each exit 101 at the intended step, never at build/setup time.

Here `Q=tests/formal/node_modules/.bin/quint`, `R` is a fresh report directory,
and `PORT` is an OS-selected isolated loopback endpoint. `OPENCODE` is the
platform-specific native executable described above. The verifier wrapper
logs exact absolute argv, starts the explicitly selected backend and retains
Apalache's own logs inside the report directory.

```sh
bash scripts/setup_formal.sh
source target/formal-tools/env.sh
npm ci --prefix tests/formal/opencode-cli --ignore-scripts
"$Q" --version
java -version
java -jar target/formal-tools/alloy-6.2.0.jar version
"$Q" typecheck --help
"$Q" test --help
"$Q" run --help
"$Q" verify --help
java -jar target/formal-tools/alloy-6.2.0.jar help exec
java -jar target/formal-tools/alloy-6.2.0.jar solvers
"$Q" typecheck tests/formal/opencode_effort.qnt
"$Q" test tests/formal/opencode_effort.qnt --backend typescript --seed 20261024 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261024 --max-samples 1000 --max-steps 12 --invariant safety --backend typescript --out-itf "$R/simulation_{seq}.itf.json" tests/formal/opencode_effort.qnt
"$Q" verify --invariant safety --max-steps 12 --main opencode_effort --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json" tests/formal/opencode_effort.qnt
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/opencode_effort.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/opencode_effort.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test opencode_bdd
node scripts/formal/check_bdd_failures.cjs "$R"
cargo +1.99.1 test --locked --all-features --lib backend::opencode::tests::
cargo +1.99.1 test --locked --all-features --lib retry::
cargo +1.99.1 test --locked --all-features --test opencode_sdk_cli --test opencode_runtime_contract --test opencode_state_contract --test opencode_config_contract
env OPENCODE_REAL_CLI="$OPENCODE" cargo +1.99.1 test --locked --all-features --test opencode_real_cli -- --ignored --nocapture
```

The single aggregate command at the top provides all variables and prerequisites.
See `OPENCODE_EFFORT_VERIFICATION.md` for observed results and blockers.
