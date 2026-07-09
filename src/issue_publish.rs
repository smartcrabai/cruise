use crate::error::{CruiseError, Result};
use crate::session::{SessionManager, SessionState};
use crate::worktree_pr::gh_output_line;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedIssue {
    pub url: String,
    pub repo: String,
}

/// Parse a `git remote` origin URL into a GitHub `owner/repo` slug.
///
/// Supports HTTPS URLs (`https://github.com/owner/repo[.git]`), SSH URLs
/// (`ssh://git@github.com/owner/repo[.git]`), and the scp-like syntax
/// (`git@github.com:owner/repo[.git]`). Returns `None` for non-GitHub hosts
/// or input that doesn't match any of these forms.
#[must_use]
pub fn parse_github_repo_from_origin(origin_url: &str) -> Option<String> {
    let trimmed = origin_url.trim();
    let without_suffix = trimmed.strip_suffix(".git").unwrap_or(trimmed);

    let (host, path) = if let Some((_scheme, remainder)) = without_suffix.split_once("://") {
        let after_user = remainder
            .rsplit_once('@')
            .map_or(remainder, |(_, host_and_path)| host_and_path);
        after_user.split_once('/')?
    } else {
        let (user_and_host, path) = without_suffix.split_once(':')?;
        let host = user_and_host
            .rsplit_once('@')
            .map_or(user_and_host, |(_, host)| host);
        (host, path)
    };

    if host != "github.com" {
        return None;
    }

    let mut segments = path.trim_matches('/').split('/');
    let owner = segments.next().filter(|s| !s.is_empty())?;
    let repo = segments.next().filter(|s| !s.is_empty())?;
    if segments.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Resolve the `owner/repo` slug for `session`: its recorded `repo` field if
/// set, otherwise the `origin` remote of its `base_dir` git checkout.
fn resolve_repo(session: &SessionState) -> Result<String> {
    if let Some(repo) = &session.repo {
        return Ok(repo.clone());
    }

    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(&session.base_dir)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git remote get-url origin: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::Other(format!(
            "failed to determine GitHub repository: git remote get-url origin failed: {}",
            stderr.trim()
        )));
    }

    let origin_url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    parse_github_repo_from_origin(&origin_url).ok_or_else(|| {
        CruiseError::Other(format!(
            "could not determine GitHub repository from origin URL: {origin_url}"
        ))
    })
}

