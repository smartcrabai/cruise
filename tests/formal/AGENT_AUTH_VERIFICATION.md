# Agent authentication specification execution receipt

## Observed local execution, 2026-10-24 UTC

Completed on macOS aarch64, from this working repository. The full CI-equivalent
run started at **14:36:50 UTC** and finished at **14:39:40 UTC**, exit **0**:

```sh
export PATH="$HOME/.local/share/mise/installs/node/22.22.0/bin:$PATH"
bash scripts/check_agent_auth_specs.sh
```

Evidence directory: `target/formal-reports/agent-auth.Jl8W4U/`.
`commands.log` contains every exact shell-escaped invocation, `results.log`
contains every exit status, and `quint-verify.log` contains the expanded backend
process argv. `AGENT_AUTH.md` documents the commands in portable repository-root
form, pinned installation, model assumptions and requirement-to-check map.
These are observed local results, not a claim that hosted GitHub CI has run.

## Tool and runtime versions actually used

| Component | Version |
| --- | --- |
| Node | 22.22.0 |
| npm | 10.9.4 |
| Quint | 0.33.0, npm-lock installed with `npm ci --ignore-scripts` |
| Verification backend | Apalache 0.62.1, explicitly selected, fresh endpoint `127.0.0.1:54556` |
| Java | Temurin OpenJDK 25.0.5+8-LTS, release 2026-10-20 |
| Alloy released CLI artifact | 6.2.0, checksum-verified JAR |
| Alloy solver | SAT4J from that pinned JAR |
| Rust | rustc 1.99.1 (58cf8f9c3 2026-10-13) |
| Cargo | 1.99.1 (5f94df478 2026-08-27) |
| Cucumber | 0.23.0, exact existing Cargo pin and lockfile |
| tempfile | 3.27.0, exact existing Cargo pin and lockfile |

The existing `scripts/setup_formal.sh` validates the JAR/JDK archive checksums
and selected JDK runtime. `AGENT_AUTH.md` records the exact SHA256 pins. No
additional dependency service, provider account, API key, or secret was used.

## Results

All 22 aggregate stages, including setup, tool versions and CLI help, exited
zero. Substantive checks:

| Actual check | Observed result |
| --- | --- |
| `quint typecheck tests/formal/agent_auth.qnt` | Pass |
| `quint test tests/formal/agent_auth.qnt --backend typescript --seed 20261024` | 13 tests passed |
| `quint run --seed 20261024 --max-samples 1000 --max-steps 8 --invariant safety --backend typescript tests/formal/agent_auth.qnt` | No violation in 1000 samples, trace lengths all 9 states |
| `quint verify --invariant safety --max-steps 8 --main agent_auth --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint 127.0.0.1:54556 tests/formal/agent_auth.qnt` | Apalache `NoError`, no safety violation through 8 transitions |
| `java -jar target/formal-tools/alloy-6.2.0.jar help exec` | Inspected actual installed help, explicit wildcard command selection supported, no CLI `--expect` flag |
| `java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/agent_auth.als` | All 17 named commands executed |
| `node scripts/formal/check_alloy_receipt.cjs tests/formal/agent_auth.als "$R/alloy/receipt.json"` | 5 UNSAT assertions and 12 SAT witnesses validated |
| `node --test scripts/formal/check_alloy_receipt.test.cjs` | 13 validator self-tests passed, including rejection of malformed/missing/extra/wrong results and weakened expectations |
| `cargo +1.99.1 test --locked --all-features --test agent_auth_bdd` | 31 scenarios, 130 steps, all passed, no undefined/pending/skipped steps |
| `node scripts/formal/check_agent_auth_bdd_failures.cjs "$R"` | Undefined, deliberately unimplemented/pending, false assertion, and empty selection all rejected with expected exit 101 and diagnostic markers |
| `cargo +1.99.1 test --locked --all-features --test agent_auth_cli` | 3 tests passed |
| `cargo +1.99.1 test --locked --all-features --lib` | 1651 passed, 0 failed, 3 pre-existing ignored child-process probes |

Here `quint` is `tests/formal/node_modules/.bin/quint`, `R` is the evidence
directory above, and the commands also used their documented `--out-itf` paths
as recorded verbatim in `commands.log`. The verifier runs from the report
directory with absolute repository/model paths to retain its server output
under `R/_apalache-out/`, not in the repository root. The three ignored library
probe functions are unrelated existing child-only harnesses. No new formal,
Gherkin or authentication CLI check was skipped or treated as a pass.

**Simulation is not proof.** Its seed is **20261024**, with **1000 samples** and
**8 transitions** per sample. The independent symbolic verification bound is
**8 transitions**, using **Apalache 0.62.1**. Alloy checks exactly **9 ordered
states**, thus **8 transitions**, using the finite account/candidate abstraction
documented in `AGENT_AUTH.md`. This is not an unbounded proof or a formal
refinement proof of the production implementation.

