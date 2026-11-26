# Prompt tool-restriction executable contract

## Authority and scope

This suite derives from the task plan `20261025083954135_f048ddfb6aad465da82473f002382f34/plan.md`, R2-R6 and R8, especially sections 2.1-2.5, 3.1, 3.5-3.9 and 5. Repository authority includes `README.md` Tool restrictions/Permission modes and `NEVER.md`. No allowlist, general sandbox, user-level MCP revocation, builtin-turn restrictions, or new git prohibition is introduced.

The models describe the observable transition from a configured prompt to a resolved policy (or command-backend rejection), followed by one requested tool operation being authorized or blocked. They are deliberately not a translation of Rust fields or shell-parser code. Six restriction identities represent web, subagent, all workflow MCP, one named workflow MCP, an explicit publish prefix, and an explicit git-mutation prefix. Seven operation classes distinguish those tools, two workflow MCP servers, publish, a listed implicit git operation, and harmless shell. The git class is **only** the agreed implicit list, not every git operation. Concrete tests cover the entire list and permitted near-matches.

Inputs are arbitrary subsets at defaults, workflow and prompt-child layers, explicit workflow presence (including an empty list), full/restricted permission, `allow_commit`, and SDK/command selection. Existing permission validation precedes this contract: command backends require full permission. Both workflow MCP servers are declared. Unknown MCP names and invalid restriction syntax are production configuration errors exercised by Rust tests, not assumed valid strings in the model. Read-only and edit collapse to restricted because both disable shell for these decisions. Their distinct filesystem-tool mappings remain concrete Rust obligations.

The `effect` bit denotes authorization to dispatch through an **active** enforcement path, not a guarantee against arbitrary external effects. The model does not assume the tool layer is a security boundary. Jcode requires an active pre-tool hook. Disabled hooks, arbitrary shell semantics, personal backend MCP, backend bugs and deliberate bypass are outside the formal abstraction. The commit guard and its prompt instruction remain unchanged. Builtin planning/title/PR turns remain unrestricted, as required by R7.

## Traceability

| Contract | Quint obligation / tests | Alloy assertion / SAT witness | Production boundary and concrete coverage |
| --- | --- | --- | --- |
| Defaults apply only when absent, explicit empty overrides, child cannot loosen inherited denials | `denyOnly`, `exactDecisions`, `unionTest`, `emptyOverrideTest` | `DenyOnly`, `ExactDecisions`, `UnionBlockedWitness`, `EmptyOverrideWitness` | `workflow_defaults.rs`, `ToolPolicy::for_prompt_step`, Cucumber policy scenarios and full library tests |
| `allow_commit` removes only implicit git denials, never explicit restrictions or restricted permission | `denyOnly`, `explicitSurvivesCommitTest`, `restrictedTest`, `safeShellTest` | `DenyOnly`, `ExplicitSurvivesCommitWitness`, `RestrictedWitness`, `SafeShellWitness` | `tool_policy.rs`, `config.rs`, jcode read-only `replace` and backend mapping tests |
| Remove only selected workflow MCP servers, or all when requested | `mcpScope`, `allMcpTest` | `McpScope`, `AllMcpWitness`, `UnionBlockedWitness` | `filter_mcp_servers`, `engine::run_prompt_step`, Cucumber asserts names and preserved configuration |
| Command backend rejects explicit denials, ignores implicit-only git rules | `commandAdmission`, `commandRejectTest`, `commandImplicitIgnoredTest` | `CommandAdmission`, `CommandRejectWitness`, `CommandImplicitIgnoredWitness` | real config validation and executor dispatch unit contracts |
| Denied operations never receive dispatch authorization, harmless operations are not overblocked | `effectGate`, `exactDecisions`, `safeShellTest` | `EffectGate`, `ExactDecisions`, blocked and allowed witnesses | real `cruise internal jcode-pre-tool` exit 0/2, matched source diagnostics, normalization, malformed JSON fail-closed, non-bash/no-policy bypass |
| Parsing and placement, parallel/group/after-PR/workflow-call propagation, jcode mapping and unchanged builtin policy | Concrete regression obligations, not claimed proved by finite models | Not abstracted | `tool_restrictions_config.rs`, `tool_restrictions_cli.rs`, full `--lib` tests across config/workflow/engine/executor/jcode |

Cucumber scenarios are in `tests/features/tool_restrictions.feature`, definitions in `tests/tool_restrictions_bdd.rs`. They invoke real production policy APIs and the shipped CLI gate, asserting policy/MCP outputs and actual process status/stderr. No SDK account or model response is needed. The runner rejects skipped/undefined/pending steps and assertions. Cucumber 0.23 has no separate pending-step status: unimplemented steps are skipped, and `fail_on_skipped()` makes them failures. Separate executable negative controls verify undefined steps, real assertion failures, malformed feature syntax and an empty filtered suite all fail without retry. This is production-connected contract testing, not a claim to exercise a live hosted model.

