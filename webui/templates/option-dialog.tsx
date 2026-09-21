export default function OptionDialog(props) {
  return (
    <div id={`dialog-${props.requestId}`} class="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <div role="dialog" aria-modal="true" class="bg-gray-50 dark:bg-gray-900 border border-gray-300 dark:border-gray-700 rounded-lg p-6 max-w-lg w-full space-y-4">
        <h2 class="text-gray-900 dark:text-gray-100 font-semibold text-lg">{props.sessionTitle}</h2>
        <p class="text-sm text-gray-700 dark:text-gray-300 whitespace-pre-wrap">{props.prompt}</p>
        <div class="space-y-3">
          {props.choices.map(choice => <button type="button" class="w-full px-4 py-2 bg-gray-200 dark:bg-gray-700 text-gray-800 dark:text-gray-200 rounded text-sm hover:bg-gray-300 dark:hover:bg-gray-600 text-left" hx-post={props.submitUrl} hx-vals={choice.vals} hx-swap="none">{choice.label}</button>)}
        </div>
        {props.hasTextInput && <form hx-post={props.submitUrl} hx-swap="none" hx-disable="this" class="space-y-2">
          <input type="hidden" name="requestId" value={props.requestId} />
          <input type="hidden" name="nextStep" value={props.textNextStep} />
          <label class="text-sm text-gray-700 dark:text-gray-300 block">{props.textLabel}</label>
          <input type="text" name="textInput" required class="w-full bg-gray-100 dark:bg-gray-800 border border-gray-300 dark:border-gray-700 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200" />
          <button type="submit" class="px-4 py-2 bg-blue-600 text-white rounded text-sm">Submit</button>
        </form>}
      </div>
    </div>
  );
}
