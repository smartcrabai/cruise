export default function TabPlan(props) {
  return <div id={`tab-plan-${props.id}`} role="tabpanel">
    {props.available && <div class="prose p-6">{props.html}</div>}
    {!props.available && <p class="p-4 text-xs text-gray-500 dark:text-gray-400">No plan available.</p>}
  </div>;
}
