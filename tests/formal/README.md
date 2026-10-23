# Checkpoint contract verification

For the separate OpenCode V2 admission, retry and lifecycle specifications, see
`OPENCODE.md` and run `bash scripts/check_opencode_specs.sh`. Both suites are
required by the provider-native formal CI job. OpenCode execution evidence is
recorded in `OPENCODE_VERIFICATION.md`.

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

Run from the repository root with Node **22.22.0** on PATH and Rust **1.99.1** installed. Run Cargo-based checks sequentially when they share `CARGO_TARGET_DIR`: concurrent default-feature, all-feature and A/B smoke builds can replace the sibling CLI binaries that integration tests execute. CI jobs have separate runner filesystems, and the existing production job runs tests before the smoke build.

```sh
bash scripts/check_formal.sh
```

`setup_formal.sh` checksum-validates Alloy **6.2.0** and Temurin **25.0.5+8**, extracts the verified JDK, checks its runtime version, and uses `npm ci --ignore-scripts` with the committed npm lock for Quint **0.33.0**. Apalache **0.62.1** is explicitly selected on a fresh loopback endpoint. Java 17 is not the selected runtime. Cargo pins Cucumber **0.23.0**, with transitive dependencies recorded in the root `Cargo.lock`. The npm lock overrides `adm-zip` to **0.6.1**.

The runner captures installed CLI help and versions. Alloy 6.2.0 uses `help exec`, not `exec --help`. Its help confirms wildcard command selection. Quint options below follow the installed 0.33.0 help. `Q` denotes `tests/formal/node_modules/.bin/quint`, `R` the newly allocated report directory, and `PORT` the OS-selected isolated port:

```sh
"$Q" typecheck tests/formal/checkpoint.qnt
"$Q" test tests/formal/checkpoint.qnt --main checkpoint --backend typescript --seed 20261023 --out-itf "$R/test_{test}_{seq}.itf.json"
"$Q" run tests/formal/checkpoint.qnt --main checkpoint --init init --step step --invariant safety --backend typescript --seed 20261023 --max-samples 100 --max-steps 12 --out-itf "$R/simulation_{seq}.itf.json"
"$Q" verify tests/formal/checkpoint.qnt --main checkpoint --init init --step step --invariant safety --invariants cp01_accounting cp02_pending cp03_checkpoint_gate cp04_skip cp05_resume cp06_reject cp07_group cp08_completed --max-steps 12 --backend apalache --apalache-version 0.62.1 --server-endpoint "127.0.0.1:$PORT" --out-itf "$R/counterexample.itf.json"
java -jar target/formal-tools/alloy-6.2.0.jar exec --command '*' --type json --output "$R/alloy" tests/formal/checkpoint.als
cargo +1.99.1 test --locked --test graph_state --test checkpoint_bdd
```

The verification command checks all named invariants together, not by simulation. The separate seeded simulation is only supplementary. The four named Quint executable tests provide retry/resume, skip/completion, group runtime and group exhaustion witnesses. Cucumber's `fail_on_skipped()` rejects undefined/pending/skipped steps and `run_and_exit` fails on assertion errors.

## Bounds and assumptions

Both models use two fixed executable identities, one retry pair, one forward pair, one group site, policy/count limits 0 through 2, and effect/runtime tokens 0 through 4. `current = 2` denotes terminal completion, not a third executable node. Quint checks 12 transitions. Alloy uses exactly 13 ordered states, hence 12 transitions, with signed 4-bit integers. All reachable arithmetic stays within that representation. Idle steps permit shorter behavior to be padded to the Alloy scope. The bound includes two accepted group retries followed by exhaustion skip and terminal execution, as witnessed by both models.

Successful checkpoint writes are atomic in the abstract machine. Acceptance plus its successful post-transition checkpoint is one abstract action. Failed persistence cannot authorize another effect. Resume denotes restoring a successfully saved authoritative state, not a crash transition that guesses unsaved state. Invalid input handling is an abstract rejection action, with actual JSON/schema/identity/topology rejection tested by Cucumber. Opaque runtime tokens represent preservation, not filesystem-content equivalence. Save/policy/failure actions deliberately allow additional interleavings beyond the normal engine loop.

There is no exactly-once guarantee across an external effect followed by a crash before persistence, no fsync/durability proof, fairness/liveness proof, arbitrary-topology proof or unbounded integer proof. Manual reposition and `usize` overflow are concrete regression assertions, not claims proved by the scalar models. These are bounded executable specifications plus production regression tests, not a formal refinement proof of all Rust behavior.

## CI and retained evidence

`.github/workflows/ci.yml` runs the provider-native `formal` job on normal relevant pull requests and pushes to `main`, using pinned Node/Rust and `scripts/check_formal.sh`. A nonzero check accumulates a nonzero aggregate status while independent checks continue. `actions/upload-artifact` runs with `if: always()`, retains `target/formal-reports/` for 14 days, and therefore includes completed logs even after a model/test failure. Cancellation or failures before any report can be created cannot manufacture reports.

Each runner invocation allocates a fresh `target/formal-reports/run.*` directory. It retains exact command lines, versions, CLI help, individual stdout/stderr logs, status receipts, Quint test/simulation traces, Alloy `receipt.json` and SAT witness JSON. A counterexample ITF is retained when the verifier emits one. Its absence on successful verification is not evidence of a missing failure log. The nested verifier's exact process argv and isolated endpoint are in `quint-verify.log`. Alloy expectations are checked against all named commands: eight assertion checks require UNSAT (`expect 0`), and four witnesses require SAT (`expect 1`), with missing/duplicate/extra command results rejected.

See `VERIFICATION.md` for the dated local execution evidence and any external constraints. No remote CI execution is implied by local checks.
