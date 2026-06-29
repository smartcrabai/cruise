# Incomplete Handoff: GUI DAG Visualization Feature

## Status

Started implementation based on the plan via `/implement-after-tests`. Completed the backend implementation of `build_dag_dto`, the Tauri handler registration fix, frontend types, the command wrapper, the `mermaid` dependency, and the `WorkflowDagPanel` component creation. Tab integration into `App.tsx` and test additions have not been started yet. Handing off here due to tool iteration limits.

## Done (this session)

- Read the plan `/Users/takumi/.local/share/cruise/sessions/20260629065146988_fb6aecb7b2894d64b99b11d310e16eb5/plan.md` and confirmed the implementation approach
- Investigated related code
  - `src/dag.rs` (`ExecutionDag`, `DagNode`, `NodeSuccessor`, `TransitionReason`)
  - `src/workflow.rs` (`CompiledWorkflow`, `compile`)
  - `src-tauri/src/commands.rs` (`get_session_plan`, `SessionDto`)
  - `src-tauri/src/lib.rs` (`invoke_handler`)
  - `ui/src/App.tsx` (`ActiveTab`, `WorkflowRunner`, tab buttons / panels)
  - `ui/src/types.ts`, `ui/src/lib/commands.ts`, `ui/src/components/WorkflowPlanPanel.tsx`, and related tests
- `src-tauri/src/commands.rs`
  - Changed `build_dag_dto` from a stub to a real implementation
    - Aggregated `ExecutionDag.nodes` by step name
    - Deduped edges with identical `(from_step, to_step, reason, selector)`
    - Determined each step's kind from `CompiledWorkflow.steps` (prompt / command / option / unknown)
    - `is_terminal` is determined by whether an edge with `target == None` exists
    - `current_step` is resolved to a step name considering `current_step_is_node_id`
  - Added `step_kind` and `transition_reason` helpers
- `src-tauri/src/lib.rs`
  - Removed the duplicate `commands::get_session_plan` in `invoke_handler!` (`get_session_dag` is already registered)
- Confirmed the backend compiles with `cargo check -p cruise-gui`
- Frontend types / API
  - Added `DagDto`, `DagStepDto`, `DagEdgeDto` to `ui/src/types.ts`
  - Added `getSessionDag(sessionId)` to `ui/src/lib/commands.ts`
- Added `mermaid` dependency to `ui/package.json` (v11.16.0)
- Created `ui/src/components/WorkflowDagPanel.tsx`
  - Calls `getSessionDag` to retrieve DAG data
  - `mermaid` is loaded via dynamic import (`await import("mermaid")`)
  - `buildMermaidSource` helper generates Mermaid syntax
    - Sanitizes step names into Mermaid IDs (sequential prefix + non-alphanumeric replacement)
    - Terminal edges point to a shared `end[/END/]` node
    - `currentStep` is highlighted with blue style
    - `startStep` is highlighted with green style
  - State management for loading / error / SVG display
  - Discards stale rendering on session switch (`renderId` counter)
- Committed the work
  - commit: `b27aa59`
  - message: `WIP: implement DAG backend and start frontend panel`

## Remaining

1. **Tab integration into App.tsx**
   - Add `"dag"` to `ActiveTab` (`ui/src/App.tsx:465`)
   - Derive `tabDagId` / `panelDagId` from `useId` within `WorkflowRunner`
   - Place the tab button (`role="tab"`) between Info and Plan
     - `id={tabDagId}`, `aria-selected={activeTab === "dag"}`, `aria-controls={panelDagId}`, `onClick={() => onActiveTabChange("dag")}`
   - Add `activeTab === "dag" && <WorkflowDagPanel sessionId={session.id} panelId={panelDagId} tabId={tabDagId} />` to the panel switch
   - Add the `WorkflowDagPanel` import at the top of `ui/src/App.tsx`

2. **Add / fix tests**
   - Create `ui/src/components/WorkflowDagPanel.test.tsx`
     - loading display
     - `getSessionDag` call
     - Mermaid SVG rendering (mock `mermaid`)
     - error display
     - re-fetch on session switch
     - verify `buildMermaidSource` output (node id sanitize, terminal edge, current step highlight)
   - `ui/src/__tests__/WorkflowRunner.test.tsx`
     - Add `getSessionDag` to the mock
     - DAG tab is displayed
     - `WorkflowDagPanel` mounts and `getSessionDag` is called when the DAG tab is selected
   - Fix existing assertions in `ui/src/test/App.*.test.tsx` that assume tab count or order

3. **Verification / build**
   - `cargo test -p cruise-gui` (or `cargo test --package cruise-gui`)
   - `pnpm --dir ui test`
   - `pnpm --dir ui lint`
   - `pnpm --dir ui build`
   - `cargo clippy -p cruise-gui --all-targets`

4. **Manual verification and PR creation**
   - Launch the GUI with `pnpm --dir ui dev` / `cargo tauri dev`
   - Open the DAG tab for an existing session and confirm rendering
   - Confirm edge rendering for a branching workflow
   - Take a screenshot and attach it to the PR body
   - Run `/review-all` before creating the PR

## Next-Agent Starting Position

- Branch: `cruise/20260629065146988_fb6aecb7b2894d64b99b11d310e16eb5-GUI-DAG-PR-DAG`
- Changes up to commit `b27aa59` are committed
- Next, start with expanding `ActiveTab` in `ui/src/App.tsx` and integrating the DAG tab button / panel
- After integration, add and run tests, then pass lint / build / clippy
- Note the open items in the plan `/Users/takumi/.local/share/cruise/sessions/20260629065146988_fb6aecb7b2894d64b99b11d310e16eb5/plan.md` (whether step-level display is sufficient, tab placement)