## Inspected Alloy receipt

Raw receipt: `target/formal-reports/agent-auth.Jl8W4U/alloy/receipt.json`.
The pinned CLI implements expectations as model-command `expect 0` / `expect 1`,
not a nonexistent CLI switch. Every command was explicitly selected and its
receipt inspected by the fail-closed validator, not inferred from CLI exit 0.

| Receipt command | Expected | Observed |
| --- | --- | --- |
| ScopeSafety | 0 counterexamples | UNSAT |
| ExactDiagnostics | 0 counterexamples | UNSAT |
| CompleteBeforeDecision | 0 counterexamples | UNSAT |
| Admission | 0 counterexamples | UNSAT |
| EffectGate | 0 counterexamples | UNSAT |
| AllTargetsRejected | 1 witness | SAT, concrete instance retained |
| RenewableAccountLaunch | 1 witness | SAT, concrete instance retained |
| UnknownAccountLaunch | 1 witness | SAT, concrete instance retained |
| PlanningIsolation | 1 witness | SAT, concrete instance retained |
| DryWithoutReads | 1 witness | SAT, concrete instance retained |
| PlanningDryWithoutReads | 1 witness | SAT, concrete instance retained |
| OptOutLaunch | 1 witness | SAT, concrete instance retained |
| CommandLaunch | 1 witness | SAT, concrete instance retained |
| UnsupportedLaunch | 1 witness | SAT, concrete instance retained |
| AlternativeAuthLaunch | 1 witness | SAT, concrete instance retained |
| RefreshExpiryRejected | 1 witness | SAT, concrete instance retained |
| FreshAccessLaunch | 1 witness | SAT, concrete instance retained |

Receipt SHA256:
`0828aacddaed41dbd98ced1fbe0a26e3648c344e3e547aea34df5cbcc2b7d291`.
Verified Quint model SHA256:
`6173577c55258e6a12cb41546e2add36c1a9f0f6d6a844e735d378fe1d9e6d85`.
Checked Alloy model SHA256:
`6d51db1aec032c75ee4802f4aac7e4308fa6da807eb76688ebaa4eb165410c8b`.

## Production coverage, fixes and remaining prerequisites

Gherkin uses actual `cruise plan`, `run` and `exec` processes, real YAML config
resolution, temporary credential files and real session/worktree observations.
Only the final external SDK subprocess is a local deterministic refusal fixture.
Passing cases must reach that observable boundary, not merely parse a feature
or complete a dry run. Rejection cases assert absent dispatch/workspace effects,
file immutability and absence of token values. Nested/after-pr/group candidates,
SDK-qualified arrays, fallback chains, both OAuth providers, multi-account
renewable/unknown admission and deduplicated all-target diagnostics are covered.
The full library run also exercises shared planning and TUI/WebUI setup contracts.

No new production defect was found, and no production source change was needed
for this specification task. During grounding, the draft models were made more
faithful to the actual contract: planning's early dry-run return is included,
and jcode refresh expiry is fixed to unknown because those file schemas have no
such timestamp. No failing invariant or assertion was weakened to obtain a pass.
Existing source/docs edits were preserved. No commits, HEAD changes, local
branch-ref moves, or commit-guard changes were performed.

CI file changed: **`.github/workflows/ci.yml`**, new **`agent-auth-specs`** job.
It invokes the full aggregate runner on ordinary PRs/pushes without secrets and
uploads the entire evidence directory with `if: always()`. Existing shared
lockfiles and checksum-pinned setup are reused. `Cargo.toml` registers the
native BDD executable explicitly, with its existing pinned dependencies.

There is **no missing prerequisite on the local macOS arm64 platform**. Linux
x86_64 execution is configured in provider-native CI but has not been run on
this Mac. Unsupported platforms fail explicitly rather than silently skipping
formal checks. Hosted CI execution/branch-protection configuration is not
claimed by local validation.

Additional checks run from this repository and passed:

```sh
cargo +1.99.1 fmt -- --check
cargo +1.99.1 clippy --locked --all-targets --all-features -- -D warnings -A clippy::large_futures
git diff --check
bash -n scripts/check_agent_auth_specs.sh
shellcheck scripts/check_agent_auth_specs.sh
actionlint .github/workflows/ci.yml
taplo lint Cargo.toml
taplo fmt --check --option reorder_keys=true Cargo.toml
```

The all-target Clippy output is retained in the evidence directory's `clippy.log`.
Repository formatting, Cargo TOML checks, strict Clippy and ordinary Rust tests
remain connected to the existing native `lint` and `test` CI jobs. The new
formal/BDD suite and its negative controls are connected to `agent-auth-specs`.
