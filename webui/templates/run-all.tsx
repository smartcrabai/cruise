export default function RunAll(props) {
  return (
    <div id="run-all-view" class="h-full flex flex-col p-6 max-w-2xl mx-auto">
      <div class="flex items-center justify-between mb-6">
        <h2 class="text-xl font-semibold text-gray-900 dark:text-gray-100">Run All</h2>
        {props.active ? <button type="button" hx-post="/webui/run-all/cancel" hx-swap="none" class="px-3 py-1.5 text-sm border border-gray-300 dark:border-gray-700 text-gray-500 dark:text-gray-400 hover:bg-gray-200 dark:hover:bg-gray-800 rounded">Cancel</button> : <a href="/" hx-get="/" hx-target="#main" hx-push-url="true" class="px-3 py-1.5 text-sm bg-blue-600 text-white hover:bg-blue-700 rounded">Done</a>}
      </div>

      <div id="run-all-progress">
        <div class="mb-4">
          <div class="flex justify-between text-xs text-gray-500 dark:text-gray-400 mb-1">
            <span>{props.finished} / {props.total} sessions</span>
            {props.status === "running" ? <span class="text-green-600 dark:text-green-400 animate-pulse">Running {props.runningCount}{props.parallelism && <> / {props.parallelism}</>}</span> : props.status === "completed" ? <span class="text-green-600 dark:text-green-400">Completed</span> : props.status === "cancelled" ? <span class="text-orange-600 dark:text-orange-400">Cancelled</span> : props.status === "error" ? <span class="text-red-600 dark:text-red-400">Error</span> : ""}
          </div>
          <div class="h-1.5 bg-gray-100 dark:bg-gray-800 rounded-full overflow-hidden">
            <div class="h-full bg-blue-600 rounded-full"></div>
          </div>
        </div>
        {!props.runningEmpty && <div class="mb-4 space-y-2">{props.running.map(session => <div class="p-3 bg-gray-50 dark:bg-gray-900 border border-green-300 dark:border-green-900/50 rounded"><div class="flex items-center gap-2 mb-1"><span class="w-2 h-2 rounded-full bg-green-400 animate-pulse"></span><span class="text-xs text-gray-500 dark:text-gray-400 font-mono">{session.id}</span></div><p class="text-sm text-gray-800 dark:text-gray-200 truncate">{session.title}</p>{session.step && <p class="text-xs text-gray-500 dark:text-gray-400 mt-1">{session.step}</p>}</div>)}</div>}
      </div>

      <pre id="run-all-log" class="mb-4 text-xs font-mono bg-white dark:bg-gray-950 text-gray-700 dark:text-gray-300 p-4 rounded overflow-auto whitespace-pre-wrap leading-relaxed max-h-80">{props.log}</pre>

      <div id="run-all-results" class="flex-1 overflow-y-auto space-y-1">
        {props.resultsEmpty && <p class="text-xs text-gray-500 dark:text-gray-400">No results yet.</p>}
        {props.results.map(result => <div class="flex items-start gap-2 px-3 py-2 rounded bg-gray-50/50 dark:bg-gray-900/50"><div class="flex-1 min-w-0"><p class="text-sm text-gray-700 dark:text-gray-300 truncate">{result.title}</p>{result.error && <p class="text-xs text-red-600 dark:text-red-400 mt-0.5 truncate">{result.error}</p>}</div><span class={result.failed ? "inline-flex px-2 py-0.5 rounded text-xs font-medium bg-red-100/50 dark:bg-red-900/50 text-red-700 dark:text-red-300" : "inline-flex px-2 py-0.5 rounded text-xs font-medium bg-gray-100/50 dark:bg-gray-800/50 text-gray-600 dark:text-gray-400"}>{result.phase}</span></div>)}
      </div>

      {!props.active && <div class="mt-4 p-3 bg-gray-50 dark:bg-gray-900 border border-gray-200 dark:border-gray-800 rounded text-sm text-gray-500 dark:text-gray-400 flex flex-col gap-1"><span>{props.finished} completed</span>{props.runError && <p class="text-xs text-red-600 dark:text-red-400">{props.runError}</p>}</div>}
    </div>
  );
}
