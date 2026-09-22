# Execution graph implementation and contract evidence

The graph is fixed topology, not an unfolded history. `graph::ExecutionGraph`
contains one stable node per compiled step and one authoritative `ExecutionState`.
The `dag` module and `dag.json` filename remain public compatibility entry points.
There is no state-expansion algorithm or node-budget fallback.

## Requirement map

| Plan section | Production path | Acceptance evidence |
|---|---|---|
| 3.1, 9.1: stable topology, large loops | `graph::build_graph`, indexed sequential lookup in `engine` | `dag::tests::test_dag_topology_is_independent_of_retry_ceiling`, `graph_state::large_chain_and_scc_use_fixed_topology` (10,000 nodes, ceilings 1, 3, 1,000,000) |
| 3.2–3.3, 9.2: accepted pair counts, rejected requests, skip exceptions, overflow | `engine::accept_transition`, `graph::Transition`, `EdgeCounter` | engine retry/option/skip/group regressions, `graph_state::skipped_transitions_are_recorded_without_consuming_budget_or_wrapping` |
| 4.1–4.2, 9.3: state, snapshots, atomic checkpoints and save failure | `graph::persistence`, `engine::execute_steps_with_graph`, CLI and application checkpoint callbacks | `graph_state::repeated_resume_never_replenishes_accepted_retry_budget`, `failed_checkpoint_prevents_the_next_side_effect`, existing prev-variable and parallel-result checkpoint tests |
| 4.3: reload, resume and explicit restart | `ExecutionGraph::restore`, `prepare_resume`, engine reload, `SessionState::reset_to_planned`, session settings, session save archival | repeated-resume test includes a model reload and lowered/raised ceilings, group snapshot resume test, `explicit_restart_archives_even_a_corrupt_checkpoint` |
| 4.4: legacy and invalid files | separate `LegacyGraph` decoder, version checks, non-destructive `application::session_dag` | `checkpoint_outvotes_stale_session_position_and_preserves_invalid_files`, CLI missing-checkpoint rejection |
| 5, 9.4: conservative normal-exit analysis | iterative reachability and Kosaraju in `graph::validation` | static/dynamic/user skips, actual entry, unreachable cycles, disabled file-change branches, optional dangerous branches, large SCC tests |
| 5.3, 6.2: real entry points | pre-workspace validation in run, exec, shared application setup, reload and after-pr | public CLI normal/dry-run tests, after-pr rejection before main side effects, existing application and after-pr tests |
| 6.1, 6.3: display and phase boundaries | WebUI DTO, Graph panel, TUI, browser fixture, separate after-pr graph | DTO serialization test, WebUI regression tests, headless real-Mermaid rendering of a back edge, self-loop, terminal edge and traversal labels |
| 7, 9.2: retained contracts | existing config/group budget resolution, workflow expansion, parallel parent execution, backend APIs | existing config, engine, parallel, CLI and shared-application regression suites |

## Test corrections and their higher-authority grounds

- Plan 3.1 and 9.5 explicitly replace generated `n0000` identities and
  per-iteration retry nodes with stable step nodes. Tests now check fixed node
  cardinality and actual self-edges. A budget-exhausted edge cannot both remain
  a real edge and become a successful terminal edge.
  The parallel CLI interruption test now requires its stable parent ID `checks`
  instead of an `n` prefix, retaining the child-drain and suspension assertions.
- Plan 3.3 requires declaration-order skip paths, including terminal skips.
  Topology assertions include the skip reason even when it shares the normal
  destination. Counts are still keyed by the pair, not by the reason.
- Plan 3.2 preserves the existing budget exceptions only for skips. The public
  flow-control reference counted every ordinary `from → to` transition. The
  staged zero-ceiling test incorrectly exempted sequential edges. Its corrected
  assertion requires `LoopProtection` for the first budgeted transition with
  zero accepted counts, rather than successful execution of both steps.
- Plan 4.1 and 9.5 make the shared checkpoint authoritative. Runtime tests read
  `state.runtime` at the next-step checkpoint, not a retained copy on every
  node. Parallel result, success, stderr and child-join assertions are retained.
- Plan 4.2 requires a final checkpoint. Callback tests separate completed
  checkpoints from pre-step checkpoints instead of treating every save as a new
  step execution. Last-step display behavior remains unchanged.
- Plan 4.4 prohibits guessed continuation without counters. The missing-file
  test now checks rejection and absence of subsequent side effects, rather than
  silent fallback.
- Plan 5.3 and 9.5 remove mixed-cycle rejection from parsing. Conditional-cycle
  tests now check acceptance. Run/exec protection tests use genuinely exitless
  cycles and retain pre-side-effect rejection assertions.
- Plan 6.1 explicitly changes the visible DAG label to Graph, so the tab/status
  label assertions follow that requirement. The staged Mermaid cycle test used
  a lower-camel-case DTO reason but required a capitalized label, contrary to the
  existing renderer's observable label preservation. Its smallest correction
  checks the back-edge endpoints and nonempty label without freezing wording.
- The new command-step persistence fixture checks `prev.stderr` and success,
  consistent with the established `run_command_step` contract that clears
  `prev.output` and `prev.input`.

## Compatibility and scope

The plan's initial decision table recorded unanswered questions. This
implementation uses its recommended conservative policies: keep the existing
finite ceiling and default 3, persist new-format counters across resume, reject
provably exitless execution before side effects, and require explicit restart
rather than guessing legacy counts. This is not a claim that the earlier
confirmation attempts obtained answers. There is no unlimited-mode setting,
backend/authentication change, per-iteration history or independently resumable
after-pr phase.

## Final validation

- `cargo fmt -- --check`: passed.
- `cargo clippy --offline --locked --all-targets --all-features -- -D warnings -A clippy::large_futures`: passed.
- `RUST_TEST_THREADS=2 cargo test --offline --locked --all-features`: passed,
  covering the library, CLI unit and integration suites.
  Bounded concurrency avoids an existing five-second commit-guard fixture timeout
  under host contention without changing its assertions or deadline.
- `cargo test webui`: WebUI template, route, partial, and behavior tests passed.
- Real headless Chrome rendered the cyclic browser fixture with four SVG nodes,
  five edges, a back-edge, a self-loop, a normal terminal and accepted-count labels.
  No replacement renderer was introduced.
