import type { WorkflowCompletedEvent, WorkflowFailedEvent, WorkflowCancelledEvent } from "../types";

export const PHASE_ICON = {
  Completed: "[v]",
  Failed: "[x]",
  Suspended: "||",
} as const;

type WorkflowTerminalEvent = WorkflowCompletedEvent | WorkflowFailedEvent | WorkflowCancelledEvent;

export function workflowEventLogLine(event: WorkflowTerminalEvent): string {
  if (event.event === "workflowCompleted") {
    return `${PHASE_ICON.Completed} Completed -- run: ${event.data.run}, skipped: ${event.data.skipped}, failed: ${event.data.failed}`;
  }
  if (event.event === "workflowFailed") {
    return `${PHASE_ICON.Failed} Failed: ${event.data.error}`;
  }
  return `${PHASE_ICON.Suspended} Cancelled`;
}

export function formatLocalTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "--";
  return date.toLocaleString(undefined, {
    dateStyle: "short",
    timeStyle: "short",
  });
}
