export default function ManagerList(props) {
  return <div class="p-4 md:p-6 space-y-6">
    <h1 class="text-2xl font-semibold">{props.t.manager.title}</h1>
    <form method="post" action="/webui/manager/open" hx-post="/webui/manager/open" hx-swap="none" hx-disable="this" class="space-y-2">
      <label class="block text-sm" for="manager-open-path">{props.t.manager.repository_path}</label>
      <div class="flex flex-wrap gap-2">
        <input id="manager-open-path" name="path" required class="flex-1 min-w-0 bg-transparent border border-gray-300 dark:border-gray-700 rounded px-3 py-2" />
        <button type="submit" class="px-4 py-2 bg-blue-600 text-white rounded text-sm">{props.t.manager.open_repository}</button>
      </div>
    </form>
    <div id="manager-open-error" role="alert" class="text-sm text-red-700 dark:text-red-300">{props.error}</div>
    {!props.records.length && <p class="text-gray-500 dark:text-gray-400">{props.t.manager.no_repositories}</p>}
    <ul class="space-y-2">
      {props.records.map(record => <li class="border border-gray-300 dark:border-gray-700 rounded p-3 space-y-2">
        <a href={record.url} class="block font-mono break-all text-blue-700 dark:text-blue-400 hover:underline">{record.repository}</a>
        <ul class="flex flex-wrap gap-3 text-sm">
          {record.counts.map(count => <li>{props.t.manager[count.status]}: {count.count}</li>)}
          <li>{record.pending ? props.t.manager.pending_proposal : props.t.manager.no_proposal}</li>
          {record.running && <li>{props.t.manager.running}</li>}
        </ul>
      </li>)}
    </ul>
  </div>;
}
