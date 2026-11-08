import RunAllControl from "./components/run-all-control";

export default function Sidebar(props) {
  return (
    <div class="h-full flex flex-col">
      <div class="px-4 py-3 border-b border-gray-200 dark:border-gray-800 space-y-1.5">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <h2 class="text-sm font-semibold text-gray-800 dark:text-gray-200">{props.t.sidebar.sessions}</h2>
          <div class="flex items-center gap-1">
            <button type="button" hx-post="/webui/clean" hx-swap="none" hx-confirm={props.t.sidebar.clean_confirm} class="whitespace-nowrap px-1.5 py-1 text-xs text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-800 rounded" title={props.t.sidebar.clean}>{props.t.sidebar.clean}</button>
            <RunAllControl runAllActive={props.runAllActive} runnableCount={props.runnableCount} runAllConfirm={props.runAllConfirm} t={props.t} />
            <button type="button" hx-get="/new" hx-target="#main" hx-push-url="true" class="whitespace-nowrap px-1.5 py-1 text-xs bg-blue-600 text-white hover:bg-blue-700 rounded">+ {props.t.sidebar.new}</button>
            <button type="button" hx-get="/webui/settings" hx-target="#dialogs" hx-swap="innerHTML" aria-label={props.t.sidebar.settings} title={props.t.sidebar.settings} class="whitespace-nowrap px-1.5 py-1 text-xs text-gray-500 dark:text-gray-400 hover:text-gray-800 dark:hover:text-gray-200 hover:bg-gray-200 dark:hover:bg-gray-800 rounded">⚙</button>
          </div>
        </div>
        <p id="clean-message" class="text-xs text-gray-500 dark:text-gray-400"></p>
      </div>
      <div id="session-list" class="flex-1 overflow-y-auto" hx-get="/webui/sidebar" hx-trigger="every 3s" hx-swap="innerMorph">{props.rows}</div>
      <div class="flex-shrink-0 border-t border-gray-200 dark:border-gray-800 px-4 py-2">
        <label class="text-xs text-gray-500 dark:text-gray-400">{props.t.sidebar.language}
          <select id="ui-language" aria-label={props.t.sidebar.language} class="ml-2 bg-transparent">{props.languages.map(language => <option value={language.tag} selected={language.selected}>{language.name}</option>)}</select>
        </label>
        <div class="text-xs text-gray-500 dark:text-gray-400">v{props.version}</div>
      </div>
    </div>
  );
}
