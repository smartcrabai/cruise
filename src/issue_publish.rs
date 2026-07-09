use crate::error::{CruiseError, Result};
use crate::session::{SessionManager, SessionState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedIssue {
    pub url: String,
    pub repo: String,
}

#[must_use]
pub fn parse_github_repo_from_origin(_origin_url: &str) -> Option<String> {
    todo!("parse GitHub origin URLs into owner/repo")
}

pub fn publish_plan_issue_and_delete(
    _manager: &SessionManager,
    _session: SessionState,
    _mention_cruise: bool,
) -> Result<PublishedIssue> {
    Err(CruiseError::Other(
        "publish_plan_issue_and_delete is not implemented yet".to_string(),
    ))
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

        let err = publish_plan_issue_and_delete(&manager, session.clone(), false)
            .expect_err("gh failure should fail publishing");

        assert!(err.to_string().contains("gh issue create"));
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

        let err = publish_plan_issue_and_delete(&manager, session.clone(), false)
            .expect_err("empty plan should fail");

        assert!(err.to_string().contains("generated plan"));
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
        let exit_code = if succeed { 0 } else { 1 };
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
