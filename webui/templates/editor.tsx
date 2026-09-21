export default function Editor(props) {
  return (
    <form hx-post={props.submitUrl} hx-swap="none" hx-disable="this" class="space-y-2 py-3">
      <label class="block text-sm font-medium text-gray-700 dark:text-gray-300">{props.label}</label>
      {props.required && <textarea name={props.field} rows="3" required placeholder={props.placeholder} class="w-full bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 placeholder-gray-400 dark:placeholder-gray-600 focus:border-blue-500 outline-none resize-none"></textarea>}
      {!props.required && <textarea name={props.field} rows="3" placeholder={props.placeholder} class="w-full bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 placeholder-gray-400 dark:placeholder-gray-600 focus:border-blue-500 outline-none resize-none"></textarea>}
      <div class="flex gap-2">
        <button type="submit" class="px-4 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700">{props.submitLabel}</button>
        <button type="button" hx-on:click={`document.getElementById('editor-${props.id}').innerHTML = ''`} class="px-4 py-1.5 border border-gray-300 dark:border-gray-700 text-gray-500 dark:text-gray-400 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Cancel</button>
      </div>
    </form>
  );
}
