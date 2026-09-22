import PhaseBadge from "./components/phase-badge";

export default function SessionRow(props) {
  return (
    <a id={`session-row-${props.row.id}`} href={props.row.href} hx-get={props.row.href} hx-target="#main" hx-swap="innerHTML" hx-push-url="true" class={props.row.selected ? "block w-full text-left px-4 py-2.5 border-b border-gray-200/50 dark:border-gray-800/50 hover:bg-gray-200 dark:hover:bg-gray-800 transition-colors bg-gray-100 dark:bg-gray-800" : "block w-full text-left px-4 py-2.5 border-b border-gray-200/50 dark:border-gray-800/50 hover:bg-gray-200 dark:hover:bg-gray-800 transition-colors"}>
      <div class="flex items-center justify-between gap-2 mb-0.5">
        <span class="text-xs text-gray-500 dark:text-gray-400 font-mono truncate">{props.row.id}</span><PhaseBadge badge={props.row.badge} />
      </div>
      <p class="text-sm text-gray-700 dark:text-gray-300 truncate">{props.row.title}</p>
      {props.row.subtitle && <p class="text-xs text-gray-500 dark:text-gray-400 truncate">{props.row.subtitle}</p>}
      <div class="flex items-center gap-1.5 mt-0.5">
        <span class="text-xs text-blue-600/70 dark:text-blue-400/70 font-mono truncate">{props.row.dirLabel}</span>
        <span class="text-xs text-gray-500 dark:text-gray-400">{props.row.timeLabel}</span>
      </div>
    </a>
  );
}
