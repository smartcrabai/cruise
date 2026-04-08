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

export function runAllStartedLogLine(total: number, parallelism: number): string {
  return `--- Run All started (${total} sessions, parallelism: ${parallelism}) ---`;
}

export function runAllSessionStartedLogLine(sessionId: string, input: string): string {
  return `--- Session: ${input} (${sessionId}) ---`;
}

function withSessionPrefix(sessionId: string, line: string): string {
  return `[${sessionId}] ${line}`;
}

export function runAllStepLogLine(sessionId: string, step: string): string {
  return withSessionPrefix(sessionId, step);
}

export function runAllWorkflowEventLogLine(event: WorkflowTerminalEvent): string {
  return withSessionPrefix(event.data.sessionId, workflowEventLogLine(event));
}

export function runAllCompletedLogLine(cancelled: number): string {
  return `--- Run All finished (cancelled: ${cancelled}) ---`;
}

export function formatLocalTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "--";
  return date.toLocaleString(undefined, {
    dateStyle: "short",
    timeStyle: "short",
  });
}
