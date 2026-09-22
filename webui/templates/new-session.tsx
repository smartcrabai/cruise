export default function NewSession(props) {
  return (
    <form
      id="new-session-form"
      hx-post="/webui/sessions"
      hx-target="#main"
      hx-swap="innerHTML"
      hx-disable="this"
      class="p-6 max-w-2xl mx-auto space-y-4"
    >
      <h2 class="text-xl font-semibold text-gray-900 dark:text-gray-100">New Session</h2>
      {props.error && <p class="text-sm text-red-600 dark:text-red-400">{props.error}</p>}

      <div class="space-y-1.5">
        <span class="text-xs text-gray-500 dark:text-gray-400 uppercase tracking-wide">Source</span>
        <div class="flex gap-4" role="radiogroup" aria-label="Workspace source">
          <label class="flex items-center gap-2 cursor-pointer">
            {props.directorySelected && <input type="radio" name="sourceMode" value="directory" checked hx-on:change="app.toggleSource(this.value)" class="accent-blue-500" />}
            {!props.directorySelected && <input type="radio" name="sourceMode" value="directory" hx-on:change="app.toggleSource(this.value)" class="accent-blue-500" />}
            <span class="text-sm text-gray-700 dark:text-gray-300">Directory</span>
          </label>
          <label class="flex items-center gap-2 cursor-pointer">
            {!props.directorySelected && <input type="radio" name="sourceMode" value="repo" checked hx-on:change="app.toggleSource(this.value)" class="accent-blue-500" />}
            {props.directorySelected && <input type="radio" name="sourceMode" value="repo" hx-on:change="app.toggleSource(this.value)" class="accent-blue-500" />}
            <span class="text-sm text-gray-700 dark:text-gray-300">GitHub Repository</span>
          </label>
        </div>
      </div>

      <fieldset id="source-directory" class={props.directorySelected ? "space-y-2" : "space-y-2 hidden"}>
        <label for="base-dir-input" class="text-xs text-gray-500 dark:text-gray-400 uppercase tracking-wide">Working Directory</label>
        <input
          id="base-dir-input"
          type="text"
          name="baseDir"
          value={props.baseDir}
          autocomplete="off"
          role="combobox"
          aria-autocomplete="list"
          hx-get="/webui/directories"
          hx-trigger="input changed delay:150ms"
          hx-target="#dir-suggestions"
          hx-include="this"
          placeholder="e.g. /Users/you/projects/myapp"
          class="w-full bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 placeholder-gray-400 dark:placeholder-gray-600 focus:border-blue-500 outline-none"
        />
        <div id="dir-suggestions" class="relative"></div>
        {props.recentWorkingDirs && <div class="relative z-[60] flex flex-wrap gap-2 pt-1">{props.recentWorkingDirs.map(dir => <button type="button" class="px-2.5 py-1 rounded-full border border-gray-300 dark:border-gray-700 bg-gray-50 dark:bg-gray-900 text-xs text-gray-700 dark:text-gray-300 hover:bg-gray-200 dark:hover:bg-gray-800" hx-on:click={`app.pickDir('${dir}')`}>{dir}</button>)}</div>}
      </fieldset>

      <fieldset id="source-repo" class={props.directorySelected ? "space-y-2 hidden" : "space-y-2"}>
        <label for="repo-input" class="text-xs text-gray-500 dark:text-gray-400 uppercase tracking-wide">Repository</label>
        <input
          id="repo-input"
          type="text"
          name="repo"
          value={props.repo}
          list="repo-options"
          placeholder="owner/repo"
          class="w-full bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 placeholder-gray-400 dark:placeholder-gray-600 focus:border-blue-500 outline-none"
        />
        <datalist id="repo-options" hx-get="/webui/repos" hx-trigger="load" hx-swap="innerHTML"></datalist>
      </fieldset>

      <div
        id="config-select"
        hx-get="/webui/configs"
        hx-trigger="change from:#base-dir-input delay:250ms, change from:#repo-input delay:350ms, change from:input[name='sourceMode']"
        hx-include="#new-session-form"
        hx-target="this"
        hx-swap="innerHTML"
      >{props.configSelect}</div>
      <div
        id="step-tree"
        hx-get="/webui/new/steps"
        hx-trigger="change from:#base-dir-input delay:250ms, change from:#repo-input delay:350ms, change from:input[name='sourceMode']"
        hx-include="#new-session-form"
        hx-target="this"
        hx-swap="innerHTML"
      >{props.steps}</div>

      <div class="space-y-1.5">
        <label for="task-input" class="text-xs text-gray-500 dark:text-gray-400 uppercase tracking-wide">Task</label>
        <textarea
          id="task-input"
          name="input"
          rows="6"
          placeholder="Describe the task..."
          class="w-full bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 placeholder-gray-400 dark:placeholder-gray-600 focus:border-blue-500 outline-none resize-none"
        >{props.input}</textarea>
      </div>

      <label class="flex items-center gap-2 cursor-pointer">
        {props.skipPlanning && <input type="checkbox" name="skipPlanning" checked class="accent-blue-500" />}
        {!props.skipPlanning && <input type="checkbox" name="skipPlanning" class="accent-blue-500" />}
        <span class="text-sm text-gray-700 dark:text-gray-300">Use input as plan (skip LLM planning)</span>
      </label>
      <label class="flex items-center gap-2 cursor-pointer">
        {props.grill && <input type="checkbox" name="grill" checked class="accent-blue-500" />}
        {!props.grill && <input type="checkbox" name="grill" class="accent-blue-500" />}
        <span class="text-sm text-gray-700 dark:text-gray-300">Grill me (interview one question at a time, then write the plan; SDK backend only)</span>
      </label>
      <label class="flex items-center gap-2 cursor-pointer">
        {props.noInteractivePlanning && <input type="checkbox" name="noInteractivePlanning" checked class="accent-blue-500" />}
        {!props.noInteractivePlanning && <input type="checkbox" name="noInteractivePlanning" class="accent-blue-500" />}
        <span class="text-sm text-gray-700 dark:text-gray-300">Non-interactive planning (agent writes plan.md directly, no planning tools)</span>
      </label>
      <label class="flex items-center gap-2 cursor-pointer">
        {props.formalSpec && <input type="checkbox" name="formalSpec" checked class="accent-blue-500" />}
        {!props.formalSpec && <input type="checkbox" name="formalSpec" class="accent-blue-500" />}
        <span class="text-sm text-gray-700 dark:text-gray-300">Formal specification (Quint and Alloy, with semantic comments)</span>
      </label>

      <label for="workspace-mode" class="text-xs text-gray-500 dark:text-gray-400 uppercase tracking-wide">Workspace</label>
      <select id="workspace-mode" name="workspaceMode" class="w-full bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 focus:border-blue-500 outline-none">
        {props.workspaceMode === "Worktree" ? <option value="Worktree" selected>Worktree</option> : <option value="Worktree">Worktree</option>}
        {props.workspaceMode === "CurrentBranch" ? <option value="CurrentBranch" selected>Current branch</option> : <option value="CurrentBranch">Current branch</option>}
      </select>

      <div class="space-y-2">
        <span class="text-xs text-gray-500 dark:text-gray-400 uppercase tracking-wide">Images</span>
        <div id="attachments">{props.attachments}</div>
        <input
          type="file"
          name="files"
          accept="image/*"
          multiple
          hx-post="/webui/attachments"
          hx-encoding="multipart/form-data"
          hx-trigger="change"
          hx-target="#attachments"
          hx-include="#attachments"
          hx-swap="innerHTML"
          class="block w-full text-sm text-gray-700 dark:text-gray-300 file:mr-3 file:py-1.5 file:px-3 file:rounded file:border-0 file:bg-gray-100 dark:file:bg-gray-800 file:text-gray-700 dark:file:text-gray-300 hover:file:bg-gray-200 dark:hover:file:bg-gray-700"
        />
      </div>

      <div class="flex gap-2">
        <button type="button" hx-post="/webui/drafts" hx-include="closest form" hx-swap="none" class="px-5 py-2 border border-gray-300 dark:border-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">
          Save draft
        </button>
        <button type="button" hx-post="/webui/sessions/draft" hx-include="closest form" hx-target="#main" hx-swap="innerHTML" class="px-5 py-2 border border-gray-300 dark:border-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">
          Create draft
        </button>
        <button type="submit" class="px-5 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700">
          Create &amp; plan
        </button>
      </div>
      <p id="draft-status" class="text-xs text-gray-500 dark:text-gray-400"></p>
    </form>
  );
}
