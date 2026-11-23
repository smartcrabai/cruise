export default function ManagerPrompt(props) {
  return <article id={`manager-prompt-${props.prompt.requestId}`} class="space-y-3 p-3 rounded border border-blue-500">
    <a href={`/sessions/${props.prompt.sessionId}`} class="text-sm text-blue-700 dark:text-blue-400 hover:underline break-all">{props.prompt.sessionId}</a>
    {props.prompt.kind === "ask" && <form hx-post={`/webui/manager/${props.key}/answer`} hx-swap="none" hx-disable="this" class="space-y-2">
      <input type="hidden" name="goal_id" value={props.goalId} />
      <input type="hidden" name="prompt_id" value={props.prompt.requestId} />
      <label for={`manager-answer-${props.prompt.requestId}`} class="block whitespace-pre-wrap break-words">{props.prompt.question}</label>
      <textarea id={`manager-answer-${props.prompt.requestId}`} name="answer" required rows="3" class="w-full bg-transparent border border-gray-300 dark:border-gray-700 rounded px-3 py-2"></textarea>
      <button type="submit" class="px-4 py-2 rounded bg-blue-600 text-white text-sm">{props.t.manager.send_answer}</button>
    </form>}
    {props.prompt.kind === "option" && <div class="space-y-2">
      {props.prompt.question && <p class="whitespace-pre-wrap break-words">{props.prompt.question}</p>}
      {props.prompt.choices.map(choice => <form hx-post={`/webui/manager/${props.key}/answer-option`} hx-swap="none" hx-disable="this" class="space-y-2">
        <input type="hidden" name="goal_id" value={props.goalId} />
        <input type="hidden" name="prompt_id" value={props.prompt.requestId} />
        <input type="hidden" name="nextStep" value={choice.nextStep || ""} />
        {choice.kind === "textInput" ? <>
          <label class="block whitespace-pre-wrap break-words">{choice.label}<input type="text" name="textInput" required class="block w-full mt-1 bg-transparent border border-gray-300 dark:border-gray-700 rounded px-3 py-2" /></label>
          <button type="submit" class="px-4 py-2 rounded bg-blue-600 text-white text-sm">{props.t.manager.send_answer}</button>
        </> : <button type="submit" class="px-4 py-2 rounded border border-gray-300 dark:border-gray-700 text-sm whitespace-pre-wrap">{choice.label}</button>}
      </form>)}
    </div>}
  </article>;
}
