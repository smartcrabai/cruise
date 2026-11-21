export default function StepTree(props) {
  return <ul class="space-y-1 text-sm">
    {props.flags.map(flag => <li class="flex flex-wrap items-center gap-x-2 gap-y-1">
      <input type="hidden" name={`flag.${flag.name}`} value={flag.on ? "on" : "off"} />
      <button type="button" id={`flag-toggle-${flag.name}`} data-flag-toggle
        aria-pressed={flag.on ? "true" : "false"}
        aria-describedby={flag.description ? `flag-description-${flag.name}` : undefined}
        hx-get="/webui/new/steps/flags" hx-target="#step-tree" hx-swap="innerHTML"
        hx-include="closest form" hx-vals={flag.toggleVals} hx-sync="#step-tree:drop"
        hx-disable="#step-tree input[name='runSteps'], [data-step-tree-commit]"
        class={flag.on ? "inline-flex min-h-11 max-w-full flex-wrap items-center gap-x-2 rounded border border-blue-600 bg-blue-100 px-3 py-2 text-left text-blue-900 focus-visible:outline-2 focus-visible:outline-blue-500 dark:border-blue-400 dark:bg-blue-900 dark:text-blue-100 disabled:opacity-50" : "inline-flex min-h-11 max-w-full flex-wrap items-center gap-x-2 rounded border border-gray-300 bg-white px-3 py-2 text-left text-gray-700 focus-visible:outline-2 focus-visible:outline-blue-500 dark:border-gray-600 dark:bg-gray-800 dark:text-gray-300 disabled:opacity-50"}>
        <span class="min-w-0 break-all">{flag.name}</span>
        <span aria-hidden="true" class="font-semibold">{" "}{flag.on ? props.t.session.on : props.t.session.off}</span>
        <span aria-hidden="true" class="htmx-indicator h-3 w-3 animate-spin rounded-full border-2 border-current border-r-transparent"></span>
      </button>
      {flag.description && <span id={`flag-description-${flag.name}`} class="min-w-0 break-words text-xs text-gray-500 dark:text-gray-400">{flag.description}</span>}
    </li>)}
    {props.empty && <li class="text-xs text-gray-500 dark:text-gray-400">No steps available.</li>}
    {props.steps.map(step => <li style={step.indentStyle} class="flex items-center gap-2">
      <label class="flex items-center gap-2 text-gray-700 dark:text-gray-300">
        {step.checked && <input type="checkbox" name="runSteps" value={step.id} checked class="accent-blue-500" />}
        {!step.checked && <input type="checkbox" name="runSteps" value={step.id} class="accent-blue-500" />}
        <input type="hidden" name="stepIds" value={step.id} />
        <span>{step.label}</span>
        {step.afterPr && <span class="text-xs text-gray-500 dark:text-gray-400">after PR</span>}
      </label>
    </li>)}
  </ul>;
}
