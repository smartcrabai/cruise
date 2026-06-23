import { openUrl } from "@tauri-apps/plugin-opener";
import type { Session } from "../types";

export interface WorkflowInfoPanelProps {
  session: Session;
  panelInfoId: string;
  tabInfoId: string;
  className?: string;
}

export function WorkflowInfoPanel({ session, panelInfoId, tabInfoId, className }: WorkflowInfoPanelProps) {
  return (
    <div
      role="tabpanel"
      id={panelInfoId}
      aria-labelledby={tabInfoId}
      className={className}
    >
      <dl className="space-y-2 text-sm">
        <div>
          <dt className="text-gray-500 dark:text-gray-400">Config</dt>
          <dd className="text-gray-800 dark:text-gray-200">{session.configSource}</dd>
        </div>
        <div>
          <dt className="text-gray-500 dark:text-gray-400">Base Directory</dt>
          <dd className="text-gray-800 dark:text-gray-200">{session.baseDir}</dd>
        </div>
        {session.worktreeBranch && (
          <div>
            <dt className="text-gray-500 dark:text-gray-400">Branch</dt>
            <dd className="text-gray-800 dark:text-gray-200">{session.worktreeBranch}</dd>
          </div>
        )}
        {session.prUrl && (
          <div>
            <dt className="text-gray-500 dark:text-gray-400">Pull Request</dt>
            <dd>
              {/^https?:\/\//i.test(session.prUrl) ? (
                <a
                  href={session.prUrl}
                  onClick={(e) => { e.preventDefault(); void openUrl(session.prUrl!); }}
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
            <dt className="text-gray-500 dark:text-gray-400">Error</dt>
            <dd className="text-red-600 dark:text-red-400">{session.phaseError}</dd>
          </div>
        )}
      </dl>
    </div>
  );
}
