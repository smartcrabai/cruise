# Incomplete Handoff: GUI-Planning-Regenerating-plan Tests

## What's Done
- Analyzed the full codebase structure for the cruise project
- Read and understood `src/plan_cmd.rs` - the `regenerate_plan_for_session` function (lines 1317-1393)
- Reviewed existing test patterns in both Rust (`src/plan_cmd.rs` tests) and TypeScript (`ui/src/components/SessionConfigEditor.test.tsx`)
- Identified the phase transition logic documented in code comments:
  - Draft | AwaitingInput -> AwaitingApproval
  - AwaitingApproval -> AwaitingApproval (no-op)
  - Planned -> Planned (preserve approval; do NOT silently un-approve)
- Found existing phase-gate tests (lines 1754-1872) that verify error cases (Running, Completed, Suspended, Failed)

## What Remains
Write test code for `regenerate_plan_for_session` success paths:

1. **Draft -> AwaitingApproval**: Session in Draft phase should transition to AwaitingApproval after successful regeneration
2. **AwaitingInput -> AwaitingApproval**: Session in AwaitingInput phase should transition to AwaitingApproval
3. **AwaitingApproval -> AwaitingApproval**: Session should stay in AwaitingApproval (no-op transition)
4. **Planned -> Planned**: Session should stay in Planned (preserve approval, do NOT un-approve)
5. **plan_error cleared**: After successful regeneration, `plan_error` should be None
6. **title refreshed**: After successful regeneration, session title should be updated from plan content

## Next-Agent Starting Position
- File: `src/plan_cmd.rs` - add tests in the `#[cfg(test)] mod tests` section after line 1872
- Use existing test helpers: `lock_process()`, `make_session()`, `init_git_repo()`, `TempDir`
- Follow Given-When-Then structure as shown in existing tests
- Tests require `#[cfg(unix)]` and `#[tokio::test]` attributes
- Each test needs a git repo with cruise.yaml config file
- Mock/echo commands can be used: `command: [echo]` or `command: ["echo", "test output"]`

## Key Code References
- `regenerate_plan_for_session` function: `src/plan_cmd.rs:1317-1393`
- Phase transition logic: `src/plan_cmd.rs:1387-1390`
- Existing phase-gate tests: `src/plan_cmd.rs:1754-1872`
- Test support module: `src/test_support.rs` (provides `lock_process`, `make_session`, `init_git_repo`)
