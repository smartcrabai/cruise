export default function PublishDialog(props) {
  return (
    <div id={`dialog-publish-${props.id}`} class="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <form role="dialog" aria-modal="true" hx-post={props.submitUrl} hx-swap="none" hx-disable="this" class="bg-white dark:bg-gray-900 rounded-lg shadow-xl border border-gray-200 dark:border-gray-700 p-6 max-w-sm w-full space-y-4">
        <h2 class="text-lg font-semibold text-gray-900 dark:text-gray-100">Publish as GitHub Issue</h2>
        <p class="text-sm text-gray-500 dark:text-gray-400">Publish session {props.id}'s plan.md as a GitHub issue, unchanged, and delete the local session. This cannot be undone.</p>
        <label class="flex items-center gap-2 text-sm text-gray-700 dark:text-gray-300">
          <input type="checkbox" name="triggerCruise" />
          Post an @cruise run comment after creating the issue
        </label>
        <div class="flex gap-2 justify-end">
          <button type="button" hx-on:click={`document.getElementById('dialog-publish-${props.id}').remove()`} class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-gray-500 dark:text-gray-400 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Cancel</button>
          <button type="submit" class="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700">Publish</button>
        </div>
      </form>
    </div>
  );
}
