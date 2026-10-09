export default function RunAllControl(props) {
  return (
    <span id="run-all-control" class="flex items-center">
      {props.runAllActive ? <button type="button" hx-get="/run-all" hx-target="#main" hx-push-url="true" class="whitespace-nowrap px-1.5 py-1 text-xs rounded bg-blue-600 text-white hover:bg-blue-700" aria-label="View running sessions" title="View running sessions">Run All</button> : <>{props.runnableCount < 1 && <button type="button" hx-post="/webui/run-all" hx-target="#main" hx-push-url="true" hx-confirm={props.runAllConfirm} class="whitespace-nowrap px-1.5 py-1 text-xs rounded text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-800 disabled:opacity-50" aria-label="Run all pending sessions" title="Run all pending sessions" disabled>Run All</button>}{props.runnableCount > 0 && <button type="button" hx-post="/webui/run-all" hx-target="#main" hx-push-url="true" hx-confirm={props.runAllConfirm} class="whitespace-nowrap px-1.5 py-1 text-xs rounded text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-800 disabled:opacity-50" aria-label="Run all pending sessions" title="Run all pending sessions">Run All</button>}</>}
    </span>
  );
}
