export default function SessionSettings(props) {
  return <form id={`session-settings-${props.id}`} class="py-3 space-y-3">
    <input type="hidden" name="baseDir" value={props.baseDir} />
    <input type="hidden" name="repo" value={props.repo} />
    {props.configSelect}
    <div id="step-tree">{props.steps}</div>
    <select name="currentStep" class="w-full bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 focus:border-blue-500 outline-none disabled:opacity-50">
      <option value="__unchanged__" selected>Leave current step unchanged</option>
      <option value="__clear__">Clear current step</option>
      {props.stepOptions.map(step => <option value={step.value}>{step.label}</option>)}
    </select>
    <div class="flex gap-2">
      <button type="submit" class="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700" hx-post={`/webui/sessions/${props.id}/settings`} hx-include="closest form" hx-swap="none">Save</button>
      <button type="submit" class="px-4 py-2 border border-blue-600 text-blue-600 dark:text-blue-400 rounded text-sm hover:bg-blue-50 dark:hover:bg-blue-900/30" hx-post={`/webui/sessions/${props.id}/settings/regenerate`} hx-include="closest form" hx-swap="none" hx-confirm="Regenerate the plan with these settings?">Save &amp; regenerate</button>
      <button type="button" class="px-4 py-2 bg-gray-200 dark:bg-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm hover:bg-gray-300 dark:hover:bg-gray-600" hx-on:click={`document.getElementById('settings-${props.id}').innerHTML = ''`}>Close</button>
    </div>
  </form>;
}
