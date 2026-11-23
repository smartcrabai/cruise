export default function ManagerTaskList(props) {
  return <ul class="list-disc ml-5">{props.tasks.map(task => <li class="break-words">
    <strong>{task.id}</strong><p class="whitespace-pre-wrap">{task.instructions}</p>
    {task.dependencies.length > 0 && <p>{props.t.manager.dependencies}: {task.dependencies.map(id => <code class="mr-2">{id}</code>)}</p>}
  </li>)}</ul>;
}
