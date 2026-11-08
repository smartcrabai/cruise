export default function TabInfo(props) {
  return <div id={`tab-info-${props.id}`} role="tabpanel" class="p-4 md:p-6 space-y-3 text-sm text-gray-500 dark:text-gray-400">
    <dl class="space-y-3">
      <div>
        <dt class="text-xs uppercase tracking-wide">{props.t.session.config}</dt>
        <dd class="break-all font-mono text-gray-700 dark:text-gray-300 mt-0.5">{props.configSource}</dd>
      </div>
      <div>
        <dt class="text-xs uppercase tracking-wide">{props.t.session.flags}</dt>
        <dd class="mt-0.5 space-y-0.5">
          {props.flags.map(flag => <div class="flex flex-wrap items-baseline gap-2">
            <span class="font-mono text-gray-700 dark:text-gray-300">{flag.name}={flag.on ? props.t.session.on : props.t.session.off}</span>
            {flag.description && <span class="text-xs">{flag.description}</span>}
          </div>)}
          {props.flagsNote && <span class="text-gray-700 dark:text-gray-300">{props.flagsNote === "None" ? props.t.session.none : props.t.session.unavailable}</span>}
        </dd>
      </div>
      <div>
        <dt class="text-xs uppercase tracking-wide">{props.locationLabel === "Repository" ? props.t.session.repository : props.t.session.base_dir}</dt>
        <dd class="break-all font-mono text-gray-700 dark:text-gray-300 mt-0.5">{props.location}</dd>
      </div>
      {props.worktreeBranch && <div>
        <dt class="text-xs uppercase tracking-wide">{props.t.session.branch}</dt>
        <dd class="break-all font-mono text-gray-700 dark:text-gray-300 mt-0.5">{props.worktreeBranch}</dd>
      </div>}
      <div>
        <dt class="text-xs uppercase tracking-wide">{props.t.session.created}</dt>
        <dd class="text-gray-700 dark:text-gray-300 mt-0.5">{props.createdAt}</dd>
      </div>
      {props.completedAt && <div>
        <dt class="text-xs uppercase tracking-wide">{props.t.session.completed}</dt>
        <dd class="text-gray-700 dark:text-gray-300 mt-0.5">{props.completedAt}</dd>
      </div>}
      {props.prUrl && <div>
        <dt class="text-xs uppercase tracking-wide">{props.t.session.pull_request}</dt>
        <dd>
          {props.prIsLink && <a href={props.prUrl} target="_blank" rel="noopener noreferrer" class="break-all text-blue-600 dark:text-blue-400 hover:text-blue-500 dark:hover:text-blue-300">{props.prUrl}</a>}
          {props.prUrl && !props.prIsLink && <span class="text-gray-800 dark:text-gray-200">{props.prUrl}</span>}
        </dd>
      </div>}
      {props.phaseError && <div>
        <dt class="text-xs uppercase tracking-wide">{props.t.session.error}</dt>
        <dd class="break-words text-red-600 dark:text-red-400 mt-0.5 font-mono text-xs">{props.phaseError}</dd>
      </div>}
      {props.planError && props.planError !== props.phaseError && <div>
        <dt class="text-xs uppercase tracking-wide">{props.t.session.planning_error}</dt>
        <dd class="break-words text-red-600 dark:text-red-400 mt-0.5 font-mono text-xs">{props.planError}</dd>
      </div>}
    </dl>
  </div>;
}
