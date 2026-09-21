export default function TabInfo(props) {
  return <div id={`tab-info-${props.id}`} role="tabpanel" class="p-6 space-y-3 text-sm text-gray-500 dark:text-gray-400">
    <dl class="space-y-3">
      <div>
        <dt class="text-xs uppercase tracking-wide">Config</dt>
        <dd class="font-mono text-gray-700 dark:text-gray-300 mt-0.5">{props.configSource}</dd>
      </div>
      <div>
        <dt class="text-xs uppercase tracking-wide">{props.locationLabel}</dt>
        <dd class="font-mono text-gray-700 dark:text-gray-300 mt-0.5">{props.location}</dd>
      </div>
      {props.worktreeBranch && <div>
        <dt class="text-xs uppercase tracking-wide">Branch</dt>
        <dd class="font-mono text-gray-700 dark:text-gray-300 mt-0.5">{props.worktreeBranch}</dd>
      </div>}
      <div>
        <dt class="text-xs uppercase tracking-wide">Created</dt>
        <dd class="text-gray-700 dark:text-gray-300 mt-0.5">{props.createdAt}</dd>
      </div>
      {props.completedAt && <div>
        <dt class="text-xs uppercase tracking-wide">Completed</dt>
        <dd class="text-gray-700 dark:text-gray-300 mt-0.5">{props.completedAt}</dd>
      </div>}
      {props.prUrl && <div>
        <dt class="text-xs uppercase tracking-wide">Pull Request</dt>
        <dd>
          {props.prIsLink && <a href={props.prUrl} target="_blank" rel="noopener noreferrer" class="text-blue-600 dark:text-blue-400 hover:text-blue-500 dark:hover:text-blue-300">{props.prUrl}</a>}
          {props.prUrl && !props.prIsLink && <span class="text-gray-800 dark:text-gray-200">{props.prUrl}</span>}
        </dd>
      </div>}
      {props.phaseError && <div>
        <dt class="text-xs uppercase tracking-wide">Error</dt>
        <dd class="text-red-600 dark:text-red-400 mt-0.5 font-mono text-xs">{props.phaseError}</dd>
      </div>}
      {props.planError && props.planError !== props.phaseError && <div>
        <dt class="text-xs uppercase tracking-wide">Planning Error</dt>
        <dd class="text-red-600 dark:text-red-400 mt-0.5 font-mono text-xs">{props.planError}</dd>
      </div>}
    </dl>
  </div>;
}
