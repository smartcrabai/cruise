export default function TabLog(props) {
  return (
    <>
      {props.running
        ? <pre id={`tab-log-${props.id}`} class="text-xs font-mono bg-white dark:bg-gray-950 text-gray-700 dark:text-gray-300 p-4 overflow-auto whitespace-pre-wrap leading-relaxed h-full" hx-get={props.pollUrl} hx-trigger="every 2s" hx-swap="outerHTML">{props.empty && "No log entries yet."}{!props.empty && props.savedLog}</pre>
        : <pre id={`tab-log-${props.id}`} class="text-xs font-mono bg-white dark:bg-gray-950 text-gray-700 dark:text-gray-300 p-4 overflow-auto whitespace-pre-wrap leading-relaxed h-full">{props.empty && "No log entries yet."}{!props.empty && props.savedLog}</pre>}
    </>
  );
}
