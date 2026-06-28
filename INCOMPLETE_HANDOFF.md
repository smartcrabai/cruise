# Incomplete Handoff: DAG-Driven Execution Migration

## Status
Review feedback for the DAG migration has been addressed in `src/engine.rs` and `src/run_cmd.rs`. Code compiles and `cargo clippy --all-targets` is clean. The broader migration tasks (resume UX, `dag.json` persistence, tests) are still outstanding.

## Done (this session)
- `src/engine.rs`
  - Added `#[expect(clippy::too_many_lines)]` to `execute_steps_with_dag`
  - Refactored config reload to avoid cloning the whole DAG (`new_dag.clone()` removed)
  - Propagated `build_dag` errors during config reload instead of silently ignoring them with `let Ok(...)`
  - Removed duplicated `LoopState` doc comment
- `src/run_cmd.rs`
  - Split the mismatched `on_step_start` closure into:
    - `on_step_start: Fn(&str) -> Result<()>` for `ExecutionContext` (step-name logging)
    - `on_node_start: Fn(&NodeCheckpoint, &ExecutionDag) -> Result<()>` passed to `execute_steps_with_dag` (node-id persistence)
  - Added an explicit warning when the saved node id is missing from the DAG before falling back to the start node
  - Minor clippy cleanups (`ok_or_else` -> `ok_or`, `to_string()` -> `clone()`)
- Verification
  - `cargo check` passes
  - `cargo clippy --all-targets` passes
  - `cargo test --lib engine::tests` passes except one pre-existing failure (`test_next_pointing_to_nonexistent_step`) that also fails on `HEAD`

## Remaining
1. `src/run_cmd.rs`
   - Update `log_resume_message` to display step name instead of node id
   - Persist `dag.json` at the end of execution
   - Add auto-migration tests for old sessions (`has_dag=false` and only step name saved)
2. `src/session.rs`
   - Add a `dag_path()` helper for `run_cmd.rs` to save `dag.json`
3. `src/dag.rs`
   - Remove the leading `#![allow(dead_code)]` to surface unused warnings
4. Test fixes / additions
   - Add mock-based stop → resume E2E tests (linear, retry budget, `dag.json` round trip)
   - Update resume tests in `src/run_cmd.rs` to be node-id based
   - Add `test_run_migrates_legacy_session_without_dag`
5. Final verification
   - `cargo test --workspace`
   - Confirm zero references to the old function with `rg "execute_steps\\b" src`

## Next-Agent Starting Position
- Current commit: `05c9ce8` (`WIP: fix CI compilation errors - engine/worktree_pr/run_cmd DAG migration (incomplete)`)
- Branch: `cruise/20260628172537949_58fda4af44a44f249125b6fec6f3e4ff-DAG-execute-steps-Requirements`
- First run `cargo check` / `cargo clippy` to confirm the review fixes are in place
- Then implement `log_resume_message` step-name display and `dag.json` persistence (add `dag_path()` in `src/session.rs`)
- Proceed to test additions/fixes and final `cargo test --workspace`
