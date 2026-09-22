export default function Toast(props) {
  return (
    <div data-toast="1" class={props.tone === "inputRequired" ? "flex items-start gap-3 px-4 py-3 rounded-lg border shadow-xl text-sm pointer-events-auto border-amber-300 dark:border-amber-700 bg-amber-100/80 dark:bg-amber-900/80 text-amber-900 dark:text-amber-100" : props.tone === "completed" ? "flex items-start gap-3 px-4 py-3 rounded-lg border shadow-xl text-sm pointer-events-auto border-green-300 dark:border-green-700 bg-green-100/80 dark:bg-green-900/80 text-green-900 dark:text-green-100" : props.tone === "failed" ? "flex items-start gap-3 px-4 py-3 rounded-lg border shadow-xl text-sm pointer-events-auto border-red-300 dark:border-red-700 bg-red-100/80 dark:bg-red-900/80 text-red-900 dark:text-red-100" : "flex items-start gap-3 px-4 py-3 rounded-lg border shadow-xl text-sm pointer-events-auto border-blue-300 dark:border-blue-700 bg-blue-100/80 dark:bg-blue-900/80 text-blue-900 dark:text-blue-100"}>
      <div class="flex-1 min-w-0">
        <div class="font-medium">{props.label}</div>
        <div class="text-xs opacity-75 truncate mt-0.5">{props.sessionInput}</div>
        {props.detail && <div data-testid="toast-detail" class="text-xs opacity-60 truncate mt-0.5">{props.detail}</div>}
      </div>
      <button type="button" aria-label="Dismiss" onclick="this.closest('[data-toast]').remove()" class="opacity-60 hover:opacity-100 flex-shrink-0 text-xs mt-0.5">x</button>
    </div>
  );
}
