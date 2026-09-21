export default function RunAllProgress(props) {
  return (
    <>
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
    </>
  );
}
