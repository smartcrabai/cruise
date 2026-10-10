export default function MergePrDialog(props) {
  return (
    <div id={`dialog-merge-pr-${props.id}`} class="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <form role="dialog" aria-modal="true" hx-post={props.canMerge ? props.submitUrl : undefined} hx-swap="none" hx-disable="this" class="bg-white dark:bg-gray-900 rounded-lg shadow-xl border border-gray-200 dark:border-gray-700 p-6 max-w-md w-full space-y-4">
        <h2 class="text-lg font-semibold text-gray-900 dark:text-gray-100">Merge pull request</h2>
        <dl class="text-sm text-gray-700 dark:text-gray-300 space-y-1">
          <div>State: <span class="font-mono">{props.state}</span></div>
          <div>Mergeable: <span class="font-mono">{props.mergeable}</span></div>
          <div>Review: <span class="font-mono">{props.reviewDecision}</span></div>
        </dl>
        <div class="text-sm text-gray-700 dark:text-gray-300">
          <div>Checks:</div>
          {props.noChecks && <div class="text-gray-500 dark:text-gray-400">none</div>}
          <ul>
            {props.checks.map(check => <li class="font-mono">{check.name}: {check.status}</li>)}
          </ul>
        </div>
        {props.canMerge && <fieldset class="text-sm text-gray-700 dark:text-gray-300 space-y-1">
          <legend>Merge method</legend>
          <label class="flex items-center gap-2"><input type="radio" name="method" value="squash" checked /> Squash</label>
          <label class="flex items-center gap-2"><input type="radio" name="method" value="merge" /> Merge</label>
          <label class="flex items-center gap-2"><input type="radio" name="method" value="rebase" /> Rebase</label>
        </fieldset>}
        {!props.canMerge && <p class="text-sm text-gray-500 dark:text-gray-400">This pull request is not open. Run cruise clean to remove the session.</p>}
        <div class="flex gap-2 justify-end">
          <button type="button" hx-on:click={`document.getElementById('dialog-merge-pr-${props.id}').remove()`} class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-gray-500 dark:text-gray-400 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Cancel</button>
          {props.canMerge && <button type="submit" class="px-4 py-2 bg-green-700 text-white rounded text-sm hover:bg-green-600">Confirm merge</button>}
        </div>
      </form>
    </div>
  );
}
