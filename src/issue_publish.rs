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
/// the `origin` remote of `session.base_dir`. The issue body is always the
/// generated plan, unchanged. When `trigger_cruise` is `true`, a separate
/// `@cruise run` comment is posted on the created issue after it's created,
/// so the `@cruise` GitHub Action picks it up.
///
/// If a previous call already created the issue but failed on the follow-up
/// comment, `session.published_issue_url` carries the created issue's URL;
/// this call reuses it instead of running `gh issue create` again, so retries
/// never create a duplicate issue.
///
/// # Errors
///
/// Returns an error if the repository cannot be determined, the generated
/// plan is missing or empty, `gh issue create` fails, or (when
/// `trigger_cruise` is `true`) the follow-up `gh issue comment` fails. The
/// session is left in place whenever publishing fails; if the issue was
/// already created but the comment failed, the error message includes the
/// issue URL and the session's `published_issue_url` is set so a retry posts
/// the comment without recreating the issue.
pub fn publish_plan_issue_and_delete(
    manager: &SessionManager,
    mut session: SessionState,
    trigger_cruise: bool,
) -> Result<PublishedIssue> {
    let repo = resolve_repo(&session)?;

    let url = if let Some(existing_url) = session.published_issue_url.clone() {
        existing_url
    } else {
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

        let body_path = manager
            .sessions_dir()
            .join(&session.id)
            .join("issue-body.md");
        std::fs::write(&body_path, &plan_content)?;

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

        session.published_issue_url = Some(url.clone());
        manager.save(&session)?;
        url
    };

    if trigger_cruise {
        let comment_output = std::process::Command::new("gh")
            .arg("issue")
            .arg("comment")
            .arg(&url)
            .arg("--body")
            .arg("@cruise run")
            .output()
            .map_err(|e| CruiseError::Other(format!("failed to run gh issue comment: {e}")))?;

        if !comment_output.status.success() {
            let stderr = String::from_utf8_lossy(&comment_output.stderr);
            return Err(CruiseError::Other(format!(
                "issue {url} was created, but posting the `@cruise run` comment failed: {}. \
                 Post the comment manually, or retry publishing -- the existing issue will be reused, not duplicated.",
                stderr.trim()
            )));
        }
    }

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
        install_issue_gh(&bin_dir, &gh_log, temp.path().join("body.md"), true, true);
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
    fn publish_plan_issue_body_is_plan_content_verbatim_regardless_of_trigger_cruise() {
        // trigger_cruise only controls whether a follow-up comment is posted
        // (see the dedicated posts_run_comment / does_not_post_comment tests
        // below); the issue body itself is always plan.md, unchanged.
        let _lock = crate::test_support::lock_process();
        for trigger_cruise in [false, true] {
            let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
            let manager = SessionManager::new(temp.path().join("data"));
            let mut session = make_awaiting_session("sess-body-verbatim", temp.path());
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
            install_issue_gh(&bin_dir, &gh_log, captured_body.clone(), true, true);
            let _path = crate::test_support::prepend_to_path(&bin_dir);

            publish_plan_issue_and_delete(&manager, session, trigger_cruise)
                .unwrap_or_else(|e| panic!("{e}"));

            let body = fs::read_to_string(captured_body).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(body, "# Build the feature\nDetails\n");
        }
    }

    #[test]
    fn publish_plan_issue_trigger_cruise_true_posts_run_comment() {
        // Given: a session ready to publish
        let _lock = crate::test_support::lock_process();
        let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let manager = SessionManager::new(temp.path().join("data"));
        let mut session = make_awaiting_session("sess-trigger-comment", temp.path());
        session.repo = Some("owner/repo".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        fs::write(
            session.plan_path(&manager.sessions_dir()),
            "# Build the feature\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let bin_dir = temp.path().join("bin");
        let gh_log = temp.path().join("gh.log");
        install_issue_gh(&bin_dir, &gh_log, temp.path().join("body.md"), true, true);
        let _path = crate::test_support::prepend_to_path(&bin_dir);

        // When: trigger_cruise = true
        let published = publish_plan_issue_and_delete(&manager, session, true)
            .unwrap_or_else(|e| panic!("{e}"));

        // Then: a separate `gh issue comment <url> --body "@cruise run"` call was made
        let log = fs::read_to_string(&gh_log).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            log.contains("issue comment") && log.contains(&published.url),
            "expected gh issue comment on the created issue: {log}"
        );
        assert!(
            log.contains("@cruise run"),
            "comment body should be exactly '@cruise run': {log}"
        );
    }

    #[test]
    fn publish_plan_issue_trigger_cruise_false_does_not_post_comment() {
        // Given: a session ready to publish
        let _lock = crate::test_support::lock_process();
        let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let manager = SessionManager::new(temp.path().join("data"));
        let mut session = make_awaiting_session("sess-no-comment", temp.path());
        session.repo = Some("owner/repo".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        fs::write(
            session.plan_path(&manager.sessions_dir()),
            "# Build the feature\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let bin_dir = temp.path().join("bin");
        let gh_log = temp.path().join("gh.log");
        install_issue_gh(&bin_dir, &gh_log, temp.path().join("body.md"), true, true);
        let _path = crate::test_support::prepend_to_path(&bin_dir);

        // When: trigger_cruise = false
        publish_plan_issue_and_delete(&manager, session, false).unwrap_or_else(|e| panic!("{e}"));

        // Then: no comment call was made
        let log = fs::read_to_string(&gh_log).unwrap_or_else(|e| panic!("{e}"));
        assert!(
            !log.contains("issue comment"),
            "should not comment when trigger_cruise is false: {log}"
        );
    }

    #[test]
    fn publish_plan_issue_comment_failure_keeps_session_and_reports_issue_url() {
        // Given: `gh issue create` succeeds but the follow-up `gh issue comment` fails
        let _lock = crate::test_support::lock_process();
        let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let manager = SessionManager::new(temp.path().join("data"));
        let mut session = make_awaiting_session("sess-comment-fail", temp.path());
        session.repo = Some("owner/repo".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        fs::write(
            session.plan_path(&manager.sessions_dir()),
            "# Build the feature\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let bin_dir = temp.path().join("bin");
        let gh_log = temp.path().join("gh.log");
        install_issue_gh(&bin_dir, &gh_log, temp.path().join("body.md"), true, false);
        let _path = crate::test_support::prepend_to_path(&bin_dir);

        // When
        let err = crate::test_support::err_string(publish_plan_issue_and_delete(
            &manager,
            session.clone(),
            true,
        ));

        // Then: the issue was already created, so its URL is surfaced for a manual retry
        assert!(
            err.contains("https://github.com/owner/repo/issues/123"),
            "error should include the created issue URL: {err}"
        );
        // And: the local session is left in place (not rolled back, not deleted)
        assert!(manager.sessions_dir().join(&session.id).exists());
        // And: the created issue URL is persisted so a retry does not recreate it
        let reloaded = manager.load(&session.id).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            reloaded.published_issue_url.as_deref(),
            Some("https://github.com/owner/repo/issues/123")
        );
    }

    #[test]
    fn publish_plan_issue_retry_after_comment_failure_does_not_duplicate_issue() {
        // Given: a first attempt that created the issue but failed to post the comment
        let _lock = crate::test_support::lock_process();
        let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let manager = SessionManager::new(temp.path().join("data"));
        let mut session = make_awaiting_session("sess-retry", temp.path());
        session.repo = Some("owner/repo".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        fs::write(
            session.plan_path(&manager.sessions_dir()),
            "# Build the feature\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let bin_dir = temp.path().join("bin");
        let gh_log = temp.path().join("gh.log");
        install_issue_gh(&bin_dir, &gh_log, temp.path().join("body.md"), true, false);
        let _path = crate::test_support::prepend_to_path(&bin_dir);

        crate::test_support::err_string(publish_plan_issue_and_delete(
            &manager,
            session.clone(),
            true,
        ));
        let after_first_attempt = manager.load(&session.id).unwrap_or_else(|e| panic!("{e}"));

        // When: retrying with a `gh` that would fail this test if `issue create`
        // ran again (only `issue comment` is wired to succeed)
        install_issue_gh(&bin_dir, &gh_log, temp.path().join("body.md"), false, true);
        let published = publish_plan_issue_and_delete(&manager, after_first_attempt, true)
            .unwrap_or_else(|e| panic!("{e}"));

        // Then: the same issue URL is reused, no second `issue create` call happened,
        // and the session is now deleted
        assert_eq!(published.url, "https://github.com/owner/repo/issues/123");
        let log = fs::read_to_string(&gh_log).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            log.matches("issue create").count(),
            1,
            "issue create should only run once across the retry: {log}"
        );
        assert!(!manager.sessions_dir().join(&session.id).exists());
    }

    #[test]
    fn publish_plan_issue_from_planned_session_succeeds_and_deletes_session() {
        // Given: a session already approved into "Planned" phase
        let _lock = crate::test_support::lock_process();
        let temp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let manager = SessionManager::new(temp.path().join("data"));
        let mut session = make_awaiting_session("sess-planned", temp.path());
        session.phase = SessionPhase::Planned;
        session.repo = Some("owner/repo".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        fs::write(
            session.plan_path(&manager.sessions_dir()),
            "# Build the feature\n",
        )
        .unwrap_or_else(|e| panic!("{e}"));

        let bin_dir = temp.path().join("bin");
        let gh_log = temp.path().join("gh.log");
        install_issue_gh(&bin_dir, &gh_log, temp.path().join("body.md"), true, true);
        let _path = crate::test_support::prepend_to_path(&bin_dir);

        // When
        let published = publish_plan_issue_and_delete(&manager, session.clone(), true)
            .unwrap_or_else(|e| panic!("{e}"));

        // Then
        assert_eq!(published.repo, "owner/repo");
        assert!(!manager.sessions_dir().join(&session.id).exists());
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
            true,
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
        install_issue_gh(&bin_dir, &gh_log, temp.path().join("body.md"), true, true);
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

    /// Installs a fake `gh` on `PATH` that logs every invocation to `log_path`
    /// and handles two subcommands:
    /// - `gh issue create ... --body-file <path>`: copies the body to
    ///   `captured_body`, prints the fixed issue URL, and exits according to
    ///   `create_succeeds`.
    /// - `gh issue comment <url> --body "..."`: exits according to
    ///   `comment_succeeds`, without touching `captured_body`.
    #[cfg(unix)]
    fn install_issue_gh(
        bin_dir: impl AsRef<Path>,
        log_path: impl AsRef<Path>,
        captured_body: impl AsRef<Path>,
        create_succeeds: bool,
        comment_succeeds: bool,
    ) {
        use std::os::unix::fs::PermissionsExt;

        fs::create_dir_all(bin_dir.as_ref()).unwrap_or_else(|e| panic!("{e}"));
        let script_path = bin_dir.as_ref().join("gh");
        let create_exit_code = i32::from(!create_succeeds);
        let comment_exit_code = i32::from(!comment_succeeds);
        let stdout = if create_succeeds {
            "https://github.com/owner/repo/issues/123"
        } else {
            ""
        };
        let script = format!(
            "#!/bin/sh\n\
printf '%s\\n' \"$*\" >> '{log}'\n\
if [ \"$1\" = 'issue' ] && [ \"$2\" = 'comment' ]; then\n\
  exit {comment_exit_code}\n\
fi\n\
body_file=''\n\
prev=''\n\
for arg in \"$@\"; do\n\
  if [ \"$prev\" = '--body-file' ]; then body_file=\"$arg\"; fi\n\
  prev=\"$arg\"\n\
done\n\
if [ -n \"$body_file\" ]; then cp \"$body_file\" '{body}'; fi\n\
printf '%s\\n' '{stdout}'\n\
exit {create_exit_code}\n",
            log = log_path.as_ref().display(),
            body = captured_body.as_ref().display(),
            stdout = stdout,
            create_exit_code = create_exit_code,
            comment_exit_code = comment_exit_code,
        );
        fs::write(&script_path, script).unwrap_or_else(|e| panic!("{e}"));
        let mut permissions = fs::metadata(&script_path)
            .unwrap_or_else(|e| panic!("{e}"))
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(script_path, permissions).unwrap_or_else(|e| panic!("{e}"));
    }
}
