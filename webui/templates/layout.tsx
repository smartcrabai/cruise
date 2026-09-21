export default function Layout(props) {
  return (
    <>
      <Head>
        <meta charset="utf-8" />
        <title>Cruise</title>
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <link rel="icon" href="/static/favicon.svg" />
        <link rel="stylesheet" href="/static/app.css" />
        <script src="/static/htmax.min.js"></script>
        <script src="/static/app.js" defer></script>
      </Head>
      <div id="app" class="flex h-screen bg-white dark:bg-gray-950 text-gray-900 dark:text-gray-100">
        <aside id="sidebar" class="flex-shrink-0 overflow-hidden border-r border-gray-200 dark:border-gray-800 bg-gray-50 dark:bg-gray-900" style="width:288px">{props.sidebar}</aside>
        <div id="sidebar-resizer" role="separator" aria-orientation="vertical" tabindex="0" class="w-1 cursor-col-resize bg-gray-200 dark:bg-gray-800 hover:bg-blue-500"></div>
        <main id="main" class="flex-1 min-w-0 overflow-auto">{props.main}</main>
      </div>
      <div id="toasts" class="fixed bottom-4 right-4 z-50 flex flex-col gap-2 max-w-sm w-full pointer-events-none"></div>
      <div id="dialogs">{props.dialogs}</div>
      <div hx-sse:connect="/webui/events" hx-swap="none" hx-on:notify="app.notify(event)"></div>
    </>
  );
}
