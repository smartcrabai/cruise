export default function RepoOptions(props) {
  return <>{props.repos.map(repo => <option value={repo}></option>)}</>;
}
