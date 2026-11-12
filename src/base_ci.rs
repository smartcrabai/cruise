//! Pre-run check that the base branch CI is not failing.

use crate::error::{CruiseError, Result};
use crate::forge::{ForgeContext, ForgeKind};
use crate::session::{SessionExecutionStage, SessionState};
use crate::workflow_defaults::WorkflowDefaults;
use console::style;
use serde_json::Value;
use std::process::{Command, Output, Stdio};

const GITHUB_DEFAULT_QUERY: &str = "query($owner:String!,$repo:String!){repository(owner:$owner,name:$repo){defaultBranchRef{name target{oid ... on Commit{statusCheckRollup{state}}}}}}";
const GITHUB_REF_QUERY: &str = "query($owner:String!,$repo:String!,$ref:String!){repository(owner:$owner,name:$repo){ref(qualifiedName:$ref){name target{oid ... on Commit{statusCheckRollup{state}}}}}}";

#[derive(Debug)]
enum CiState {
    Passing,
    Pending,
    NoChecks,
    Failing,
}

struct BaseCi {
    branch: String,
    sha: String,
    state: CiState,
}

/// Reject a fresh workflow start while the base branch CI is failing.
pub(crate) fn ensure_base_ci_not_failing(session: &SessionState) -> Result<()> {
    ensure_base_ci_not_failing_with(session, |program, args| {
        Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
    })
}

pub(crate) fn ensure_base_ci_not_failing_with<F>(
    session: &SessionState,
    mut runner: F,
) -> Result<()>
where
    F: FnMut(&str, &[&str]) -> std::io::Result<Output>,
{
    if session.exec
        || session.current_step.is_some()
        || session.execution_stage == SessionExecutionStage::AfterPr
    {
        return Ok(());
    }
    let defaults = WorkflowDefaults::load(session.workflow_defaults_dir())?;
    if !defaults.check_base_ci() {
        return Ok(());
    }
    let context = match crate::forge::repository_context(session.repo.as_deref(), &session.base_dir)
    {
        Ok(context) => context,
        Err(error) => {
            crate::status_eprintln!("{} skipping base CI check: {error}", style("->").cyan());
            return Ok(());
        }
    };
    let configured = defaults.worktree_base().branch;
    let branch_label = configured.as_deref().unwrap_or("default branch");
    let result = match context.kind {
        ForgeKind::GitHub => github_ci(&context, configured.as_deref(), &mut runner),
        ForgeKind::GitLab => gitlab_ci(&context, configured.as_deref(), &mut runner),
    };
    let ci = result.map_err(|detail| CruiseError::BaseCiFailed(format!(
        "could not verify CI on base branch `{branch_label}` in {}: {detail}; set `check_base_ci: false` in default.yaml to skip this check", context.repository
    )))?;
    let short_sha: String = ci.sha.chars().take(7).collect();
    match ci.state {
        CiState::Failing => Err(CruiseError::BaseCiFailed(format!(
            "CI on base branch `{}` ({short_sha}) is failing in {}; fix it before starting, or set `check_base_ci: false` in default.yaml to skip this check",
            ci.branch, context.repository
        ))),
        CiState::Passing => {
            crate::status_eprintln!(
                "{} base CI: {} @ {short_sha} passing",
                style("->").cyan(),
                ci.branch
            );
            Ok(())
        }
        CiState::Pending | CiState::NoChecks => {
            let reason = if matches!(ci.state, CiState::Pending) {
                "pending"
            } else {
                "no checks"
            };
            crate::status_eprintln!(
                "{} warning: base CI: {} @ {short_sha} {reason}; continuing",
                style("->").yellow(),
                ci.branch
            );
            Ok(())
        }
    }
}

fn command_json<F>(
    runner: &mut F,
    program: &str,
    args: &[&str],
) -> std::result::Result<Value, String>
where
    F: FnMut(&str, &[&str]) -> std::io::Result<Output>,
{
    let output = runner(program, args).map_err(|e| format!("could not run {program}: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "{program} failed ({}): {}",
            output.status,
            stderr.trim()
        ));
    }
    serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("invalid {program} JSON response: {e}"))
}

fn required_string<'a>(value: &'a Value, key: &str) -> std::result::Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing or invalid {key} in API response"))
}

