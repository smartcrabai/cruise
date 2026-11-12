# Checkpoint contract verification

For the WebUI daemon lifecycle, see `WEBUI_DAEMON.md` and run `bash scripts/check_webui_daemon_specs.sh`.
The `webui_daemon.qnt` model checks single ownership, start eligibility,
identity-checked signals, session preservation and status at the observation
point. Its two-PID, two-generation bounded abstraction does not model network
authentication, timeouts, or prove the implementation correct. The script also
runs the real CLI and control endpoint integration tests. Reports and command
logs are written under `target/formal-reports/`; the dedicated CI workflow
uploads them even on failure.

For provider installation lifecycle and resolution precedence, see
`PROVIDER_INSTALL.md` and run `bash scripts/check_provider_install_specs.sh`.
The dedicated `provider-install-specs` CI job runs the pinned Quint checks and
retains reports. The bounded model is not a production refinement proof.

For the planned workflow environment override precedence and snapshot model,
see `CONFIG_ENV_OVERRIDE.md` and run `bash scripts/check_config_env_specs.sh`.
This standalone Quint model is not a production conformance test.

For local squash merge lifecycle safety, run `bash scripts/check_local_merge_specs.sh`.
The dedicated `local-merge-specs` CI job runs the pinned Quint tests, simulation
and bounded verification, all Alloy assertions and witnesses with receipt
validation, and the local merge Gherkin target. Fresh evidence is retained under
`target/formal-reports/local-merge.*`.
The models cover refusal and rollback, at-most-one commit, marker-before-cleanup,
session deletion after cleanup, and safe commit guards, including a cleanup-failure
retry witness. Quint explores eight steps; Alloy uses eight ordered states.
Exact commands, pins, local results and receipt details are recorded in
`LOCAL_MERGE_VERIFICATION.md`.
These are bounded abstractions, not a proof of the Rust implementation. In
particular, failure or crash between the base commit and marker persistence is
not modeled, and the modeled cleanup failure leaves both resources intact.

For bounded batch auto-requeue safety, see `AUTO_REQUEUE.md` and run
`bash scripts/check_auto_requeue_specs.sh`. The dedicated `auto-requeue-specs`
CI job retains verification receipts and traces.

For the bounded PR publication recovery lifecycle, run `bash scripts/check_pr_failed_specs.sh` with the pinned toolchain described below. The suite checks that Create PR does not rerun the workflow, cancellation retains PR Failed, worktrees remain available, and completion has a PR or a no-commits skip. It includes Quint, Alloy assertion and witness receipts, and executable production application scenarios. These bounded abstractions are not a proof of the production implementation. Reports are retained under `target/formal-reports/pr-failed.*`, and the `agent-auth-specs` CI job runs the suite independently of the preceding authentication check.

For PR review-fix permission filtering and bounded termination, see
`PR_REVIEW_FIX.md` and run `bash scripts/check_pr_review_fix_specs.sh`.
The `pr-review-fix-specs` CI job runs Quint, Alloy receipt checks and
production-connected Gherkin.

For task-scoped `subagent_model` selection, backend configuration and exclusions,
see `SUBAGENT_MODEL.md` and run `bash scripts/check_subagent_model_specs.sh`.
The dedicated `subagent-model-specs` PR job runs pinned Quint verification,
Alloy assertions and witnesses with receipt validation, production Gherkin and
Rust CLI/API regression tests. Local evidence is in
`SUBAGENT_MODEL_VERIFICATION.md`.

For per-prompt `disallowed_tools`, implicit git denials and MCP filtering, see
`TOOL_RESTRICTIONS.md` and run `bash scripts/check_tool_restrictions_specs.sh`.
The dedicated `tool-restrictions-specs` native PR job runs Quint, all Alloy
assertions and witnesses with receipt validation, production Cucumber scenarios,
negative controls and affected Rust contracts using the shared pins below.

For responsive WebUI drawer transitions, width persistence, real browser layout
and htmx Gherkin, see `RESPONSIVE_WEBUI.md` and run
`bash scripts/check_responsive_webui_specs.sh`. The dedicated secret-free
`responsive-webui-specs.yml` workflow executes all checks and validates Alloy
assertion and witness receipts.

For OpenCode common effort resolution and replay-safe `provider.no-route`
fallback, see `OPENCODE_EFFORT.md` and run
`bash scripts/check_opencode_effort_specs.sh`. The dedicated native CI job runs
Quint, Alloy receipt validation, real-Executor Cucumber, and affected Rust
contracts using this directory's shared locked toolchain.

