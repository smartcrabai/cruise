export default function RunAllResults(props) {
  return (
    <>
      {props.resultsEmpty && <p class="text-xs text-gray-500 dark:text-gray-400">No results yet.</p>}
      {props.results.map(result => <div class="flex items-start gap-2 px-3 py-2 rounded bg-gray-50/50 dark:bg-gray-900/50"><div class="flex-1 min-w-0"><p class="text-sm text-gray-700 dark:text-gray-300 truncate">{result.title}</p>{result.error && <p class="text-xs text-red-600 dark:text-red-400 mt-0.5 truncate">{result.error}</p>}</div><span class={result.failed ? "inline-flex px-2 py-0.5 rounded text-xs font-medium bg-red-100/50 dark:bg-red-900/50 text-red-700 dark:text-red-300" : "inline-flex px-2 py-0.5 rounded text-xs font-medium bg-gray-100/50 dark:bg-gray-800/50 text-gray-600 dark:text-gray-400"}>{result.phase}</span></div>)}
    </>
  );
}
