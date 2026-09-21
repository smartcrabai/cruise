export default function DirectorySuggestions(props) {
  return (
    <>
      {props.empty
        ? <ul role="listbox" class="absolute z-10 w-full max-h-60 overflow-auto bg-white dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded shadow-lg"><li class="px-3 py-1.5 text-sm text-gray-500 dark:text-gray-400">No matches</li></ul>
        : <ul role="listbox" class="absolute z-10 w-full max-h-60 overflow-auto bg-white dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded shadow-lg">{props.entries.map(entry => <li role="option"><button type="button" class="w-full text-left px-3 py-1.5 text-sm hover:bg-gray-100 dark:hover:bg-gray-800" hx-on:click={`app.pickDir('${entry.path}')`}>{entry.name}</button></li>)}</ul>}
    </>
  );
}