fn github_ci<F>(
    context: &ForgeContext,
    branch: Option<&str>,
    runner: &mut F,
) -> std::result::Result<BaseCi, String>
where
    F: FnMut(&str, &[&str]) -> std::io::Result<Output>,
{
    let (owner, repo) = context
        .repository
        .split_once('/')
        .ok_or_else(|| "invalid GitHub repository path".to_string())?;
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return Err("invalid GitHub repository path".to_string());
    }
    let query = format!(
        "query={}",
        if branch.is_some() {
            GITHUB_REF_QUERY
        } else {
            GITHUB_DEFAULT_QUERY
        }
    );
    let owner_arg = format!("owner={owner}");
    let repo_arg = format!("repo={repo}");
    let ref_arg = branch.map(|b| format!("ref=refs/heads/{b}"));
    let mut args = vec![
        "api",
        "--hostname",
        context.host.as_str(),
        "graphql",
        "-f",
        query.as_str(),
        "-f",
        owner_arg.as_str(),
        "-f",
        repo_arg.as_str(),
    ];
    if let Some(ref_arg) = &ref_arg {
        args.extend(["-f", ref_arg.as_str()]);
    }
    let response = command_json(runner, "gh", &args)?;
    if let Some(errors) = response.get("errors") {
        match errors.as_array() {
            Some(errors) if errors.is_empty() => {}
            _ => return Err(format!("GraphQL errors: {errors}")),
        }
    }
    let field = if branch.is_some() {
        "ref"
    } else {
        "defaultBranchRef"
    };
    let reference = response
        .pointer(&format!("/data/repository/{field}"))
        .filter(|v| v.is_object())
        .ok_or_else(|| {
            format!(
                "branch `{}` not found or invalid response",
                branch.unwrap_or("default branch")
            )
        })?;
    let name = required_string(reference, "name")?.to_owned();
    let target = reference
        .get("target")
        .filter(|v| v.is_object())
        .ok_or_else(|| "missing commit target".to_string())?;
    let sha = required_string(target, "oid")?.to_owned();
    let state = match target.get("statusCheckRollup") {
        Some(Value::Null) => CiState::NoChecks,
        Some(rollup) => match required_string(rollup, "state")? {
            "SUCCESS" => CiState::Passing,
            "PENDING" | "EXPECTED" => CiState::Pending,
            "FAILURE" | "ERROR" => CiState::Failing,
            state => return Err(format!("unknown GitHub CI state `{state}`")),
        },
        None => return Err("missing statusCheckRollup in API response".to_string()),
    };
    Ok(BaseCi {
        branch: name,
        sha,
        state,
    })
}