For workflow-start OAuth authentication preflight, including bounded Quint and
Alloy checks and production CLI Gherkin, see `AGENT_AUTH.md` and run
`bash scripts/check_agent_auth_specs.sh`. Its dedicated `agent-auth-specs` PR CI
job uses the same pinned toolchain and validates every Alloy receipt command.

For flag-dependent skip candidates and flags-before-skips approval, see
`FLAG_SKIP_SELECTION.md` and `FLAG_SKIP_SELECTION_VERIFICATION.md`. The same
`bash scripts/check_workflow_flags_specs.sh` command now requires the separate
skip-selection Quint/Alloy models, production approval Gherkin and UI/application
regressions in the existing `workflow-flags-specs` PR job.

For the CLI planning-to-execution workflow-flag boundary, see
`WORKFLOW_FLAGS.md` and run `bash scripts/check_workflow_flags_specs.sh`.
Its dedicated PR CI job runs Quint, Alloy receipt checks, real PTY Gherkin,
and production regression tests with the shared pins below.

For SDK-qualified model arrays, ordered cross-SDK fallback, resume ownership,
and production-connected configuration/CLI Gherkin, see `SDK_MODELS.md` and run
`bash scripts/check_sdk_model_specs.sh`. This suite has its own required native
PR CI job and uses the same pinned formal toolchain and receipt validator.

For the separate OpenCode V2 admission, retry and lifecycle specifications, see
`OPENCODE.md` and run `bash scripts/check_opencode_specs.sh`. Both suites are
required by the provider-native formal CI job. OpenCode execution evidence is
recorded in `OPENCODE_VERIFICATION.md`.

## Quality-gate checkpoint abstraction (CP-09 through CP-11)

`checkpoint.qnt` and `checkpoint.als` extend the existing checkpoint model with
gate failures, shared retry accounting, checkpointed feedback, and exhausted
goto and termination. The runner verifies CP-09 through CP-11 and checks Alloy
receipts, including retry/resume and both exhaustion witnesses. The separate
`quality_gate` models are supplementary, not the CP numbering authority.

| ID | Bounded property | Concrete coverage |
| --- | --- | --- |
| CP-09 | Retry requires available shared budget; exhaustion routes to goto or termination without another source effect | `quality_gate_cli::exhaustion_stops_without_running_the_next_step`, `zero_budget_fails_without_retry` |
| CP-10 | Feedback is saved with pending retry, survives resume, and clears on non-retry transitions | `quality_gate_graph::checkpoint_roundtrips_gate_feedback_pending_retry_and_budget`, `quality_gate_cli::failure_retries_with_feedback_and_clears_it_for_next_step` |
| CP-11 | An accepted gate retry consumes exactly one shared budget unit without exceeding its ceiling | `quality_gate_cli::exhaustion_stops_without_running_the_next_step` |

The models are abstractions, not a proof of Rust refinement. Feedback is a
boolean, not command output or its redaction. The checkpoint model bounds counts
and limits at two and abstracts goto as the existing forward exit. It omits
multiple gates, timeouts, and explicit reposition. Resume models only a successfully
saved state. There is no claim of exactly-once shell effects across a crash
between an external effect and checkpoint persistence. Concrete contract tests
cover additional behavior separately and are not replaced by the models.

## Scope and traceability

CP-01 through CP-08 are reconstructed repository contract identifiers, not recovered labels from an unavailable historical audit. Each ID has a Quint invariant, an Alloy assertion and a production-connected Cucumber scenario in `../features/checkpoint.feature`, implemented in `../checkpoint_bdd.rs`. The fixtures compile real YAML, invoke the production engine with real POSIX shell effects, save/load real checkpoints and execute Rust assertions. No parser-only or dry-run substitute is used.

