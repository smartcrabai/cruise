export default function TabDag(props) {
  return <div id={`tab-dag-${props.id}`} role="tabpanel" class="p-6">
    {props.mermaidSource && <pre class="mermaid text-xs">{props.mermaidSource}</pre>}
    {props.message && <p class="p-4 text-xs text-gray-500 dark:text-gray-400">{props.message}</p>}
  </div>;
}
