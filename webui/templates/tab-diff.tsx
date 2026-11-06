export default function TabDiff(props) {
  return <div id={`tab-diff-${props.id}`} role="tabpanel" class="p-4 md:p-6">
    <button type="button" hx-get={`/webui/sessions/${props.id}/tab/diff`} hx-target={`#tab-diff-${props.id}`} hx-swap="outerHTML" class="text-xs font-medium text-blue-600 dark:text-blue-400">Refresh</button>
    {props.message && <p class="mt-2 text-xs text-gray-500 dark:text-gray-400">{props.message}</p>}
    {props.truncated && <p class="mt-2 text-xs text-amber-600">Diff truncated.</p>}
    <pre class="mt-2 overflow-auto text-xs font-mono">{props.lines.map(line => <div class={line.class === "add" ? "text-green-600 dark:text-green-400" : line.class === "remove" ? "text-red-600 dark:text-red-400" : line.class === "hunk" ? "text-cyan-600 dark:text-cyan-400" : line.class === "header" ? "font-bold" : ""}>{line.text}</div>)}</pre>
  </div>;
}