| ID | Contract | Production paths | Existing evidence in `../graph_state.rs` |
| --- | --- | --- | --- |
| CP-01 | Count accepted ordered-pair traversals, preserve history across lowered ceilings, deny without consuming budget | `src/engine.rs::accept_transition`, `src/graph.rs::restore` | `repeated_resume_never_replenishes_accepted_retry_budget` |
| CP-02 | Accept saved pending outcomes without executing their source again | `src/engine.rs::execute_steps_with_graph`, `accept_transition` | pending-transition assertions in `repeated_resume_never_replenishes_accepted_retry_budget` |
| CP-03 | Persistence failure stops execution before the next effect | `src/engine.rs::save_checkpoint`, `execute_steps_with_graph`, `src/graph/persistence.rs::save_graph` | `failed_checkpoint_prevents_the_next_side_effect` |
| CP-04 | Skip fallback records traversal, spends no budget and rejects overflow without wrapping | `src/engine.rs::step_loop_iteration`, `accept_transition` | `skipped_transitions_are_recorded_without_consuming_budget_or_wrapping` |
| CP-05 | Saved position/runtime override stale display state; explicit reposition retains history | `src/graph/persistence.rs::prepare_resume`, `src/graph.rs::restore`, `src/engine.rs::execute_steps_with_graph`, `src/application.rs::session_dag` | `checkpoint_outvotes_stale_session_position_and_preserves_invalid_files`, `graph_view_uses_a_checkpoint_when_session_display_state_is_stale` |
| CP-06 | Reject malformed, legacy, incompatible and incomplete checkpoints without rewriting evidence | `src/graph/persistence.rs::load_graph`, `validate_checkpoint`, `prepare_resume`, `src/graph.rs::restore` | incomplete/runtime/pending/legacy/mismatched/control-structure checkpoint tests |
| CP-07 | Group retries retain accepted counters and runtime file snapshots; exhaustion skips the body | `src/engine.rs::accept_transition`, `check_group_retry_skip`, `src/graph.rs::restore`, `src/file_tracker.rs::restore_snapshots` | `group_counts_and_snapshots_survive_resume_and_limit_skip` |
| CP-08 | Retry limits do not unfold topology; completed automatic resume performs no effects | `src/graph.rs::build_graph`, `src/engine.rs::execute_steps_with_graph` | `large_chain_and_scc_use_fixed_topology`, plus completed-resume Cucumber assertions |

The engine checks budget before incrementing counters, uses checked additions, saves rejected pending outcomes and saves accepted transitions before continuing. A completed graph bypasses the execution loop. `prepare_resume` treats node-ID/automatic position as checkpoint-authoritative, while a human step-name override explicitly clears pending/completed position but retains history. The models deliberately exclude this reposition operation. Runtime restoration is exercised with fresh runtime objects and stale caller variables, not just equality of synthetic records.

## Pins and real commands

Run from the repository root with Node **26.13.0** on PATH and Rust **1.99.1** installed. Run Cargo-based checks sequentially when they share `CARGO_TARGET_DIR`: concurrent default-feature, all-feature and A/B smoke builds can replace the sibling CLI binaries that integration tests execute. CI jobs have separate runner filesystems, and the existing production job runs tests before the smoke build.

```sh
bash scripts/check_formal.sh
```

`setup_formal.sh` checksum-validates Alloy **6.2.0** and Temurin **25.0.5+8**, extracts the verified JDK, checks its runtime version, and uses `npm ci --ignore-scripts` with the committed npm lock for Quint **0.33.0**. Apalache **0.62.1** is explicitly selected on a fresh loopback endpoint. Java 17 is not the selected runtime. Cargo pins Cucumber **0.23.0**, with transitive dependencies recorded in the root `Cargo.lock`. The npm lock overrides `adm-zip` to **0.6.1**.

The runner captures installed CLI help and versions. Alloy 6.2.0 uses `help exec`, not `exec --help`. Its help confirms wildcard command selection. Quint options below follow the installed 0.33.0 help. `Q` denotes `tests/formal/node_modules/.bin/quint`, `R` the newly allocated report directory, and `PORT` the OS-selected isolated port:

```sh
"$Q" typecheck tests/formal/checkpoint.qnt
"$Q" test tests/formal/checkpoint.qnt --main checkpoint --backend typescript --seed 20261023 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run tests/formal/checkpoint.qnt --main checkpoint --init init --step step --invariant safety --backend typescript --seed 20261023 --max-samples 100 --max-steps 12 --out-itf "$R/simulation_{seq}.itf.json"
"$Q" verify tests/formal/checkpoint.qnt --main checkpoint --init init --step step --invariant safety --invariants cp01_accounting cp02_pending cp03_checkpoint_gate cp04_skip cp05_resume cp06_reject cp07_group cp08_completed cp09_gate_route cp10_feedback cp11_gate_budget --max-steps 12 --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json"
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --type json --output "$R/alloy" tests/formal/checkpoint.als
cargo +1.99.1 test --locked --test graph_state --test checkpoint_bdd
```

