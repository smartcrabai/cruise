import PhaseBadge from "./components/phase-badge";

export default function SessionHeader(props) {
  return (
    <header id={`session-header-${props.id}`} class="px-6 pt-6 pb-4 border-b border-gray-200 dark:border-gray-800 space-y-3">
      <div class="flex items-center gap-3">
        <h2 class="text-lg font-semibold font-mono text-gray-900 dark:text-gray-100">{props.title}</h2>
        <PhaseBadge badge={props.badge} />
      </div>
      {props.currentStep && <div class="text-sm text-gray-500 dark:text-gray-400">Step: <span class="font-medium text-gray-800 dark:text-gray-200">{props.currentStep}</span></div>}
      <p class="text-sm text-gray-700 dark:text-gray-300 whitespace-pre-wrap">{props.input}</p>
      {props.prIsLink && <a href={props.prUrl} target="_blank" rel="noopener noreferrer" class="text-blue-600 dark:text-blue-400 hover:text-blue-500">{props.prUrl}</a>}
      {props.prUrl && !props.prIsLink && <span class="text-sm text-gray-600 dark:text-gray-400">{props.prUrl}</span>}
      {props.phaseError && <div role="alert" class="rounded border border-red-300 dark:border-red-700 bg-red-50 dark:bg-red-950/30 px-3 py-2 text-sm text-red-700 dark:text-red-300">Run failed: {props.phaseError}</div>}
      {props.planError && <div role="alert" class="rounded border border-red-300 dark:border-red-700 bg-red-50 dark:bg-red-950/30 px-3 py-2 text-sm text-red-700 dark:text-red-300">Planning failed: {props.planError}</div>}
      <div class="flex gap-2 flex-wrap">
        {props.actions.showApprove && <button type="button" hx-post={`${props.baseUrl}/approve`} hx-target="#session-detail" hx-swap="none" hx-disable="this" class="px-4 py-2 bg-green-700 text-white rounded text-sm hover:bg-green-600">Approve</button>}
        {props.actions.showGeneratePlan && <button type="button" hx-post={`${props.baseUrl}/generate`} hx-target="#session-detail" hx-swap="none" hx-disable="this" class="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700">Generate Plan</button>}
        {props.actions.showFix && <button type="button" hx-get={`${props.baseUrl}/editor/fix`} hx-target={`#editor-${props.id}`} hx-swap="innerHTML" hx-disable="this" class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Fix</button>}
        {props.actions.showAsk && <button type="button" hx-get={`${props.baseUrl}/editor/ask`} hx-target={`#editor-${props.id}`} hx-swap="innerHTML" hx-disable="this" class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Ask</button>}
        {props.actions.showReplan && <button type="button" hx-get={`${props.baseUrl}/editor/replan`} hx-target={`#editor-${props.id}`} hx-swap="innerHTML" hx-disable="this" class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Replan</button>}
        {props.actions.showPublishIssue && <button type="button" hx-get={`${props.baseUrl}/editor/publish`} hx-target="#dialogs" hx-swap="beforeend" hx-disable="this" class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Publish as Issue</button>}
        {props.actions.showCreateWorktree && <button type="button" hx-post={`${props.baseUrl}/run`} hx-vals='{"workspaceMode":"Worktree"}' hx-target="#session-detail" hx-swap="none" hx-disable="this" class="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700">Create worktree (new branch)</button>}
        {props.actions.showCreateWorktree && <button type="button" hx-post={`${props.baseUrl}/run`} hx-vals='{"workspaceMode":"CurrentBranch"}' hx-target="#session-detail" hx-swap="none" hx-disable="this" class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-gray-800 dark:text-gray-200 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Run on current branch</button>}
        {props.actions.showRun && <button type="button" hx-post={`${props.baseUrl}/run`} hx-target="#session-detail" hx-swap="none" hx-disable="this" class="px-4 py-2 bg-blue-600 text-white rounded text-sm hover:bg-blue-700">{props.actions.runLabel}</button>}
        {props.actions.showReset && <button type="button" hx-post={`${props.baseUrl}/reset`} hx-target="#session-detail" hx-swap="none" hx-disable="this" hx-confirm={`Reset to Planned ${props.id}?`} class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-orange-600 dark:text-orange-400 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Reset to Planned</button>}
        {props.actions.showCancel && <button type="button" hx-post={`${props.baseUrl}/cancel`} hx-target="#session-detail" hx-swap="none" hx-disable="this" class="px-4 py-2 bg-red-600 text-white rounded text-sm hover:bg-red-700">Cancel</button>}
        {props.actions.showDiscard && <button type="button" hx-post={`${props.baseUrl}/discard`} hx-target="#main" hx-swap="innerHTML" hx-disable="this" hx-confirm={`Discard session "${props.id}"? This cannot be undone.`} class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-red-600 dark:text-red-400 rounded text-sm hover:bg-red-100/30 dark:hover:bg-red-900/30">Discard</button>}
        {props.actions.showDelete && <button type="button" hx-post={`${props.baseUrl}/delete`} hx-target="#main" hx-swap="innerHTML" hx-disable="this" hx-confirm={`Delete session "${props.id}" and its worktree? This cannot be undone.`} class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-red-600 dark:text-red-400 rounded text-sm hover:bg-red-100/30 dark:hover:bg-red-900/30">Delete</button>}
        <button type="button" hx-get={`${props.baseUrl}/settings`} hx-target={`#settings-${props.id}`} hx-swap="innerHTML" hx-disable="this" class="px-4 py-2 border border-gray-300 dark:border-gray-700 text-gray-700 dark:text-gray-300 rounded text-sm hover:bg-gray-200 dark:hover:bg-gray-800">Settings</button>
      </div>
    </header>
  );
}
