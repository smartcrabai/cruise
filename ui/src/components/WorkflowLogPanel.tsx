import type { RefObject } from "react";
import type { RunStatus } from "../lib/sessionActions";

export interface WorkflowLogPanelProps {
  panelLogId: string;
  tabLogId: string;
  status: RunStatus;
  logContent: string;
  logEndRef: RefObject<HTMLSpanElement | null>;
  preRef: RefObject<HTMLPreElement | null>;
  onScroll: () => void;
  className?: string;
}

export function WorkflowLogPanel({
  panelLogId,
  tabLogId,
  status,
  logContent,
  logEndRef,
  preRef,
  onScroll,
  className,
}: WorkflowLogPanelProps) {
  return (
    <div
      role="tabpanel"
      id={panelLogId}
      aria-labelledby={tabLogId}
      className={className}
      onScroll={onScroll}
    >
      {logContent ? (
        <pre ref={preRef} className="text-xs text-gray-300 whitespace-pre-wrap break-all">
          {logContent}
          <span ref={logEndRef} />
        </pre>
      ) : (
        <p className="text-sm text-gray-500">
          {status === "idle"
            ? "Run the session to see logs here."
            : status === "cancelled"
              ? "Session was cancelled."
              : "No log entries yet."}
        </p>
      )}
    </div>
  );
}
