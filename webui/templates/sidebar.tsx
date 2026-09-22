export default function Sidebar(props) {
  return (
    <div class="h-full flex flex-col">
      <div class="px-4 py-3 border-b border-gray-200 dark:border-gray-800 space-y-1.5">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h2 class="text-sm font-semibold text-gray-800 dark:text-gray-200">Sessions</h2>
          <div class="flex items-center gap-1">
            <button type="button" hx-post="/webui/clean" hx-swap="none" hx-confirm="Remove sessions with closed or merged PRs?" class="whitespace-nowrap px-1.5 py-1 text-xs text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-800 rounded" title="Clean completed sessions">Clean</button>
            {props.runAllActive ? <button type="button" hx-get="/run-all" hx-target="#main" hx-push-url="true" class="whitespace-nowrap px-1.5 py-1 text-xs rounded bg-blue-600 text-white hover:bg-blue-700" aria-label="View running sessions" title="View running sessions">Run All</button> : <>{props.runnableCount < 1 && <button type="button" hx-post="/webui/run-all" hx-target="#main" hx-push-url="true" hx-confirm={props.runAllConfirm} class="whitespace-nowrap px-1.5 py-1 text-xs rounded text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-800 disabled:opacity-50" aria-label="Run all pending sessions" title="Run all pending sessions" disabled>Run All</button>}{props.runnableCount > 0 && <button type="button" hx-post="/webui/run-all" hx-target="#main" hx-push-url="true" hx-confirm={props.runAllConfirm} class="whitespace-nowrap px-1.5 py-1 text-xs rounded text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-800 disabled:opacity-50" aria-label="Run all pending sessions" title="Run all pending sessions">Run All</button>}</>}
            <button type="button" hx-get="/new" hx-target="#main" hx-push-url="true" class="whitespace-nowrap px-1.5 py-1 text-xs bg-blue-600 text-white hover:bg-blue-700 rounded">+ New</button>
            <button type="button" hx-get="/webui/settings" hx-target="#dialogs" hx-swap="innerHTML" aria-label="Settings" title="Settings" class="whitespace-nowrap px-1.5 py-1 text-xs text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-800 rounded">⚙</button>
          </div>
        </div>
        <p id="clean-message" class="text-xs text-gray-500 dark:text-gray-400"></p>
      </div>
      <div id="session-list" class="flex-1 overflow-y-auto" hx-get="/webui/sidebar" hx-trigger="every 3s" hx-swap="innerMorph">{props.rows}</div>
      <div class="flex-shrink-0 border-t border-gray-200 dark:border-gray-800 px-4 py-2">
        <div class="text-xs text-gray-500 dark:text-gray-400">v{props.version}</div>
      </div>
    </div>
  );
}
