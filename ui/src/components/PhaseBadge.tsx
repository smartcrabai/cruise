import type { SessionPhase } from "../types";

/** Display label shown when phase is "Awaiting Approval" but plan isn't ready yet. */
export const PLANNING_LABEL = "Planning";

const PHASE_COLORS: Record<SessionPhase, string> = {
  "Awaiting Approval": "bg-yellow-900/50 text-yellow-300",
  Planned: "bg-blue-900/50 text-blue-300",
  Running: "bg-green-900/50 text-green-300",
  Completed: "bg-gray-700/50 text-gray-300",
  Failed: "bg-red-900/50 text-red-300",
  Suspended: "bg-orange-900/50 text-orange-300",
};

export function PhaseBadge({
  phase,
  planAvailable,
}: {
  phase: SessionPhase;
  planAvailable?: boolean;
}) {
  const cls = PHASE_COLORS[phase];
  const isAwaiting = phase === "Awaiting Approval";
  const showApproveReady = isAwaiting && planAvailable === true;
  const displayLabel = isAwaiting && planAvailable !== true ? PLANNING_LABEL : phase;
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
