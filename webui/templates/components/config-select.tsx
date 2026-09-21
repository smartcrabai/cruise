export default function ConfigSelect(props) {
  return <select id="config-path" name="configPath" class="w-full bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 focus:border-blue-500 outline-none disabled:opacity-50" hx-get="/webui/new/steps" hx-trigger="change" hx-target="#step-tree" hx-include="closest form">
    {props.sentinels.map(option => <>
      {option.selected && <option value={option.value} selected>{option.label}</option>}
      {!option.selected && <option value={option.value}>{option.label}</option>}
    </>)}
    {props.groups.map(group => <optgroup label={group.label}>{group.options.map(option => <>
      {option.selected && <option value={option.value} selected>{option.label}</option>}
      {!option.selected && <option value={option.value}>{option.label}</option>}
    </>)}</optgroup>)}
  </select>;
}
