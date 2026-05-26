import type { Session } from "../types";

export type RunStatus = "idle" | "running" | "completed" | "failed" | "cancelled";

/** True when the session is in "Awaiting Approval" phase with a plan ready for review. */
export function isApprovalReady(session: Session): boolean {
  return session.phase === "Awaiting Approval" && session.planAvailable === true && !session.fixInProgress;
}

/** Which action buttons are visible in the session detail pane. */
export interface SessionActions {
  /** Show the Approve button (`phase === "Awaiting Approval" && planAvailable`). */
  showApprove: boolean;
  /** Show the "Fix" button (`phase === "Awaiting Approval" && planAvailable`). */
  showFix: boolean;
  /** Show the "Ask" button (`phase === "Awaiting Approval" && planAvailable`). */
  showAsk: boolean;
  /** Show the "Create worktree (new branch)" button (`phase === "Planned"` only). */
  showCreateWorktree: boolean;
  /** Show the Resume / Retry run button. */
  showRun: boolean;
  /** Label for the run button: "Resume" (Suspended) or "Retry" (Failed). */
  runLabel: string;
  /** Show the "Reset to Planned" button. */
  showReset: boolean;
  /** Show the "Replan" button (`phase === "Planned"` only). */
  showReplan: boolean;
  /** Show the "Delete" button (`phase !== "Running"`). */
  showDelete: boolean;
  /** Show the "Cancel" button (while the session is actively running, locally or per backend phase). */
  showCancel: boolean;
  /** Show the "Generate Plan" button (`phase === "Draft"` only). */
  showGeneratePlan: boolean;
}

/**
 * Derive which action buttons to show in the session detail pane.
 *
 * For Awaiting Approval sessions, follows the approve-plan review loop
 * (`src/plan_cmd.rs:218-295`) rather than the CLI list phase-action matrix.
 *
 * @param session  - The current session DTO (always reflects latest persisted state).
 * @param status   - Whether the local process is actively running this session.
 * @param isFixing - When true, suppresses Approve/Fix/Ask while a plan fix is in progress.
 */
export function getSessionActions(session: Session, status: RunStatus, isFixing?: boolean): SessionActions {
  const { phase } = session;

  const isLocallyRunning = status === "running";
  const isPhaseRunning = phase === "Running";

  // Local execution finished but refreshSession() hasn't updated session.phase yet.
  const isAwaitingRefresh =
    !isLocallyRunning && status !== "idle" && isPhaseRunning;

  // "Actively running" = local run in progress OR backend reports Running (excluding refresh-wait transient state).
  const isActiveRun = isLocallyRunning || (isPhaseRunning && !isAwaitingRefresh);

  const showCancel = isActiveRun;

  const awaitingApprovalWithPlan =
    !isLocallyRunning && !isFixing && isApprovalReady(session);

  const showApprove = awaitingApprovalWithPlan;
  const showFix = awaitingApprovalWithPlan;
  const showAsk = awaitingApprovalWithPlan;

  const showCreateWorktree = !isLocallyRunning && phase === "Planned";

  const showRun =
    !isActiveRun &&
    !isAwaitingRefresh &&
    (phase === "Suspended" ||
    phase === "Failed");

  const runLabel =
    phase === "Failed" ? "Retry" : "Resume";

  const showReset =
    !isActiveRun &&
    !isAwaitingRefresh &&
    (phase === "Suspended" ||
    phase === "Failed" ||
    phase === "Completed");

  const showReplan = !isLocallyRunning && phase === "Planned";

  const showDelete = !isLocallyRunning && phase !== "Running";

  const showGeneratePlan = !isLocallyRunning && !isFixing && !session.fixInProgress && phase === "Draft";

  return {
    showApprove,
    showFix,
    showAsk,
    showCreateWorktree,
    showRun,
    runLabel,
    showReset,
    showReplan,
    showDelete,
    showCancel,
    showGeneratePlan,
  };
}
