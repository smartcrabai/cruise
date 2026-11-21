export default function NewSessionActions(props) {
  return (
    <div id="new-session-actions">
      <div class="flex flex-wrap gap-2">
        <button type="button" data-step-tree-commit hx-post="/webui/drafts" hx-include="closest form" hx-swap="none" class="px-5 py-2 border border-gray-300 dark:border-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Save draft</button>
        <button type="button" data-step-tree-commit hx-post="/webui/sessions/draft" hx-include="closest form" hx-target="#main" hx-swap="innerHTML" class="px-5 py-2 border border-gray-300 dark:border-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Create draft</button>
        {props.showExec && <button type="button" data-step-tree-commit hx-post="/webui/sessions/exec" hx-include="closest form" hx-target="#main" hx-swap="innerHTML" class="px-5 py-2 border border-gray-300 dark:border-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Exec</button>}
        <button type="submit" data-step-tree-commit class="px-5 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700">{props.forceExec ? "Execute" : "Create & plan"}</button>
      </div>
      {props.forceExec && <p class="text-xs text-gray-500 dark:text-gray-400 mt-2">This workflow sets force_exec: it runs directly in the working directory without planning, a worktree, or a PR. Use Create draft to plan instead.</p>}
    </div>
  );
}
