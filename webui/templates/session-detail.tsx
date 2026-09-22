export default function SessionDetail(props) {
  return (
    <div id="session-detail" data-session-id={props.id} class="h-full flex flex-col">
      {props.header}
      <div id={`ask-panel-${props.id}`}>
        {props.askPanel}
      </div>
      <pre id={`plan-progress-${props.id}`} class="px-6 py-2 text-xs font-mono whitespace-pre-wrap text-gray-500 dark:text-gray-400 max-h-40 overflow-auto"></pre>
      <div id={`editor-${props.id}`} class="px-6">
        {props.editor}
      </div>
      <div id={`settings-${props.id}`} class="px-6">
        {props.settings}
      </div>
      <nav role="tablist" class="flex border-b border-gray-200 dark:border-gray-800">
        <button
          type="button"
          role="tab"
          hx-get={props.tabHrefs.info}
          hx-target={`#tab-panel-${props.id}`}
          hx-swap="innerHTML"
          hx-push-url={props.pushHrefs.info}
          aria-selected={props.activeTab === "info" ? "true" : "false"}
          class="cruise-tab px-4 py-2 text-xs font-medium transition-colors"
        >
          Info
        </button>
        <button
          type="button"
          role="tab"
          hx-get={props.tabHrefs.dag}
          hx-target={`#tab-panel-${props.id}`}
          hx-swap="innerHTML"
          hx-push-url={props.pushHrefs.dag}
          aria-selected={props.activeTab === "dag" ? "true" : "false"}
          class="cruise-tab px-4 py-2 text-xs font-medium transition-colors"
        >
          Graph
        </button>
        <button
          type="button"
          role="tab"
          hx-get={props.tabHrefs.plan}
          hx-target={`#tab-panel-${props.id}`}
          hx-swap="innerHTML"
          hx-push-url={props.pushHrefs.plan}
          aria-selected={props.activeTab === "plan" ? "true" : "false"}
          class="cruise-tab px-4 py-2 text-xs font-medium transition-colors"
        >
          Plan
        </button>
        <button
          type="button"
          role="tab"
          hx-get={props.tabHrefs.log}
          hx-target={`#tab-panel-${props.id}`}
          hx-swap="innerHTML"
          hx-push-url={props.pushHrefs.log}
          aria-selected={props.activeTab === "log" ? "true" : "false"}
          class="cruise-tab px-4 py-2 text-xs font-medium transition-colors"
        >
          Log
        </button>
      </nav>
      <div id={`tab-panel-${props.id}`} class="flex-1 min-h-0 overflow-auto">
        {props.tabPanel}
      </div>
    </div>
  );
}
