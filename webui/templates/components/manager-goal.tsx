import ManagerReview from "./manager-review";
import ManagerTaskList from "./manager-task-list";

export default function ManagerGoal(props) {
  return <article id={`manager-goal-${props.goal.id}`} class="rounded border border-gray-300 dark:border-gray-700 p-3 space-y-3">
    <div class="flex flex-wrap items-start justify-between gap-2">
      <h3 class="font-semibold whitespace-pre-wrap break-words min-w-0">{props.goal.proposal.objective}</h3>
      <p class="text-sm font-semibold rounded bg-gray-100 dark:bg-gray-800 px-2 py-1">{props.t.manager[props.goal.status]}</p>
    </div>
    <p class="font-mono text-sm break-all">{props.goal.id}</p>
    {props.goal.reason && <p class="whitespace-pre-wrap break-words">{props.goal.reason}</p>}
    {props.goal.execution.integrated_tip && <p class="text-sm">{props.t.manager.integrated_tip}: <code class="break-all">{props.goal.execution.integrated_tip}</code></p>}
    <div class="flex flex-wrap gap-2">
      {props.goal.status !== "completed" && <form hx-post={`/webui/manager/${props.key}/${props.goal.status === "paused" ? "resume" : "pause"}`} hx-swap="none" hx-disable="this">
        <input type="hidden" name="goal_id" value={props.goal.id} />
        <button type="submit" class="px-3 py-1 rounded border border-gray-300 dark:border-gray-700 text-sm">{props.goal.status === "paused" ? props.t.manager.resume : props.t.manager.pause}</button>
      </form>}
      {props.goal.execution.can_retry_verification && <form hx-post={`/webui/manager/${props.key}/retry`} hx-swap="none" hx-disable="this">
        <input type="hidden" name="goal_id" value={props.goal.id} />
        <button type="submit" class="px-3 py-1 rounded border border-gray-300 dark:border-gray-700 text-sm">{props.t.manager.retry_verification}</button>
      </form>}
    </div>
    <div class="text-sm space-y-1">
      <h4 class="font-semibold">{props.t.manager.tasks}</h4>
      <ManagerTaskList tasks={props.goal.proposal.tasks} t={props.t} />
    </div>
    {props.goal.execution.attempts.length > 0 && <div class="text-sm space-y-2">
      <h4 class="font-semibold">{props.t.manager.attempts}</h4>
      <ul class="space-y-2">{props.goal.execution.attempts.map(attempt => <li id={`manager-attempt-${attempt.session_id}`} class="rounded bg-gray-100 dark:bg-gray-800 px-3 py-2 space-y-1">
        <p class="break-words"><strong>{attempt.task_id}</strong> · {props.t.manager.attempt} {attempt.attempt}</p>
        <a href={`/sessions/${attempt.session_id}`} hx-get={`/sessions/${attempt.session_id}?tab=log`} hx-target="#main" hx-swap="innerHTML" hx-push-url={`/sessions/${attempt.session_id}`} class="font-mono break-all text-blue-700 dark:text-blue-400 hover:underline">{attempt.session_id}</a>
        <p>{attempt.child_state.phase} · {attempt.integrated ? props.t.manager.integrated : props.t.manager.not_integrated}</p>
        {attempt.child_state.failure && <p class="whitespace-pre-wrap break-words text-red-700 dark:text-red-300">{attempt.child_state.failure}</p>}
        <div class="flex flex-wrap gap-2">
          {attempt.can_resume && <form hx-post={`/webui/manager/${props.key}/resume-task`} hx-swap="none" hx-disable="this">
            <input type="hidden" name="goal_id" value={props.goal.id} />
            <input type="hidden" name="task_id" value={attempt.task_id} />
            <button type="submit" class="px-3 py-1 rounded border border-gray-300 dark:border-gray-700">{props.t.manager.resume_task}</button>
          </form>}
          {attempt.can_retry && <form hx-post={`/webui/manager/${props.key}/retry`} hx-swap="none" hx-disable="this">
            <input type="hidden" name="goal_id" value={props.goal.id} />
            <input type="hidden" name="task_id" value={attempt.task_id} />
            <button type="submit" class="px-3 py-1 rounded border border-gray-300 dark:border-gray-700">{props.t.manager.retry_task}</button>
          </form>}
        </div>
      </li>)}</ul>
    </div>}
    {props.goal.execution.publication && <div class="text-sm space-y-1">
      <h4 class="font-semibold">{props.t.manager.publication}</h4>
      <p>{props.goal.execution.publication.phase} · <code class="break-all">{props.goal.execution.publication.tip}</code></p>
      {props.goal.execution.publication.request && <p class="break-all">{props.goal.execution.publication.request.state} · {props.goal.execution.publication.request.url}</p>}
      <ul class="list-disc ml-5">{props.goal.execution.publication.checks.map(check => <li class="break-words">
        <strong>{check.check_id}</strong>: {check.cancelled ? props.t.manager.check_cancelled : check.state_error ? check.state_error : check.exit_code === null ? props.t.manager.check_no_exit : `${props.t.manager.exit_code}: ${check.exit_code}`}
      </li>)}</ul>
    </div>}
    {props.goal.execution.can_preview_corrective && <form id={`manager-corrective-${props.goal.id}`} hx-post={`/webui/manager/${props.key}/preview-corrective`} hx-swap="none" hx-disable="this" class="space-y-2">
      <input type="hidden" name="goal_id" value={props.goal.id} />
      <input type="hidden" name="expected_tip" value={props.goal.execution.integrated_tip} />
      <label class="block text-sm">{props.t.manager.corrective_task_id}<input type="text" name="task_id" required class="w-full bg-transparent border border-gray-300 dark:border-gray-700 rounded px-3 py-2" /></label>
      <label class="block text-sm">{props.t.manager.corrective_instructions}<textarea name="instructions" rows="3" required class="w-full bg-transparent border border-gray-300 dark:border-gray-700 rounded px-3 py-2"></textarea></label>
      <button type="submit" class="px-3 py-1 rounded border border-gray-300 dark:border-gray-700 text-sm">{props.t.manager.preview_corrective}</button>
    </form>}
    {props.goal.execution.corrective_preview && <div class="rounded border border-gray-300 dark:border-gray-700 p-3 space-y-2">
      <h4 class="font-semibold">{props.t.manager.corrective_preview}</h4>
      <p class="text-sm font-mono break-all">{props.goal.execution.corrective_preview.task.id}</p>
      <p class="text-sm whitespace-pre-wrap break-words">{props.goal.execution.corrective_preview.task.instructions}</p>
      {props.goal.execution.can_preview_corrective && <ManagerReview t={props.t} corrective={true} dialogId={`manager-approve-corrective-${props.goal.execution.corrective_preview.id}`} url={`/webui/manager/${props.key}/approve-corrective`} goalId={props.goal.id} previewId={props.goal.execution.corrective_preview.id} revision={props.goal.execution.corrective_preview.revision} objective={props.goal.execution.corrective_preview.task.id} instructions={props.goal.execution.corrective_preview.task.instructions} target={props.goal.execution.corrective_preview.integrated_tip} />}
    </div>}
  </article>;
}
