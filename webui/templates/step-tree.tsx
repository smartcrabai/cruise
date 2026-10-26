export default function StepTree(props) {
  return <ul class="space-y-1 text-sm">
    {props.flags.map(flag => <li class="flex items-center gap-2">
      <label class="flex items-center gap-2 text-gray-700 dark:text-gray-300">
        <span>{flag.name}</span>
        <select name={"flag." + flag.name} class="rounded border border-gray-300 bg-white px-1 text-sm dark:border-gray-600 dark:bg-gray-800" hx-get="/webui/new/steps/flags" hx-trigger="change" hx-target="#step-tree" hx-include="closest form">
          {flag.on && <option value="on" selected>on</option>}
          {!flag.on && <option value="on">on</option>}
          {!flag.on && <option value="off" selected>off</option>}
          {flag.on && <option value="off">off</option>}
        </select>
        {flag.description && <span class="text-xs text-gray-500 dark:text-gray-400">{flag.description}</span>}
      </label>
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
