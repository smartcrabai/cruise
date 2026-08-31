import type { Session } from "../types";

export type RunStatus = "idle" | "running" | "completed" | "failed" | "cancelled";

/** True when the session is in review with a usable plan and no durable planning error. */
export function isApprovalReady(session: Session): boolean {
  return session.phase === "Awaiting Approval" && session.planAvailable === true && !session.planError && !session.fixInProgress;
}

/** Which action buttons are visible in the session detail pane. */
export interface SessionActions {
  /** Show the Approve button (`phase === "Awaiting Approval" && planAvailable`). */
  showApprove: boolean;
  /** Show the "Publish as Issue" button (`phase === "Awaiting Approval" || phase === "Planned"`, both requiring `planAvailable`). */
  showPublishIssue: boolean;
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
  /**
   * Show the "Discard" button (`phase === "Awaiting Approval"`), which routes
   * through the lighter `discard_session` backend command instead of `delete_session`
   * since a pre-run session has no local git worktree to clean up.
   */
  showDiscard: boolean;
  /** Show the "Delete" button (`phase !== "Running"`, excluding "Awaiting Approval" -- see `showDiscard`). */
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
export function getSessionActions(session: Session, status: RunStatus, isFixing?: boolean, hasPendingPrompt?: boolean): SessionActions {
  const { phase } = session;

  const isLocallyRunning = status === "running";
  const isPhaseRunning = phase === "Running";

  // Local execution finished but refreshSession() hasn't updated session.phase yet.
  const isAwaitingRefresh =
    !isLocallyRunning && status !== "idle" && isPhaseRunning;

  // Hydrated or local planning claims remain cancellable after navigation.
  const isActiveRun = isLocallyRunning || !!hasPendingPrompt || !!session.fixInProgress || !!isFixing || (isPhaseRunning && !isAwaitingRefresh);

  const showCancel = isActiveRun;

  const hasPlanError = !!session.planError;
  const awaitingApprovalWithPlan =
    phase === "Awaiting Approval" &&
    !isLocallyRunning &&
    !isFixing &&
    !session.fixInProgress &&
    session.planAvailable === true;
  const usablePlan = session.planAvailable === true && !hasPlanError;

  const showApprove = awaitingApprovalWithPlan && !hasPlanError;
  const showPublishIssue = (phase === "Awaiting Approval" || phase === "Planned") && usablePlan && !isLocallyRunning && !isActiveRun;
  const showFix = awaitingApprovalWithPlan;
  const showAsk = awaitingApprovalWithPlan && !hasPlanError;

  const showCreateWorktree = !isActiveRun && phase === "Planned" && usablePlan;
  const showRun =
    !isActiveRun &&
    !isAwaitingRefresh &&
    usablePlan &&
    (phase === "Suspended" || phase === "Failed");

  const runLabel =
    phase === "Failed" ? "Retry" : "Resume";

  const showReset =
    !isActiveRun &&
    !isAwaitingRefresh &&
    (phase === "Suspended" ||
    phase === "Failed" ||
    phase === "Completed");

  const showReplan = !isActiveRun && phase === "Planned";

  const showDiscard = !isLocallyRunning && phase === "Awaiting Approval";

  const showDelete = !isLocallyRunning && phase !== "Running" && phase !== "Awaiting Approval";

  const showGeneratePlan =
    !isLocallyRunning &&
    !isFixing &&
    !session.fixInProgress &&
    (phase === "Draft" || phase === "Awaiting Input" || (phase === "Awaiting Approval" && hasPlanError));

  return {
    showApprove,
    showPublishIssue,
    showFix,
    showAsk,
    showCreateWorktree,
    showRun,
    runLabel,
    showReset,
    showReplan,
    showDiscard,
    showDelete,
    showCancel,
    showGeneratePlan,
  };
}
