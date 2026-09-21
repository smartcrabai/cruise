export default function SettingsModal(props) {
  return (
    <div class="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <form hx-post="/webui/settings" hx-swap="none" role="dialog" aria-modal="true" aria-labelledby="settings-title" class="bg-gray-50 dark:bg-gray-900 rounded-lg shadow-xl border border-gray-300 dark:border-gray-700 p-6 max-w-sm w-full space-y-4">
        <h2 id="settings-title" class="text-lg font-semibold text-gray-900 dark:text-gray-100">Settings</h2>
        <div class="space-y-1.5">
          <label for="run-all-parallelism" class="text-sm text-gray-500 dark:text-gray-400">Run All Parallelism</label>
          <input id="run-all-parallelism" name="runAllParallelism" type="number" min="1" value={props.runAllParallelism} class="w-full bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-1.5 text-sm text-gray-800 dark:text-gray-200 outline-none focus:border-blue-500" />
          {props.error && <p class="text-sm text-red-600 dark:text-red-400">{props.error}</p>}
        </div>
        <div class="flex gap-2 justify-end">
          <button type="button" hx-on:click="document.getElementById('dialogs').innerHTML = ''" class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-gray-500 dark:text-gray-400 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Cancel</button>
          <button type="submit" class="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700">Save</button>
        </div>
      </form>
    </div>
  );
}
