import type { SessionPhase } from "../types";

const PHASE_COLORS: Record<SessionPhase, string> = {
  "Awaiting Approval": "bg-yellow-900/50 text-yellow-300",
  Planned: "bg-blue-900/50 text-blue-300",
  Running: "bg-green-900/50 text-green-300",
  Completed: "bg-gray-700/50 text-gray-300",
  Failed: "bg-red-900/50 text-red-300",
  Suspended: "bg-orange-900/50 text-orange-300",
};

export function PhaseBadge({ phase }: { phase: SessionPhase }) {
  const cls = PHASE_COLORS[phase];
  return (
    <span className={`px-2 py-0.5 rounded text-xs font-medium ${cls}`}>
      {phase}
    </span>
  );
}
