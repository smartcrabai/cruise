export default function AskPanel(props) {
  return (
    <section aria-label="Planning agent question" class="rounded border border-blue-300/60 dark:border-blue-800/60 bg-blue-50/50 dark:bg-blue-950/20 p-4 space-y-3">
      <div>
        <h3 class="text-sm font-semibold text-blue-800 dark:text-blue-200">The planning agent has a question</h3>
        <p class="mt-1 text-sm text-gray-700 dark:text-gray-300 whitespace-pre-wrap">{props.question}</p>
      </div>
      <form hx-post={`/webui/sessions/${props.sessionId}/ask-answer`} hx-swap="none" hx-disable="this" class="space-y-3">
        <input type="hidden" name="requestId" value={props.requestId} />
        <textarea name="answer" required rows="3" aria-label="Your answer" class="w-full h-28 border border-gray-300 dark:border-gray-700 bg-gray-50 dark:bg-gray-900 rounded px-3 py-2 text-sm text-gray-800 dark:text-gray-200 placeholder-gray-400 dark:placeholder-gray-600 outline-none focus:border-blue-500 resize-none" placeholder="Your answer... (Cmd/Ctrl+Enter to send)"></textarea>
        <div class="flex justify-end">
          <button type="submit" class="px-4 py-1.5 bg-blue-600 text-white rounded text-sm hover:bg-blue-700">Send answer</button>
        </div>
      </form>
    </section>
  );
}
