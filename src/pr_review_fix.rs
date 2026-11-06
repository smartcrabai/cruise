//! Resolve and validate existing pull requests for review-fix sessions.
use crate::error::{CruiseError, Result};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

pub const BUILTIN_SELECTOR: &str = "builtin:pr-review-fix";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrTarget {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub head_branch: String,
    pub base_branch: String,
}

/// Check both effective origin URLs against the repository in the resolved PR URL.
/// `git remote get-url` expands Git's URL rewrite rules, including push rewrites.
pub(crate) fn validate_origin(target: &PrTarget, dir: &std::path::Path) -> Result<()> {
    let url = url::Url::parse(&target.url)
        .map_err(|_| CruiseError::Other("invalid resolved pull request URL".into()))?;
    let segments: Vec<_> = url.path_segments().into_iter().flatten().collect();
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || segments.len() != 4
        || segments[2] != "pull"
        || segments[3].parse::<u64>().ok() != Some(target.number)
    {
        return Err(CruiseError::Other(
            "invalid resolved pull request URL".into(),
        ));
    }
    let expected_host = url.host_str().unwrap_or_default();
    let expected_repo = format!("{}/{}", segments[0], segments[1]);
    for push in [false, true] {
        let mut command = std::process::Command::new("git");
        command.args(["remote", "get-url", "--all"]);
        if push {
            command.arg("--push");
        }
        let output = command
            .arg("origin")
            .current_dir(dir)
            .output()
            .map_err(|e| CruiseError::Other(format!("failed to inspect origin: {e}")))?;
        if !output.status.success() {
            return Err(CruiseError::Other(format!(
                "failed to inspect origin: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let urls = std::str::from_utf8(&output.stdout)
            .map_err(|_| CruiseError::Other("invalid origin URL".into()))?;
        if urls.lines().next().is_none() {
            return Err(CruiseError::Other("missing origin URL".into()));
        }
        for remote in urls.lines() {
            // The forge parser normalizes hosts without ports, so do not silently
            // equate a nonstandard endpoint with the PR's HTTPS host.
            if remote.contains("://")
                && url::Url::parse(remote).map_or(true, |remote| remote.port().is_some())
            {
                return Err(CruiseError::Other(
                    "cannot verify origin repository against resolved pull request".into(),
                ));
            }
            let identity =
                crate::forge::resolve_remote(remote, Some(crate::forge::ForgeKind::GitHub))
                    .map_err(|_| {
                        CruiseError::Other(
                            "cannot verify origin repository against resolved pull request".into(),
                        )
                    })?;
            if !identity.host.eq_ignore_ascii_case(expected_host)
                || !identity.repository.eq_ignore_ascii_case(&expected_repo)
            {
                return Err(CruiseError::Other(format!(
                    "origin {} repository does not match resolved pull request repository",
                    if push { "push" } else { "fetch" }
                )));
            }
        }
    }
    Ok(())
}

/// Parse and validate a `gh pr view` response.
///
/// # Errors
/// Returns an error for invalid output, a mismatched PR, or an unsupported PR.
pub fn parse_pr_view(json: &[u8], requested: u64) -> Result<PrTarget> {
    let v: serde_json::Value = serde_json::from_slice(json)
        .map_err(|e| CruiseError::Other(format!("invalid gh pr view output: {e}")))?;
    let text = |key: &str| v.get(key).and_then(serde_json::Value::as_str).unwrap_or("");
    if !text("state").eq_ignore_ascii_case("OPEN") {
        return Err(CruiseError::Other(
            "only open pull requests are supported".into(),
        ));
    }
    match v
        .get("isCrossRepository")
        .and_then(serde_json::Value::as_bool)
    {
        Some(false) => {}
        Some(true) => {
            return Err(CruiseError::Other(
                "fork pull requests are not supported".into(),
            ));
        }
        None => {
            return Err(CruiseError::Other(
                "invalid gh pr view output: missing isCrossRepository".into(),
            ));
        }
    }
    let number = v
        .get("number")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if number == 0
        || number != requested
        || text("headRefName").trim().is_empty()
        || text("url").is_empty()
        || text("baseRefName").is_empty()
    {
        return Err(CruiseError::Other(
            "invalid gh pr view output: missing or mismatched pull request fields".into(),
        ));
    }
    Ok(PrTarget {
        number,
        title: text("title").to_string(),
        url: text("url").to_string(),
        head_branch: text("headRefName").to_string(),
        base_branch: text("baseRefName").to_string(),
    })
}

/// Resolve an existing GitHub pull request.
///
/// # Errors
/// Returns an error if the repository or PR is unsupported or resolution fails.
pub fn resolve_pr_target(number: u64, repo: Option<&str>) -> Result<PrTarget> {
    resolve_pr_target_in_dir(number, repo, None)
}

/// Resolve an existing GitHub pull request from an optional working directory.
///
/// # Errors
/// Returns an error if the repository or PR is unsupported or resolution fails.
pub fn resolve_pr_target_in_dir(
    number: u64,
    repo: Option<&str>,
    dir: Option<&std::path::Path>,
) -> Result<PrTarget> {
    if number == 0 {
        return Err(CruiseError::Other("PR number must be positive".into()));
    }
    if let Some(repo) = repo {
        let forge = crate::forge::resolve_repo_locator_env(repo)?;
        if forge.kind != crate::forge::ForgeKind::GitHub {
            return Err(CruiseError::Other(
                "PR review fix only supports GitHub, not GitLab".into(),
            ));
        }
    } else if let Ok(remote) = {
        let mut git = std::process::Command::new("git");
        git.args(["remote", "get-url", "origin"]);
        if let Some(dir) = dir {
            git.current_dir(dir);
        }
        git.output()
    } && remote.status.success()
        && let Ok(forge) = crate::forge::resolve_remote(
            String::from_utf8_lossy(&remote.stdout).trim(),
            crate::forge::forge_override_from_env()?,
        )
        && forge.kind != crate::forge::ForgeKind::GitHub
    {
        return Err(CruiseError::Other(
            "PR review fix only supports GitHub, not GitLab".into(),
        ));
    }
    let mut cmd = std::process::Command::new("gh");
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    cmd.args([
        "pr",
        "view",
        &number.to_string(),
        "--json",
        "number,title,url,state,headRefName,baseRefName,isCrossRepository,isDraft",
    ]);
    if let Some(repo) = repo {
        cmd.args(["--repo", repo]);
    }
    let output = cmd
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh pr view: {e}")))?;
    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "gh pr view failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    parse_pr_view(&output.stdout, number)
}

/// Validate that a workflow supports PR review fixes.
///
/// # Errors
/// Returns an error when the required PR creation or review step is missing.
pub fn validate_workflow(config: &crate::config::WorkflowConfig) -> Result<()> {
    if config.create_pr
        && config.after_pr.values().any(|step| {
            step.github_review
                .as_ref()
                .is_some_and(|review| review.reviewers == crate::config::ReviewerScope::All)
        })
    {
        Ok(())
    } else {
        Err(CruiseError::Other(format!(
            "PR review fix requires create_pr and a github-review step with reviewers: all; use {BUILTIN_SELECTOR}"
        )))
    }
}

/// Require at least one review step that will not be disabled at execution.
///
/// # Errors
/// Returns an error if PR creation or an enabled review-all step is missing.
pub fn validate_effective_workflow(
    workflow: &crate::workflow::CompiledWorkflow,
    flags: &crate::flags::FlagValues,
    skipped_steps: &[String],
) -> Result<()> {
    if workflow.create_pr
        && workflow.after_pr.iter().any(|(name, step)| {
            !skipped_steps.contains(name)
                && !crate::flags::is_disabled(step.flag.as_ref(), flags)
                && step
                    .github_review
                    .as_ref()
                    .is_some_and(|review| review.reviewers == crate::config::ReviewerScope::All)
        })
    {
        Ok(())
    } else {
        Err(CruiseError::Other(format!(
            "PR review fix requires an enabled, unskipped github-review step with reviewers: all; use {BUILTIN_SELECTOR}"
        )))
    }
}

/// Build the plan for a pull request review-fix session.
#[must_use]
pub fn plan_text(target: &PrTarget, extra: &str) -> String {
    let title: String = target
        .title
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut plan = format!(
        "# PR #{} review fix: {}\n\nPull request: {}\nHead branch: {}\n\nAddress unresolved review threads and commit changes to the head branch.\n",
        target.number, title, target.url, target.head_branch
    );
    if !extra.trim().is_empty() {
        let _ = writeln!(plan, "\n## Additional instruction\n\n{}", extra.trim());
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr() -> serde_json::Value {
        serde_json::json!({"number":12,"title":"Fix","url":"https://github.com/a/b/pull/12","state":"OPEN","headRefName":"feat/x","baseRefName":"main","isCrossRepository":false,"isDraft":true})
    }
    fn parse(v: &serde_json::Value) -> Result<PrTarget> {
        parse_pr_view(
            &serde_json::to_vec(v).unwrap_or_else(|error| panic!("unexpected error: {error}")),
            12,
        )
    }
    #[test]
    fn validates_pull_request_and_rejects_unsafe_targets() {
        let target = parse(&pr()).unwrap_or_else(|error| panic!("unexpected error: {error}"));
        assert_eq!(target.head_branch, "feat/x");
        for state in ["CLOSED", "MERGED"] {
            let mut v = pr();
            v["state"] = state.into();
            assert!(
                parse(&v)
                    .err()
                    .unwrap_or_else(|| panic!("expected an error"))
                    .to_string()
                    .contains("only open pull requests")
            );
        }
        let mut v = pr();
        v["isCrossRepository"] = true.into();
        assert!(
            parse(&v)
                .err()
                .unwrap_or_else(|| panic!("expected an error"))
                .to_string()
                .contains("fork")
        );
        let mut v = pr();
        v["headRefName"] = "".into();
        assert!(parse(&v).is_err());
        assert!(
            parse_pr_view(b"not json", 12)
                .err()
                .unwrap_or_else(|| panic!("expected an error"))
                .to_string()
                .contains("invalid gh pr view output")
        );
    }
    #[test]
    fn plan_sanitizes_title_and_trims_optional_instruction() {
        let mut target = parse(&pr()).unwrap_or_else(|error| panic!("unexpected error: {error}"));
        target.title = "Fix\nheading".into();
        let plan = plan_text(&target, "  focus on tests  ");
        assert!(plan.starts_with("# PR #12 review fix: Fix heading\n"));
        assert!(plan.contains("## Additional instruction\n\nfocus on tests\n"));
        assert!(!plan_text(&target, " \n").contains("Additional instruction"));
    }
    #[test]
    fn builtin_workflow_meets_requirements() {
        let config =
            crate::config::WorkflowConfig::from_yaml(include_str!("../builtin/pr-review-fix.yaml"))
                .unwrap_or_else(|error| panic!("unexpected error: {error}"));
        assert!(validate_workflow(&config).is_ok());
        let mut config = config;
        config.create_pr = false;
        assert!(
            validate_workflow(&config)
                .err()
                .unwrap_or_else(|| panic!("expected an error"))
                .to_string()
                .contains(BUILTIN_SELECTOR)
        );
    }

    #[test]
    fn effective_workflow_requires_an_enabled_unskipped_review() {
        let config = crate::config::WorkflowConfig::from_yaml(
            "flags:\n  review: {}\ncreate_pr: true\nsteps:\n  work:\n    command: echo work\nafter-pr:\n  first:\n    flag: review\n    github-review:\n      reviewers: all\n    prompt: review\n  second:\n    github-review:\n      reviewers: all\n    prompt: review\n",
        ).unwrap_or_else(|error| panic!("unexpected error: {error}"));
        let compiled = crate::workflow::compile(config)
            .unwrap_or_else(|error| panic!("unexpected error: {error}"));
        let mut flags = crate::flags::FlagValues::new();
        let skip_second = vec!["second".to_string()];
        assert!(validate_effective_workflow(&compiled, &flags, &skip_second).is_err());
        flags.insert("review".into(), true);
        assert!(validate_effective_workflow(&compiled, &flags, &skip_second).is_ok());
        assert!(
            validate_effective_workflow(&compiled, &flags, &["first".into(), "second".into()])
                .is_err()
        );
        flags.insert("review".into(), false);
        assert!(validate_effective_workflow(&compiled, &flags, &[]).is_ok());
    }

    #[test]
    fn session_pr_target_roundtrip_and_legacy_compatibility() {
        let mut session = crate::session::SessionState::new(
            "test".into(),
            std::path::PathBuf::from("."),
            crate::session_config::SessionConfigRef::BuiltinSnapshot { name: None },
            "task".into(),
        );
        let json = serde_json::to_value(&session)
            .unwrap_or_else(|error| panic!("unexpected error: {error}"));
        assert!(json.get("pr_target").is_none());
        assert!(
            serde_json::from_value::<crate::session::SessionState>(json)
                .unwrap_or_else(|error| panic!("unexpected error: {error}"))
                .pr_target
                .is_none()
        );
        session.pr_target =
            Some(parse(&pr()).unwrap_or_else(|error| panic!("unexpected error: {error}")));
        let restored: crate::session::SessionState = serde_json::from_slice(
            &serde_json::to_vec(&session)
                .unwrap_or_else(|error| panic!("unexpected error: {error}")),
        )
        .unwrap_or_else(|error| panic!("unexpected error: {error}"));
        assert_eq!(session.pr_target, restored.pr_target);
    }
}
