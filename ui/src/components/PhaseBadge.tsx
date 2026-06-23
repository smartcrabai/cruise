import type { SessionPhase } from "../types";

/** Display label shown when a Draft session has plan generation currently in-flight. */
export const PLANNING_LABEL = "Planning";

/** Display label shown when a plan fix/regeneration is currently in progress. */
export const FIXING_LABEL = "Fixing";

const PHASE_COLORS: Record<SessionPhase, string> = {
  Draft: "bg-gray-100/50 dark:bg-gray-800/50 text-gray-600 dark:text-gray-400",
  "Awaiting Approval": "bg-yellow-100/50 dark:bg-yellow-900/50 text-yellow-700 dark:text-yellow-300",
  "Awaiting Input": "bg-amber-100/50 dark:bg-amber-900/50 text-amber-700 dark:text-amber-300",
  Planned: "bg-blue-100/50 dark:bg-blue-900/50 text-blue-700 dark:text-blue-300",
  Running: "bg-green-100/50 dark:bg-green-900/50 text-green-700 dark:text-green-300",
  Completed: "bg-gray-200/50 dark:bg-gray-700/50 text-gray-800 dark:text-gray-300",
  Failed: "bg-red-100/50 dark:bg-red-900/50 text-red-700 dark:text-red-300",
  Suspended: "bg-orange-100/50 dark:bg-orange-900/50 text-orange-700 dark:text-orange-300",
};

export function PhaseBadge({
  phase,
  planAvailable,
  fixing,
}: {
  phase: SessionPhase;
  planAvailable?: boolean;
  /** When true, overrides the label: "Planning" for Draft sessions, "Fixing" for Awaiting Approval sessions. */
  fixing?: boolean;
}) {
  const cls = PHASE_COLORS[phase];
  const isAwaiting = phase === "Awaiting Approval";
  const isDraftPlanning = phase === "Draft" && !!fixing;
  const showApproveReady = isAwaiting && planAvailable === true && !fixing;
  const displayLabel = isDraftPlanning
    ? PLANNING_LABEL
    : isAwaiting && fixing
      ? FIXING_LABEL
      : phase;
  return (
    <span className={`inline-flex items-center gap-1 px-2 py-0.5 rounded text-xs font-medium ${cls}`}>
      {showApproveReady && (
        <span
          role="img"
          aria-label="plan ready for approval"
          className="w-2 h-2 rounded-full bg-blue-400 flex-shrink-0"
        />
      )}
      {displayLabel}
    </span>
  );
}