[`tool_restrictions_stdin.qnt`](tool_restrictions_stdin.qnt) models a separate BDD harness contract: the write and child exit may race, but only fixtures declared to bypass stdin may tolerate `BrokenPipe`. The existing CI job typechecks the model, runs its eight schedule and error tests, and verifies `safety` to four transitions. The production BDD also forces child-first order in all four early-exit cases; a wrong gate result still fails the output assertion.

## Reproducible execution and pins

From the repository root, with Node **26.13.1** on PATH and Rust **1.99.1** installed:

```sh
bash scripts/check_tool_restrictions_specs.sh
```

The suite reuses `scripts/setup_formal.sh`, `tests/formal/package.json` and the committed `package-lock.json`: Quint **0.33.0**, Node **26.13.1**, Apalache **0.62.1**, Alloy **6.2.0**, Rust **1.99.1**, and Cucumber **0.23.0**. Cucumber and tempfile are exact locked Cargo dependencies. It requires no provider CLI, provider credentials or paid inference.

The runner captures installed versions and help for all four Quint commands plus `java -jar ... help exec`. **Alloy 6.2.0 does not have a CLI `--expect` option**. Its actual supported equivalent is `expect 0` on every assertion command and `expect 1` on each witness in the model. `--command '*'` explicitly selects all commands. The independent receipt validator rejects missing/duplicate/extra results, malformed solutions, expectation disagreement, SAT assertions or UNSAT witnesses. Its own regression tests run in this suite.

Exact commands (`Q=tests/formal/node_modules/.bin/quint`, `R` is a fresh report directory):

```sh
"$Q" typecheck tests/formal/tool_restrictions.qnt
"$Q" test tests/formal/tool_restrictions.qnt --backend typescript --seed 20261025 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run --seed 20261025 --max-samples 1000 --max-steps 4 --invariant safety --backend typescript --out-itf "$R/simulation_{seq}.itf.json" tests/formal/tool_restrictions.qnt
node scripts/formal/verify_quint.cjs "$Q" "$PWD/tests/formal/tool_restrictions.qnt" tool_restrictions 4 "$R/counterexample.itf.json"
# The helper logs and executes the exact argv equivalent to:
"$Q" verify --invariant safety --max-steps 4 --main tool_restrictions --init init --step step --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json" tests/formal/tool_restrictions.qnt
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --solver sat4j --type json --output "$R/alloy" tests/formal/tool_restrictions.als
node scripts/formal/check_alloy_receipt.cjs tests/formal/tool_restrictions.als "$R/alloy/receipt.json"
node --test scripts/formal/check_alloy_receipt.test.cjs
cargo +1.99.1 test --locked --all-features --test tool_restrictions_bdd
node scripts/formal/check_tool_restrictions_bdd_failures.cjs "$R"
cargo +1.99.1 test --locked --all-features --test tool_restrictions_cli --test tool_restrictions_config
cargo +1.99.1 test --locked --all-features --lib
```

The suite's Rust contracts cover policy-field wiring through jcode and command execution. Strict all-target Clippy and Rustfmt checks remain connected through the repository's existing native lint/test jobs.

Quint simulation uses seed **20261025**, **1000 samples**, at most **4 transitions** each. Simulation is not proof. Independent Apalache verification checks the conjunctive `safety` invariant at **4 transitions**. Alloy SAT4J checks **exactly 5 ordered states**, also **4 transitions**, with six fixed restriction identities and seven operation identities. Idle transitions pad shorter executions. There is no liveness/fairness, unbounded-system, full shell-language, or Rust-refinement proof. An assertion and its corresponding witness both execute, so a contradictory trace model cannot silently pass all assertions.

## CI and evidence

`.github/workflows/ci.yml` adds the `tool-restrictions-specs` native GitHub Actions PR/push job. It has read-only contents permissions, pinned Node/Rust installation, no secret-dependent step, and runs the entire suite above. It always uploads `target/formal-reports/tool-restrictions.*/` with missing evidence treated as an error. Each invocation creates a fresh directory, logs exact commands and statuses, preserves all stdout/stderr, installed CLI help, backend logs, and the Alloy receipt and concrete witness JSON. Independent checks continue after failure, but setup/platform failures block and any failed check makes the aggregate status nonzero.

See `TOOL_RESTRICTIONS_VERIFICATION.md` for the actual execution results. Local execution does not imply a remote CI run.
