import { openUrl } from "@tauri-apps/plugin-opener";
import { formatLocalTime } from "../lib/format";
import type { Session } from "../types";

export interface WorkflowInfoPanelProps {
  session: Session;
  panelInfoId: string;
  tabInfoId: string;
  className?: string;
}

export function WorkflowInfoPanel({ session, panelInfoId, tabInfoId, className = "" }: WorkflowInfoPanelProps) {
  return (
    <div id={panelInfoId} role="tabpanel" aria-labelledby={tabInfoId} className={`p-6 space-y-3 text-sm text-gray-500 dark:text-gray-400 ${className}`}>
      <dl className="space-y-3">
        <div>
          <dt className="text-xs uppercase tracking-wide">Config</dt>
          <dd className="font-mono text-gray-700 dark:text-gray-300 mt-0.5">{session.configSource}</dd>
        </div>
        <div>
          <dt className="text-xs uppercase tracking-wide">{session.repo ? "Repository" : "Base dir"}</dt>
          <dd className="font-mono text-gray-700 dark:text-gray-300 mt-0.5">{session.repo ?? session.baseDir}</dd>
        </div>
        {session.worktreeBranch && (
          <div>
            <dt className="text-xs uppercase tracking-wide">Branch</dt>
            <dd className="font-mono text-gray-700 dark:text-gray-300 mt-0.5">{session.worktreeBranch}</dd>
          </div>
        )}
        <div>
          <dt className="text-xs uppercase tracking-wide">Created</dt>
          <dd className="text-gray-700 dark:text-gray-300 mt-0.5">{formatLocalTime(session.createdAt)}</dd>
        </div>
        {session.completedAt && (
          <div>
            <dt className="text-xs uppercase tracking-wide">Completed</dt>
            <dd className="text-gray-700 dark:text-gray-300 mt-0.5">{formatLocalTime(session.completedAt)}</dd>
          </div>
        )}
        {session.prUrl && (
          <div>
            <dt className="text-xs uppercase tracking-wide">Pull Request</dt>
            <dd>
              {/^https?:\/\//i.test(session.prUrl) ? (
                <a
                  href={session.prUrl}
                  onClick={(event) => { event.preventDefault(); void openUrl(session.prUrl!); }}
                  className="text-blue-600 dark:text-blue-400 hover:text-blue-500 dark:hover:text-blue-300"
                >
                  {session.prUrl}
                </a>
              ) : (
                <span className="text-gray-800 dark:text-gray-200">{session.prUrl}</span>
              )}
            </dd>
          </div>
        )}
        {session.phaseError && (
          <div>
            <dt className="text-xs uppercase tracking-wide">Error</dt>
            <dd className="text-red-600 dark:text-red-400 mt-0.5 font-mono text-xs">{session.phaseError}</dd>
          </div>
        )}
        {session.planError && session.planError !== session.phaseError && (
          <div>
            <dt className="text-xs uppercase tracking-wide">Planning Error</dt>
            <dd className="text-red-600 dark:text-red-400 mt-0.5 font-mono text-xs">{session.planError}</dd>
          </div>
        )}
      </dl>
    </div>
  );
}
