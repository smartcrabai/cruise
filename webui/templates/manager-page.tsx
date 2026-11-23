export default function ManagerPage(props) {
  return <div class="p-4 md:p-6 space-y-4">
    <a href="/manager" class="text-sm text-blue-700 dark:text-blue-400 hover:underline">{props.t.manager.title}</a>
    <h1 class="text-xl font-semibold font-mono break-all">{props.repository}</h1>
    <div id="manager-action-error" role="alert" class="sticky top-0 z-20 empty:hidden bg-red-50 dark:bg-gray-900 border border-red-300 dark:border-red-700 p-3 text-sm text-red-700 dark:text-red-300 break-words"></div>
    <div id="manager-region">{props.region}</div>
    <div id="manager-sync" class="hidden" hx-get={`/webui/manager/${props.key}/sync`} hx-trigger="every 3s" hx-swap="none"></div>
    <div id="manager-review-host"></div>
  </div>;
}
