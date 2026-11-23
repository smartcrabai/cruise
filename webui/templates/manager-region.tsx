import ManagerReview from "./components/manager-review";
import ManagerGoal from "./components/manager-goal";
import ManagerPrompt from "./components/manager-prompt";
import ManagerTaskList from "./components/manager-task-list";

export default function ManagerRegion(props) {
  return <div class="space-y-6">
    <section aria-labelledby="manager-conversation-title" class="space-y-3">
      <h2 id="manager-conversation-title" class="text-lg font-semibold">{props.t.manager.conversation}</h2>
      {!props.snapshot.transcript.length && <p class="text-sm text-gray-500 dark:text-gray-400">{props.t.manager.no_messages}</p>}
      {props.snapshot.transcript.length > 0 && <ol class="space-y-2">
        {props.snapshot.transcript.map(turn => <li id={`manager-turn-${turn.id}`} class="p-3 border border-gray-300 dark:border-gray-700 rounded">
          <div class="text-sm font-semibold">{turn.role === "user" ? props.t.manager.user : turn.role === "assistant" ? props.t.manager.assistant : turn.role}</div>
          <p class="whitespace-pre-wrap break-words">{turn.text}</p>
          {turn.status === "running" && <p class="text-sm">{props.t.manager.running}</p>}
          {turn.status === "interrupted" && <p class="text-sm">{props.t.manager.interrupted}</p>}
          {turn.status === "completed" && <p class="text-sm text-gray-500 dark:text-gray-400">{props.t.manager.completed}</p>}
        </li>)}
      </ol>}
      <form data-manager-reset-composer="true" hx-post={`/webui/manager/${props.key}/message`} hx-swap="none" hx-disable="this" class="space-y-2">
        <label for="manager-message" class="block text-sm">{props.t.manager.message}</label>
        <textarea id="manager-message" name="text" rows="3" required class="w-full bg-transparent border border-gray-300 dark:border-gray-700 rounded px-3 py-2"></textarea>
        <button type="submit" disabled={props.snapshot.running} class="px-4 py-2 rounded bg-blue-600 text-white text-sm disabled:opacity-50">{props.t.manager.send}</button>
      </form>
      <form hx-post={`/webui/manager/${props.key}/propose`} hx-swap="none" hx-disable="this">
        <button type="submit" disabled={props.snapshot.running} class="px-4 py-2 rounded border border-gray-300 dark:border-gray-700 text-sm disabled:opacity-50">{props.t.manager.make_proposal}</button>
      </form>
    </section>
    <section aria-labelledby="manager-proposal-title" class="space-y-3 border-t border-gray-300 dark:border-gray-700 pt-4">
      <h2 id="manager-proposal-title" class="text-lg font-semibold">{props.t.manager.proposal}</h2>
      {!props.snapshot.proposal && <p class="text-sm text-gray-500 dark:text-gray-400">{props.t.manager.no_proposal}</p>}
      {props.snapshot.proposal && <div class="space-y-3">
        <p class="font-semibold whitespace-pre-wrap break-words">{props.snapshot.proposal.objective}</p>
        <dl class="text-sm space-y-1">
          <div><dt class="inline font-semibold">{props.t.manager.revision}: </dt><dd class="inline">{props.snapshot.proposal.revision}</dd></div>
          <div><dt class="inline font-semibold">{props.t.manager.target}: </dt><dd class="inline font-mono break-all">{props.snapshot.proposal.target_branch} @ {props.snapshot.proposal.target_sha}</dd></div>
          <div><dt class="inline font-semibold">{props.t.manager.workflow}: </dt><dd class="inline font-mono break-all">{props.snapshot.proposal.workflow}</dd></div>
        </dl>
        {props.snapshot.proposal.contract && <div class="text-sm rounded border border-gray-300 dark:border-gray-700 p-3 space-y-1">
          <h3 class="font-semibold">{props.t.manager.frozen_contract}</h3>
          <p class="font-mono break-all">{props.snapshot.proposal.contract.anchor}</p>
          <p class="font-mono break-all">{props.snapshot.proposal.contract.forge_kind}: {props.snapshot.proposal.contract.forge_host}/{props.snapshot.proposal.contract.forge_repository}</p>
        </div>}
        <div class="text-sm space-y-1">
          <h3 class="font-semibold">{props.t.manager.criteria}</h3>
          <ul class="list-disc ml-5">{props.snapshot.proposal.criteria.map(criterion => <li class="whitespace-pre-wrap break-words"><code>{criterion.id}</code>: {criterion.description}</li>)}</ul>
        </div>
        <div class="text-sm space-y-1">
          <h3 class="font-semibold">{props.t.manager.checks}</h3>
          <ul class="list-disc ml-5">{props.snapshot.proposal.checks.map(check => <li class="break-words">
            <strong>{check.id}</strong>
            <ul class="flex flex-wrap gap-1" aria-label={props.t.manager.arguments}>{check.argv.map(arg => <li><code class="whitespace-pre-wrap break-all bg-gray-100 dark:bg-gray-800 px-1">{arg === "" ? '""' : arg}</code></li>)}</ul>
            <p>{props.t.manager.criteria}: {check.criterion_ids.map(id => <code class="mr-2">{id}</code>)}</p>
          </li>)}</ul>
        </div>
        <div class="text-sm space-y-1">
          <h3 class="font-semibold">{props.t.manager.tasks}</h3>
          <ManagerTaskList tasks={props.snapshot.proposal.tasks} t={props.t} />
        </div>
        {props.snapshot.proposal.exclusions.length > 0 && <div class="text-sm space-y-1">
          <h3 class="font-semibold">{props.t.manager.exclusions}</h3>
          <ul class="list-disc ml-5">{props.snapshot.proposal.exclusions.map(exclusion => <li class="whitespace-pre-wrap break-words">{exclusion}</li>)}</ul>
        </div>}
        <ManagerReview t={props.t} corrective={false} dialogId={`manager-approve-${props.snapshot.proposal.id}-${props.snapshot.proposal.revision}`} url={`/webui/manager/${props.key}/approve`} proposalId={props.snapshot.proposal.id} revision={props.snapshot.proposal.revision} objective={props.snapshot.proposal.objective} target={props.snapshot.proposal.target_sha} />
      </div>}
    </section>
    <section aria-labelledby="manager-goals-title" class="space-y-3 border-t border-gray-300 dark:border-gray-700 pt-4">
      <h2 id="manager-goals-title" class="text-lg font-semibold">{props.t.manager.goals}</h2>
      {!props.snapshot.goals.length && <p class="text-sm text-gray-500 dark:text-gray-400">{props.t.manager.no_goals}</p>}
      {props.snapshot.goals.map(goal => <ManagerGoal key={props.key} goal={goal} t={props.t} />)}
    </section>
    {props.snapshot.prompts.length > 0 && <section aria-labelledby="manager-prompts-title" class="space-y-3 border-t border-gray-300 dark:border-gray-700 pt-4">
      <h2 id="manager-prompts-title" class="text-lg font-semibold">{props.t.manager.prompts}</h2>
      {props.snapshot.prompts.map(entry => <ManagerPrompt key={props.key} goalId={entry.goal_id} prompt={entry.prompt} t={props.t} />)}
    </section>}
  </div>;
}
