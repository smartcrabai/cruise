import { MarkdownViewer } from "./MarkdownViewer";
import { Spinner } from "./Spinner";

export interface WorkflowPlanPanelProps {
  panelPlanId: string;
  tabPlanId: string;
  askResponse: string;
  planLoading: boolean;
  planContent: string;
  className?: string;
}

export function WorkflowPlanPanel({
  panelPlanId,
  tabPlanId,
  askResponse,
  planLoading,
  planContent,
  className,
}: WorkflowPlanPanelProps) {
  return (
    <div
      role="tabpanel"
      id={panelPlanId}
      aria-labelledby={tabPlanId}
      className={className}
    >
      {askResponse && (
        <div className="mb-4 p-3 bg-blue-100/30 dark:bg-blue-900/30 border border-blue-300 dark:border-blue-700 rounded">
          <p className="text-xs text-blue-600 dark:text-blue-400 font-semibold mb-1">Answer</p>
          <p className="text-sm text-gray-700 dark:text-gray-300">{askResponse}</p>
        </div>
      )}

      {planLoading ? (
        <div className="flex items-center gap-2 text-sm text-gray-500 dark:text-gray-400">
          <Spinner />
          <span>Loading plan...</span>
        </div>
      ) : planContent ? (
        <MarkdownViewer content={planContent} />
      ) : (
        <p className="text-sm text-gray-500 dark:text-gray-400">No plan available.</p>
      )}
    </div>
  );
}