fn gitlab_ci<F>(
    context: &ForgeContext,
    branch: Option<&str>,
    runner: &mut F,
) -> std::result::Result<BaseCi, String>
where
    F: FnMut(&str, &[&str]) -> std::io::Result<Output>,
{
    use crate::pr_merge::automatic::url_encode_path_segment;
    let project = url_encode_path_segment(&context.repository);
    let project_path = format!("projects/{project}");
    let branch = if let Some(branch) = branch {
        branch.to_owned()
    } else {
        let project = command_json(
            runner,
            "glab",
            &["api", "--hostname", &context.host, &project_path],
        )?;
        required_string(&project, "default_branch")?.to_owned()
    };
    let encoded_branch = url_encode_path_segment(&branch);
    let path = format!("{project_path}/repository/commits/{encoded_branch}");
    let commit = command_json(runner, "glab", &["api", "--hostname", &context.host, &path])
        .map_err(|e| format!("branch `{branch}`: {e}"))?;
    let sha = required_string(&commit, "id")
        .map_err(|e| format!("branch `{branch}`: {e}"))?
        .to_owned();
    let pipelines_path = format!("{project_path}/pipelines?ref={encoded_branch}&sha={sha}&order_by=id&sort=desc&per_page=1");
    let pipelines = command_json(runner, "glab", &["api", "--hostname", &context.host, &pipelines_path])
        .map_err(|e| format!("branch `{branch}`: {e}"))?;
    let pipelines = pipelines.as_array().ok_or_else(|| format!("branch `{branch}`: invalid pipelines response"))?;
    let state = match pipelines.first() {
        None => CiState::NoChecks,
        Some(pipeline) => match required_string(pipeline, "status")
            .map_err(|e| format!("branch `{branch}`: {e}"))?
        {
            "success" | "skipped" => CiState::Passing,
            "running"
            | "pending"
            | "created"
            | "waiting_for_resource"
            | "preparing"
            | "scheduled"
            | "manual" => CiState::Pending,
            "failed" | "canceled" | "cancelled" | "canceling" => CiState::Failing,
            status => {
                return Err(format!(
                    "branch `{branch}`: unknown GitLab CI status `{status}`"
                ));
            }
        },
    };
    Ok(BaseCi { branch, sha, state })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::session::SessionExecutionStage;
    use crate::test_support::{EnvGuard, lock_process, set_fake_home};
    use std::collections::VecDeque;
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;

    struct Fixture {
        _guards: Vec<EnvGuard>,
        _lock: crate::test_support::ProcessLock,
        _home: tempfile::TempDir,
        project: tempfile::TempDir,
        session: SessionState,
    }

    fn fixture(repo: Option<&str>, defaults: Option<&str>) -> Fixture {
        let lock = lock_process();
        let home = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let project = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let mut guards = set_fake_home(home.path());
        guards.extend(crate::test_support::clear_config_override_envs());
        guards.push(EnvGuard::remove("CRUISE_FORGE"));
        guards.push(EnvGuard::remove("GITLAB_HOST"));
        let mut session = SessionState::new(
            "test".into(),
            project.path().to_path_buf(),
            crate::session_config::SessionConfigRef::File {
                path: PathBuf::from("cruise.yaml"),
            },
            "test".into(),
        );
        if let Some(repo) = repo {
            // A local git config provides an origin without invoking a network operation.
            std::fs::create_dir(project.path().join(".git")).unwrap_or_else(|e| panic!("{e}"));
            std::fs::write(project.path().join(".git/config"), format!("[core]\n repositoryformatversion = 0\n bare = false\n[remote \"origin\"]\n url = {repo}\n")).unwrap_or_else(|e| panic!("{e}"));
            std::fs::write(project.path().join(".git/HEAD"), "ref: refs/heads/main\n")
                .unwrap_or_else(|e| panic!("{e}"));
            std::fs::create_dir_all(project.path().join(".git/objects"))
                .unwrap_or_else(|e| panic!("{e}"));
            std::fs::create_dir_all(project.path().join(".git/refs"))
                .unwrap_or_else(|e| panic!("{e}"));
        }
        if let Some(defaults) = defaults {
            std::fs::create_dir(project.path().join(".cruise")).unwrap_or_else(|e| panic!("{e}"));
            std::fs::write(project.path().join(".cruise/default.yaml"), defaults)
                .unwrap_or_else(|e| panic!("{e}"));
        }
        session.current_step = None;
        Fixture {
            _guards: guards,
            _lock: lock,
            _home: home,
            project,
            session,
        }
    }

    fn output(body: &str, code: i32) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: body.as_bytes().to_vec(),
            stderr: b"command failed".to_vec(),
        }
    }

    fn run(
        session: &SessionState,
        responses: Vec<std::io::Result<Output>>,
    ) -> (Result<()>, Vec<(String, Vec<String>)>) {
        let mut responses = VecDeque::from(responses);
        let mut calls = Vec::new();
        let result = ensure_base_ci_not_failing_with(session, |command, args| {
            calls.push((
                command.to_string(),
                args.iter().map(ToString::to_string).collect(),
            ));
            responses
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected command: {command} {args:?}"))
        });
        assert!(
            responses.is_empty(),
            "not all scripted responses were consumed"
        );
        (result, calls)
    }

    fn github(state: Option<&str>, reference: &str) -> String {
        let rollup = state.map_or(serde_json::Value::Null, |s| serde_json::json!({"state": s}));
        let mut repository = serde_json::Map::new();
        repository.insert(reference.to_string(), serde_json::json!({"name":"main", "target":{"oid":"abc123456789", "statusCheckRollup":rollup}}));
        serde_json::json!({"data":{"repository":repository}}).to_string()
    }

    #[test]
    fn github_default_branch_failure_and_query_contract() {
        let f = fixture(Some("https://github.com/owner/repo.git"), None);
        let (result, calls) = run(
            &f.session,
            vec![Ok(output(&github(Some("FAILURE"), "defaultBranchRef"), 0))],
        );
        let Err(error) = result else {
            panic!("failing base CI must be rejected");
        };
        assert_eq!(error.telemetry_type(), "base_ci_failed");
        let message = error.to_string();
        assert!(
            message.contains("main") && message.contains("check_base_ci: false"),
            "{message}"
        );
        assert_eq!(calls[0].0, "gh");
        assert!(calls[0].1.iter().any(|s| s.contains("defaultBranchRef")));
        assert!(calls[0].1.iter().any(|s| s == "github.com"));
    }

    #[test]
    fn github_rollup_states_and_invalid_responses() {
        let f = fixture(Some("https://github.com/owner/repo.git"), None);
        for state in [Some("SUCCESS"), Some("PENDING"), Some("EXPECTED"), None] {
            assert!(
                run(
                    &f.session,
                    vec![Ok(output(&github(state, "defaultBranchRef"), 0))]
                )
                .0
                .is_ok(),
                "{state:?}"
            );
        }
        for response in [
            Ok(output(&github(Some("ERROR"), "defaultBranchRef"), 0)),
            Ok(output(&github(Some("SURPRISE"), "defaultBranchRef"), 0)),
            Ok(output("not json", 0)),
            Ok(output(r#"{"errors":[{"message":"denied"}]}"#, 0)),
            Ok(output("{}", 1)),
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "missing gh",
            )),
        ] {
            assert!(run(&f.session, vec![response]).0.is_err());
        }
    }

    #[test]
    fn github_configured_ref_and_missing_ref() {
        let f = fixture(
            Some("https://github.com/owner/repo.git"),
            Some("base_branch: develop\n"),
        );
        let (result, calls) = run(
            &f.session,
            vec![Ok(output(&github(Some("SUCCESS"), "ref"), 0))],
        );
        assert!(result.is_ok());
        assert!(calls[0].1.iter().any(|arg| arg == "ref=refs/heads/develop"));
        let (result, _) = run(
            &f.session,
            vec![Ok(output(r#"{"data":{"repository":{"ref":null}}}"#, 0))],
        );
        assert!(result.is_err());
    }

    #[test]
    fn gitlab_default_branch_encoding_and_statuses() {
        let f = fixture(Some("https://gitlab.com/group/sub/project.git"), None);
        for (status, succeeds) in [
            (Some("success"), true),
            (Some("skipped"), true),
            (Some("running"), true),
            (Some("manual"), true),
            (None, true),
            (Some("failed"), false),
            (Some("canceled"), false),
            (Some("cancelled"), false),
            (Some("canceling"), false),
            (Some("unexpected"), false),
        ] {
            let pipeline =
                status.map_or_else(|| "[]".to_string(), |s| format!(r#"[{{"status":"{s}"}}]"#));
            let (result, calls) = run(
                &f.session,
                vec![
                    Ok(output(r#"{"default_branch":"feature/x"}"#, 0)),
                    Ok(output(
                        r#"{"id":"abc123456789"}"#,
                        0,
                    )),
                    Ok(output(&pipeline, 0)),
                ],
            );
            assert_eq!(result.is_ok(), succeeds, "{status:?}");
            assert_eq!(calls[0].0, "glab");
            assert!(
                calls[0]
                    .1
                    .iter()
                    .any(|s| s == "projects/group%2Fsub%2Fproject")
            );
            assert!(
                calls[1]
                    .1
                    .iter()
                    .any(|s| s == "projects/group%2Fsub%2Fproject/repository/commits/feature%2Fx")
            );
        }
    }

    #[test]
    fn gitlab_shared_sha_is_scoped_to_selected_ref() {
        let f = fixture(Some("https://gitlab.com/group/sub/project.git"), None);
        for (selected, other, expected) in [("success", "failed", true), ("failed", "success", false)] {
            let (result, calls) = run(&f.session, vec![
                Ok(output(r#"{"default_branch":"feature/é"}"#, 0)),
                Ok(output(&format!(r#"{{"id":"abc123456789","last_pipeline":{{"status":"{other}"}}}}"#), 0)),
                Ok(output(&format!(r#"[{{"status":"{selected}"}}]"#), 0)),
            ]);
            assert_eq!(result.is_ok(), expected);
            assert!(calls[2].1.iter().any(|s| s.contains("pipelines?ref=feature%2F%C3%A9&sha=abc123456789")));
        }
        let (result, _) = run(&f.session, vec![
            Ok(output(r#"{"default_branch":"main"}"#, 0)),
            Ok(output(r#"{"id":"abc123456789"}"#, 0)),
            Ok(output("unavailable", 1)),
        ]);
        assert!(matches!(result, Err(CruiseError::BaseCiFailed(_))));
    }

    #[test]
    fn skipped_cases_never_invoke_runner() {
        let mut f = fixture(
            Some("https://github.com/owner/repo.git"),
            Some("check_base_ci: false\n"),
        );
        let no_calls = |_: &str, _: &[&str]| -> std::io::Result<Output> {
            panic!("runner must not be invoked")
        };
        assert!(ensure_base_ci_not_failing_with(&f.session, no_calls).is_ok());
        std::fs::remove_file(f.project.path().join(".cruise/default.yaml"))
            .unwrap_or_else(|e| panic!("{e}"));
        f.session.exec = true;
        assert!(ensure_base_ci_not_failing_with(&f.session, no_calls).is_ok());
        f.session.exec = false;
        f.session.current_step = Some("step".into());
        assert!(ensure_base_ci_not_failing_with(&f.session, no_calls).is_ok());
        f.session.current_step = None;
        f.session.execution_stage = SessionExecutionStage::AfterPr;
        assert!(ensure_base_ci_not_failing_with(&f.session, no_calls).is_ok());
        f.session.execution_stage = SessionExecutionStage::Main;
        std::fs::remove_dir_all(f.project.path().join(".git")).unwrap_or_else(|e| panic!("{e}"));
        assert!(ensure_base_ci_not_failing_with(&f.session, no_calls).is_ok());
    }
}
