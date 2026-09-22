import SessionRow from "./components/session-row";

export default function SidebarRows(props) {
  return (
    <>
      {props.empty && <p class="px-4 py-3 text-xs text-gray-500 dark:text-gray-400">No sessions found.</p>}
      {props.sessions.map(session => <SessionRow row={session} />)}
    </>
  );
}
