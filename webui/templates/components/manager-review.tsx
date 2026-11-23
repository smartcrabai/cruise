export default function ManagerReview(props) {
  return <div>
    <button type="button" aria-haspopup="dialog" data-manager-review={props.dialogId} class="px-4 py-2 rounded bg-green-700 hover:bg-green-600 text-white text-sm">{props.corrective ? props.t.manager.approve_corrective : props.t.manager.approve}</button>
    <template id={props.dialogId}>
      <dialog aria-labelledby={`${props.dialogId}-title`} class="m-auto w-[calc(100%-2rem)] max-w-lg max-h-[85vh] overflow-y-auto rounded-lg shadow-xl border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-900 text-gray-900 dark:text-gray-100 p-5 backdrop:bg-black/60">
        <form hx-post={props.url} hx-swap="none" hx-disable="this" class="space-y-3" data-manager-confirm="true">
          <h3 id={`${props.dialogId}-title`} class="text-lg font-semibold">{props.corrective ? props.t.manager.review_corrective : props.t.manager.review_proposal}</h3>
          <p class="whitespace-pre-wrap break-words">{props.objective}</p>
          {props.corrective && <p class="whitespace-pre-wrap break-words">{props.instructions}</p>}
          <p class="text-sm">{`${props.t.manager.revision}: ${props.revision}`}</p>
          <p class="text-sm"><span>{props.t.manager.target}: </span><code class="break-all">{props.target}</code></p>
          {props.corrective ? <>
            <input type="hidden" name="goal_id" value={props.goalId} />
            <input type="hidden" name="preview_id" value={props.previewId} />
          </> : <input type="hidden" name="proposal_id" value={props.proposalId} />}
          <input type="hidden" name="revision" value={props.revision} />
          <p class="text-sm">{props.t.manager.approval_explanation}</p>
          <div class="flex flex-wrap justify-end gap-2">
            <button type="button" autofocus data-manager-close="true" class="px-4 py-2 rounded border border-gray-300 dark:border-gray-700 text-sm">{props.t.common.cancel}</button>
            <button type="submit" class="px-4 py-2 rounded bg-green-700 hover:bg-green-600 text-white text-sm">{props.t.manager.confirm_approval}</button>
          </div>
        </form>
      </dialog>
    </template>
  </div>;
}