Alloy 6.2.0's `exec` help exposes no `--expect` option. Instead each model command declares `expect 0` or `expect 1`, and the runner independently compares every command and expectation to its receipt. The supplementary `quality_gate` models are also typechecked, tested, simulated, verified and checked with the same bounds and pins. The verification command checks all named invariants together, not by simulation. The separate seeded simulation is only supplementary. Quint executable tests provide retry/resume, skip/completion, group runtime, group exhaustion, shared-budget and zero-budget witnesses. Cucumber's `fail_on_skipped()` rejects undefined/pending/skipped steps and `run_and_exit` fails on assertion errors.

## Bounds and assumptions

Both models use two fixed executable identities, one retry pair, one forward pair, one group site, policy/count limits 0 through 2, and effect/runtime tokens 0 through 4. `current = 2` denotes terminal completion, not a third executable node. Quint checks 12 transitions. Alloy uses exactly 13 ordered states, hence 12 transitions, with signed 4-bit integers. All reachable arithmetic stays within that representation. Idle steps permit shorter behavior to be padded to the Alloy scope. The bound includes two accepted group retries followed by exhaustion skip and terminal execution, as witnessed by both models.

Successful checkpoint writes are atomic in the abstract machine. Acceptance plus its successful post-transition checkpoint is one abstract action. Failed persistence cannot authorize another effect. Resume denotes restoring a successfully saved authoritative state, not a crash transition that guesses unsaved state. Invalid input handling is an abstract rejection action, with actual JSON/schema/identity/topology rejection tested by Cucumber. Opaque runtime tokens represent preservation, not filesystem-content equivalence. Save/policy/failure actions deliberately allow additional interleavings beyond the normal engine loop.

There is no exactly-once guarantee across an external effect followed by a crash before persistence, no fsync/durability proof, fairness/liveness proof, arbitrary-topology proof or unbounded integer proof. Manual reposition and `usize` overflow are concrete regression assertions, not claims proved by the scalar models. These are bounded executable specifications plus production regression tests, not a formal refinement proof of all Rust behavior.

## CI and retained evidence

`.github/workflows/ci.yml` runs the provider-native `formal` job on normal relevant pull requests and pushes to `main`, using pinned Node/Rust and `scripts/check_formal.sh`. A nonzero check accumulates a nonzero aggregate status while independent checks continue. `actions/upload-artifact` runs with `if: always()`, retains `target/formal-reports/` for 14 days, and therefore includes completed logs even after a model/test failure. Cancellation or failures before any report can be created cannot manufacture reports.

The formal CI jobs cache Rust 1.99.1 component archives by OS, architecture and profile; rustup still validates archive checksums during installation. The checksum-pinned JDK and Alloy downloads are cached separately by the setup-script hash, and `setup_formal.sh` verifies SHA-256 after every restore. It also stages Quint's Apalache 0.62.1 distribution from its cached, checksum-verified release archive (SHA-256 `38202a8c4ebeb3ab73e0fe5a7e4bb948442d46ad5de8a7fb45c9e5cac2beb2a6`) so Quint avoids its checksum-free on-demand fetch. `setup-node` continues to cache npm by lockfile integrity. Responsive CI caches the Chromium revision selected by the locked Playwright package and runs a real browser launch check; hosted CI avoids Playwright's apt update step, while local Linux runs still use `--with-deps`.

Each runner invocation allocates a fresh `target/formal-reports/run.*` directory. It retains exact command lines, versions, CLI help, individual stdout/stderr logs, status receipts, Quint test/simulation traces, Alloy `receipt.json` and SAT witness JSON. A counterexample ITF is retained when the verifier emits one. Its absence on successful verification is not evidence of a missing failure log. The nested verifier's exact process argv and isolated endpoint are in `quint-verify.log`. Alloy expectations are checked against all named commands: eleven checkpoint assertion checks require UNSAT (`expect 0`), and nine checkpoint witnesses require SAT (`expect 1`), with missing/duplicate/extra command results rejected. The supplementary quality-gate model adds three assertions and three witnesses.

For the quality-gate execution results, see `QUALITY_GATE_VERIFICATION.md`. See `VERIFICATION.md` for earlier checkpoint execution evidence and external constraints. No remote CI execution is implied by local checks.