/// Publish a session's generated plan as a GitHub issue, then delete the
/// local session.
///
/// The target repository comes from `session.repo` if set, otherwise from
/// the `origin` remote of `session.base_dir`. When `mention_cruise` is
/// `true`, the issue body is prefixed with an `@cruise` mention line so the
/// `@cruise` GitHub Action picks up the issue.
///
/// # Errors
///
/// Returns an error if the repository cannot be determined, the generated
/// plan is missing or empty, or `gh issue create` fails. The session is left
/// in place whenever publishing fails.
#[expect(
    clippy::needless_pass_by_value,
    reason = "session is consumed and its backing directory is deleted by this function"
)]
pub fn publish_plan_issue_and_delete(
    manager: &SessionManager,
    session: SessionState,
    mention_cruise: bool,
) -> Result<PublishedIssue> {
    let repo = resolve_repo(&session)?;

    let plan_path = session.plan_path(&manager.sessions_dir());
    let plan_content = std::fs::read_to_string(&plan_path).map_err(|e| {
        CruiseError::Other(format!(
            "failed to read generated plan at {}: {e}",
            plan_path.display()
        ))
    })?;
    if plan_content.trim().is_empty() {
        return Err(CruiseError::Other(format!(
            "no generated plan found for session {}",
            session.id
        )));
    }

    let body = if mention_cruise {
        format!("@cruise\n\n{plan_content}")
    } else {
        plan_content
    };

    let body_path = manager
        .sessions_dir()
        .join(&session.id)
        .join("issue-body.md");
    std::fs::write(&body_path, &body)?;

    let output = std::process::Command::new("gh")
        .arg("issue")
        .arg("create")
        .arg("--repo")
        .arg(&repo)
        .arg("--title")
        .arg(session.title_or_input())
        .arg("--body-file")
        .arg(&body_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh issue create: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::Other(format!(
            "gh issue create failed: {}",
            stderr.trim()
        )));
    }

    let url = gh_output_line(&output.stdout).ok_or_else(|| {
        CruiseError::Other("gh issue create succeeded but printed no URL".to_string())
    })?;

    if session.repo.is_some() {
        crate::repo_clone::cleanup_session_workspace(manager, &session);
    } else if let Some(ctx) = session.worktree_context()
        && let Err(e) = crate::worktree::cleanup_worktree(&ctx)
    {
        eprintln!("warning: failed to remove worktree for {}: {e}", session.id);
    }
    manager.delete(&session.id)?;

    Ok(PublishedIssue { url, repo })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionPhase, SessionState};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    #[test]
    fn parse_github_repo_from_origin_https() {
        assert_eq!(
            parse_github_repo_from_origin("https://github.com/owner/repo.git").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(
            parse_github_repo_from_origin("https://github.com/owner/repo").as_deref(),
            Some("owner/repo")
        );
    }

    #[test]
    fn parse_github_repo_from_origin_ssh_scp() {
        assert_eq!(
            parse_github_repo_from_origin("git@github.com:owner/repo.git").as_deref(),
            Some("owner/repo")
        );
    }

    #[test]
    fn parse_github_repo_from_origin_ssh_url() {
        assert_eq!(
            parse_github_repo_from_origin("ssh://git@github.com/owner/repo.git").as_deref(),
            Some("owner/repo")
        );
    }

    #[test]
    fn parse_github_repo_from_origin_rejects_non_github_urls() {
        assert_eq!(
            parse_github_repo_from_origin("https://example.com/owner/repo"),
            None
        );
        assert_eq!(
            parse_github_repo_from_origin("git@example.com:owner/repo.git"),
            None
        );
        assert_eq!(parse_github_repo_from_origin("not a url"), None);
    }

    #[test]
    fn publish_plan_issue_uses_session_repo_and_deletes_session() {
        let _lock = crate::test_support::lock_process();
        let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let manager = SessionManager::new(temp.path().join("data"));
        let mut session = make_awaiting_session("sess-repo", temp.path());
        session.repo = Some("owner/repo".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        fs::write(
            session.plan_path(&manager.sessions_dir()),
            "# Build the feature\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let bin_dir = temp.path().join("bin");
        let gh_log = temp.path().join("gh.log");
        install_issue_gh(&bin_dir, &gh_log, temp.path().join("body.md"), true);
        let _path = crate::test_support::prepend_to_path(&bin_dir);

        let published = publish_plan_issue_and_delete(&manager, session.clone(), false)
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(published.repo, "owner/repo");
        assert_eq!(published.url, "https://github.com/owner/repo/issues/123");
        assert!(
            fs::read_to_string(&gh_log)
                .unwrap_or_else(|e| panic!("{e}"))
                .contains("issue create --repo owner/repo")
        );
        assert!(!manager.sessions_dir().join(&session.id).exists());
    }

    #[test]
    fn publish_plan_issue_includes_mention_when_enabled() {
        let _lock = crate::test_support::lock_process();
        let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let manager = SessionManager::new(temp.path().join("data"));
        let mut session = make_awaiting_session("sess-mention", temp.path());
        session.repo = Some("owner/repo".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        fs::write(
            session.plan_path(&manager.sessions_dir()),
            "# Build the feature\nDetails\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let bin_dir = temp.path().join("bin");
        let gh_log = temp.path().join("gh.log");
        let captured_body = temp.path().join("captured-body.md");
        install_issue_gh(&bin_dir, &gh_log, captured_body.clone(), true);
        let _path = crate::test_support::prepend_to_path(&bin_dir);

        publish_plan_issue_and_delete(&manager, session, true).unwrap_or_else(|e| panic!("{e}"));

        let body = fs::read_to_string(captured_body).unwrap_or_else(|e| panic!("{e}"));
        assert!(body.starts_with("@cruise\n\n# Build the feature\n"));
    }

    #[test]
    fn publish_plan_issue_does_not_delete_session_when_gh_fails() {
        let _lock = crate::test_support::lock_process();
        let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let manager = SessionManager::new(temp.path().join("data"));
        let mut session = make_awaiting_session("sess-fail", temp.path());
        session.repo = Some("owner/repo".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        fs::write(
            session.plan_path(&manager.sessions_dir()),
            "# Build the feature\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let bin_dir = temp.path().join("bin");
        install_issue_gh(
            &bin_dir,
            temp.path().join("gh.log"),
            temp.path().join("body.md"),
            false,
        );
        let _path = crate::test_support::prepend_to_path(&bin_dir);

        let err = crate::test_support::err_string(publish_plan_issue_and_delete(
            &manager,
            session.clone(),
            false,
        ));

        assert!(err.contains("gh issue create"));
        assert!(manager.sessions_dir().join(&session.id).exists());
    }

    #[test]
    fn publish_plan_issue_errors_when_plan_missing_or_empty() {
        let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let manager = SessionManager::new(temp.path().join("data"));
        let mut session = make_awaiting_session("sess-empty", temp.path());
        session.repo = Some("owner/repo".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        fs::write(session.plan_path(&manager.sessions_dir()), "   \n")
            .unwrap_or_else(|e| panic!("{e}"));

        let err = crate::test_support::err_string(publish_plan_issue_and_delete(
            &manager,
            session.clone(),
            false,
        ));

        assert!(err.contains("generated plan"));
        assert!(manager.sessions_dir().join(&session.id).exists());
    }

    #[test]
    fn publish_plan_issue_infers_repo_from_local_origin() {
        let _lock = crate::test_support::lock_process();
        let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let repo_dir = temp.path().join("repo");
        fs::create_dir_all(&repo_dir).unwrap_or_else(|e| panic!("{e}"));
        crate::test_support::init_git_repo(&repo_dir);
        crate::test_support::run_git_ok(
            &repo_dir,
            &["remote", "add", "origin", "git@github.com:owner/repo.git"],
        );

        let manager = SessionManager::new(temp.path().join("data"));
        let session = make_awaiting_session("sess-origin", &repo_dir);
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        fs::write(
            session.plan_path(&manager.sessions_dir()),
            "# Build the feature\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let bin_dir = temp.path().join("bin");
        let gh_log = temp.path().join("gh.log");
        install_issue_gh(&bin_dir, &gh_log, temp.path().join("body.md"), true);
        let _path = crate::test_support::prepend_to_path(&bin_dir);

        let published = publish_plan_issue_and_delete(&manager, session.clone(), false)
            .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(published.repo, "owner/repo");
        assert!(
            fs::read_to_string(&gh_log)
                .unwrap_or_else(|e| panic!("{e}"))
                .contains("issue create --repo owner/repo")
        );
        assert!(!manager.sessions_dir().join(&session.id).exists());
    }

    fn make_awaiting_session(id: &str, base_dir: &Path) -> SessionState {
        let mut session = SessionState::new(
            id.to_string(),
            base_dir.to_path_buf(),
            "cruise.yaml".to_string(),
            "test task".to_string(),
        );
        session.phase = SessionPhase::AwaitingApproval;
        session
    }

    #[cfg(unix)]
    fn install_issue_gh(
        bin_dir: impl AsRef<Path>,
        log_path: impl AsRef<Path>,
        captured_body: impl AsRef<Path>,
        succeed: bool,
    ) {
        use std::os::unix::fs::PermissionsExt;

        fs::create_dir_all(bin_dir.as_ref()).unwrap_or_else(|e| panic!("{e}"));
        let script_path = bin_dir.as_ref().join("gh");
        let exit_code = i32::from(!succeed);
        let stdout = if succeed {
            "https://github.com/owner/repo/issues/123"
        } else {
            ""
        };
        let script = format!(
            "#!/bin/sh\n\
printf '%s\\n' \"$*\" >> '{log}'\n\
body_file=''\n\
prev=''\n\
for arg in \"$@\"; do\n\
  if [ \"$prev\" = '--body-file' ]; then body_file=\"$arg\"; fi\n\
  prev=\"$arg\"\n\
done\n\
if [ -n \"$body_file\" ]; then cp \"$body_file\" '{body}'; fi\n\
printf '%s\\n' '{stdout}'\n\
exit {exit_code}\n",
            log = log_path.as_ref().display(),
            body = captured_body.as_ref().display(),
            stdout = stdout,
            exit_code = exit_code,
        );
        fs::write(&script_path, script).unwrap_or_else(|e| panic!("{e}"));
        let mut permissions = fs::metadata(&script_path)
            .unwrap_or_else(|e| panic!("{e}"))
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(script_path, permissions).unwrap_or_else(|e| panic!("{e}"));
    }
}
