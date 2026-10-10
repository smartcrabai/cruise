export default function SessionSync(props) {
  return <div id={`session-sync-${props.id}`} class="hidden" hx-get={props.url} hx-trigger="every 3s" hx-swap="none"></div>;
}
