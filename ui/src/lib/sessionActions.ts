import type { Session } from "../types";

export type RunStatus = "idle" | "running";

/** Which action buttons are visible in the session detail pane. */
export interface SessionActions {
  /** Show the Approve button (`phase === "Awaiting Approval" && planAvailable`). */
  showApprove: boolean;
  /** Show the "Create worktree (new branch)" button (`phase === "Planned"` only). */
  showCreateWorktree: boolean;
  /** Show the Resume / Retry run button. */
  showRun: boolean;
  /** Label for the run button: "Resume" (Running/Suspended) or "Retry" (Failed). */
  runLabel: string;
  /** Show the "Reset to Planned" button. */
  showReset: boolean;
  /** Show the "Replan" button (`phase === "Planned"` only). */
  showReplan: boolean;
  /** Show the "Delete" button (`phase !== "Running"`). */
  showDelete: boolean;
  /** Show the "Cancel" button (only while the local process is running). */
  showCancel: boolean;
}

/**
 * Derive which action buttons to show in the session detail pane.
 *
 * Follows the same phase-action matrix as the CLI (`src/list_cmd.rs:135-167`).
 *
 * @param session - The current session DTO (always reflects latest persisted state).
 * @param status  - Whether the local process is actively running this session.
 */
export function getSessionActions(session: Session, status: RunStatus): SessionActions {
  const { phase } = session;

  const isRunning = status === "running";
  const showCancel = isRunning;

  const showApprove =
    !isRunning && phase === "Awaiting Approval" && session.planAvailable === true;

  const showCreateWorktree = !isRunning && phase === "Planned";

  const showRun =
    !isRunning &&
    (phase === "Running" ||
    phase === "Suspended" ||
    phase === "Failed");

  const runLabel =
    phase === "Failed" ? "Retry" : "Resume";

  const showReset =
    !isRunning &&
    (phase === "Running" ||
    phase === "Suspended" ||
    phase === "Failed" ||
    phase === "Completed");

  const showReplan = !isRunning && phase === "Planned";

  const showDelete = !isRunning && phase !== "Running";

  return {
    showApprove,
    showCreateWorktree,
    showRun,
    runLabel,
    showReset,
    showReplan,
    showDelete,
    showCancel,
  };
}
