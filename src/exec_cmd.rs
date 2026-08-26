use crate::cli::{ExecArgs, RunArgs};
use crate::config::WorkflowConfig;
use crate::engine;
use crate::error::Result;
use crate::paths;
use crate::resolver::{ConfigSource, load_config_from_source, resolve_config};
use crate::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};

pub(crate) struct ExecRequest {
    pub input: String,
    pub max_retries: Option<usize>,
    pub rate_limit_retries: usize,
    pub dry_run: bool,
}

pub async fn run(args: ExecArgs) -> Result<()> {
    let (yaml, source) = resolve_config(args.config.as_deref())?;
    let config = load_config_from_source(&yaml, &source)?;
    run_resolved(
        &config,
        &yaml,
        &source,
        ExecRequest {
            input: args.input.unwrap_or_default(),
            max_retries: args.max_retries,
            rate_limit_retries: args.rate_limit_retries,
            dry_run: args.dry_run,
        },
    )
    .await
}

pub(crate) async fn run_resolved(
    config: &WorkflowConfig,
    yaml: &str,
    source: &ConfigSource,
    req: ExecRequest,
) -> Result<()> {
    let effective_max_retries =
        crate::config::resolve_effective_max_retries(req.max_retries, config);
    crate::config::validate_group_retry_budget(config, effective_max_retries)?;

    if req.dry_run {
        engine::print_dry_run(config, None);
        return Ok(());
    }

    let manager = SessionManager::new(paths::data_dir()?);
    let session = setup_exec_session(&manager, source, yaml, req.input)?;

    let run_args = RunArgs {
        session: Some(session.id.clone()),
        all: false,
        max_retries: req.max_retries,
        rate_limit_retries: req.rate_limit_retries,
        dry_run: false,
        cleanup_after_pr: false,
        no_cleanup_after_pr: false,
        parallelism: None,
    };

    crate::run_cmd::run(run_args).await
}

/// Create and persist a transient session for exec mode.
///
/// Sets `workspace_mode = CurrentBranch`, allows execution on a dirty working
/// tree, and sets `phase = Planned` so `run_cmd` skips worktree creation and
/// PR flow. The session is removed after terminal execution; only paused or
/// suspended runs remain for explicit resume. Writes an empty `plan.md`
/// placeholder so that configs referencing `{plan}` do not fail with "No such
/// file".
pub(crate) fn setup_exec_session(
    manager: &SessionManager,
    source: &ConfigSource,
    yaml: &str,
    input: String,
) -> Result<SessionState> {
    let session_id = SessionManager::new_session_id();
    let base_dir = std::env::current_dir()?;
    let mut session =
        SessionState::new(session_id.clone(), base_dir, source.display_string(), input);
    session.config_path = source.path().cloned();
    session.workspace_mode = WorkspaceMode::CurrentBranch;
    session.allow_dirty_working_tree = true;
    session.phase = SessionPhase::Planned;
    session.exec = true;
    manager.create(&session)?;

    let session_dir = manager.sessions_dir().join(&session_id);
    let write_result: crate::error::Result<()> = (|| {
        if session.config_path.is_none() {
            std::fs::write(session_dir.join("config.yaml"), yaml)?;
        }
        let plan_path = session.plan_path(&manager.sessions_dir());
        std::fs::write(&plan_path, "")?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_dir_all(&session_dir);
        return Err(e);
    }

    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{SessionPhase, WorkspaceMode};
    use crate::test_support::{group_retry_budget_config_with, init_git_repo, run_git_ok};
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    // ---------------------------------------------------------------------------
    // Process-level test helpers (mirrors the pattern in run_cmd.rs)
    // ---------------------------------------------------------------------------

    struct ProcessStateGuard {
        prev_home: Option<std::ffi::OsString>,
        prev_userprofile: Option<std::ffi::OsString>,
        prev_path: Option<std::ffi::OsString>,
        prev_xdg_data_home: Option<std::ffi::OsString>,
        prev_xdg_config_home: Option<std::ffi::OsString>,
        prev_xdg_state_home: Option<std::ffi::OsString>,
        prev_dir: PathBuf,
        lock: crate::test_support::ProcessLock,
    }

    impl ProcessStateGuard {
        fn new(home: &Path) -> Self {
            let lock = crate::test_support::lock_process();
            let prev_home = std::env::var_os("HOME");
            let prev_userprofile = std::env::var_os("USERPROFILE");
            let prev_path = std::env::var_os("PATH");
            let prev_xdg_data_home = std::env::var_os("XDG_DATA_HOME");
            let prev_xdg_config_home = std::env::var_os("XDG_CONFIG_HOME");
            let prev_xdg_state_home = std::env::var_os("XDG_STATE_HOME");
            let prev_dir = std::env::current_dir().unwrap_or_else(|_| fallback_root());
            unsafe {
                std::env::set_var("HOME", home);
                std::env::set_var("USERPROFILE", home);
                std::env::remove_var("XDG_DATA_HOME");
                std::env::remove_var("XDG_CONFIG_HOME");
                std::env::remove_var("XDG_STATE_HOME");
            }
            Self {
                prev_home,
                prev_userprofile,
                prev_path,
                prev_xdg_data_home,
                prev_xdg_config_home,
                prev_xdg_state_home,
                prev_dir,
                lock,
            }
        }

        fn set_current_dir(&self, dir: &Path) {
            let _ = &self.lock;
            let _ = std::env::set_current_dir(dir);
        }

        fn prepend_path(&self, dir: &Path) {
            let _ = &self.lock;
            let mut paths = vec![dir.to_path_buf()];
            if let Some(existing) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&existing));
            }
            if let Ok(joined) = std::env::join_paths(paths) {
                unsafe { std::env::set_var("PATH", joined) };
            }
        }
    }

    impl Drop for ProcessStateGuard {
        fn drop(&mut self) {
            if std::env::set_current_dir(&self.prev_dir).is_err() {
                let _ = std::env::set_current_dir(fallback_root());
            }
            unsafe {
                macro_rules! restore {
                    ($var:literal, $field:expr) => {
                        if let Some(ref v) = $field {
                            std::env::set_var($var, v);
                        } else {
                            std::env::remove_var($var);
                        }
                    };
                }
                restore!("HOME", self.prev_home);
                restore!("USERPROFILE", self.prev_userprofile);
                restore!("PATH", self.prev_path);
                restore!("XDG_DATA_HOME", self.prev_xdg_data_home);
                restore!("XDG_CONFIG_HOME", self.prev_xdg_config_home);
                restore!("XDG_STATE_HOME", self.prev_xdg_state_home);
            }
        }
    }

    fn fallback_root() -> PathBuf {
        #[cfg(unix)]
        return PathBuf::from("/");
        #[cfg(windows)]
        return PathBuf::from("C:\\");
    }

    fn create_repo_with_origin(tmp: &TempDir) -> PathBuf {
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);

        let bare = tmp.path().join("origin.git");
        run_git_ok(tmp.path(), &["init", "--bare", "origin.git"]);
        run_git_ok(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                bare.to_str().unwrap_or_else(|| panic!("unexpected None")),
            ],
        );
        repo
    }

    fn install_logging_gh(bin_dir: &Path, log_path: &Path, url: &str) {
        fs::create_dir_all(bin_dir).unwrap_or_else(|e| panic!("{e:?}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script_path = bin_dir.join("gh");
            let script = format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf '%s\\n' 'gh version test'\n  exit 0\nfi\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then\n  printf '%s\\n' \"{}\"\nfi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then\n  printf '%s\\n' \"{}\"\nfi\n",
                log_path.display(),
                url,
                url
            );
            fs::write(&script_path, script).unwrap_or_else(|e| panic!("{e:?}"));
            let mut perms = fs::metadata(&script_path)
                .unwrap_or_else(|e| panic!("{e:?}"))
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script_path, perms).unwrap_or_else(|e| panic!("{e:?}"));
        }
    }

    fn single_command_config(step_name: &str, command: &str) -> String {
        format!("command:\n  - cat\nsteps:\n  {step_name}:\n    command: |\n      {command}\n")
    }

    fn exec_args_no_op(config_path: &Path) -> ExecArgs {
        ExecArgs {
            input: Some("test task".to_string()),
            config: Some(
                config_path
                    .to_str()
                    .unwrap_or_else(|| panic!("non-utf8 path"))
                    .to_string(),
            ),
            max_retries: None,
            rate_limit_retries: 0,
            dry_run: false,
        }
    }

    // ---------------------------------------------------------------------------
    // Unit tests: setup_exec_session
    // ---------------------------------------------------------------------------

    #[test]
    fn test_exec_creates_session_with_current_branch_mode() {
        // Given: a temp home dir with a git repo set as current dir
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let manager =
            SessionManager::new(crate::paths::data_dir().unwrap_or_else(|e| panic!("{e:?}")));
        let yaml = single_command_config("noop", "printf noop");
        let config_file = tmp.path().join("cruise.yaml");
        fs::write(&config_file, &yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let source = crate::resolver::ConfigSource::Explicit(config_file);

        // When: setup_exec_session is called
        let session = setup_exec_session(&manager, &source, &yaml, "test task".to_string())
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: workspace_mode is CurrentBranch and phase is Planned
        assert_eq!(
            session.workspace_mode,
            WorkspaceMode::CurrentBranch,
            "exec sessions must use CurrentBranch to skip worktree creation"
        );
        assert!(
            session.allow_dirty_working_tree,
            "exec sessions must allow tracked changes in the working tree"
        );
        assert!(
            matches!(session.phase, SessionPhase::Planned),
            "exec sessions must start in Planned phase (not AwaitingApproval)"
        );
        assert!(
            session.exec,
            "exec sessions must be marked as exec sessions"
        );
        assert_eq!(session.input, "test task");
        // Canonicalize both sides to handle macOS /private/var symlink.
        assert_eq!(
            session
                .base_dir
                .canonicalize()
                .unwrap_or_else(|e| panic!("{e:?}")),
            repo.canonicalize().unwrap_or_else(|e| panic!("{e:?}"))
        );
    }

    #[test]
    fn test_exec_writes_placeholder_plan_md() {
        // Given: a temp home dir with a git repo
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let manager =
            SessionManager::new(crate::paths::data_dir().unwrap_or_else(|e| panic!("{e:?}")));
        let yaml = single_command_config("noop", "printf noop");
        let config_file = tmp.path().join("cruise.yaml");
        fs::write(&config_file, &yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let source = crate::resolver::ConfigSource::Explicit(config_file);

        // When: setup_exec_session is called
        let session = setup_exec_session(&manager, &source, &yaml, "placeholder test".to_string())
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: an empty plan.md exists at the expected path
        let plan_path = session.plan_path(&manager.sessions_dir());
        assert!(
            plan_path.exists(),
            "plan.md placeholder must exist so {{plan}} references don't fail"
        );
        let content = fs::read_to_string(&plan_path).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            content.is_empty(),
            "placeholder plan.md should be empty, got: {content:?}"
        );
    }

    #[test]
    fn test_exec_uses_explicit_config_path() {
        // Given: an explicit config file on disk
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let config_file = tmp.path().join("my.yaml");
        let yaml = single_command_config("noop", "printf noop");
        fs::write(&config_file, &yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let source = crate::resolver::ConfigSource::Explicit(config_file.clone());

        let manager =
            SessionManager::new(crate::paths::data_dir().unwrap_or_else(|e| panic!("{e:?}")));

        // When: setup_exec_session is called with an explicit config source
        let session =
            setup_exec_session(&manager, &source, &yaml, "explicit config test".to_string())
                .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: session.config_path is the absolute path of the config file
        let stored = session
            .config_path
            .clone()
            .unwrap_or_else(|| panic!("expected config_path to be set for explicit source"));
        assert!(
            stored.is_absolute(),
            "config_path must be absolute, got: {stored:?}"
        );
        assert_eq!(
            stored, config_file,
            "config_path must point to the supplied file"
        );

        // And: no session-local config.yaml is written (the file already exists on disk)
        let session_config = manager.sessions_dir().join(&session.id).join("config.yaml");
        assert!(
            !session_config.exists(),
            "explicit config must NOT be copied into session_dir"
        );
    }

    // ---------------------------------------------------------------------------
    // Integration tests: exec end-to-end via run()
    // ---------------------------------------------------------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn test_exec_does_not_create_worktree_or_pr() {
        // Given: a git repo with a simple command config that writes a file
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let config_content = single_command_config("write", "printf exec-result > exec-out.txt");
        // Write config OUTSIDE the repo so the working tree stays clean.
        let config_path = tmp.path().join("cruise.yaml");
        fs::write(&config_path, &config_content).unwrap_or_else(|e| panic!("{e:?}"));

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/99");
        process.prepend_path(&bin_dir);

        // When: exec is called
        let result = run(exec_args_no_op(&config_path)).await;
        assert!(
            result.is_ok(),
            "exec in a clean repo should succeed: {result:?}"
        );

        // Then: the output file was written into the repo (not a worktree)
        assert!(
            repo.join("exec-out.txt").exists(),
            "exec should write changes directly into the working directory"
        );
        assert_eq!(
            fs::read_to_string(repo.join("exec-out.txt")).unwrap_or_else(|e| panic!("{e:?}")),
            "exec-result"
        );

        // And: no gh invocation happened (no PR flow)
        assert!(
            !gh_log.exists(),
            "exec should not invoke gh (no PR creation)"
        );

        // And: the transient exec session is removed after reaching a terminal phase
        let manager =
            SessionManager::new(crate::paths::data_dir().unwrap_or_else(|e| panic!("{e:?}")));
        let sessions = manager.list().unwrap_or_else(|e| panic!("{e:?}"));
        let repo_canonical = repo.canonicalize().unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            sessions
                .iter()
                .all(|s| !s.base_dir.canonicalize().is_ok_and(|p| p == repo_canonical)),
            "terminal exec sessions must not remain persisted"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_exec_if_fail_recovery_returns_success() {
        // Given: a workflow whose first command fails and explicitly recovers
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);
        let config_path = tmp.path().join("cruise.yaml");
        let config = r#"command: [cat]
steps:
  failing:
    command: "exit 1"
    if:
      fail: recover
  recover:
    command: "printf recovered > recovered.txt"
"#;
        fs::write(&config_path, config).unwrap_or_else(|e| panic!("{e:?}"));

        // When: exec runs the recoverable workflow
        let result = run(exec_args_no_op(&config_path)).await;

        // Then: recovery makes the exec command successful and terminal state
        // is still disposed of like any other completed exec run.
        assert!(
            result.is_ok(),
            "if.fail recovery should succeed: {result:?}"
        );
        assert_eq!(
            fs::read_to_string(repo.join("recovered.txt")).unwrap_or_else(|e| panic!("{e:?}")),
            "recovered"
        );
        let manager =
            SessionManager::new(crate::paths::data_dir().unwrap_or_else(|e| panic!("{e:?}")));
        assert!(
            manager
                .list()
                .unwrap_or_else(|e| panic!("{e:?}"))
                .is_empty()
        );
    }

    #[test]
    fn test_dispose_exec_session_removes_terminal_sessions() {
        // Given: exec sessions in every terminal phase
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let phases = [
            ("completed", SessionPhase::Completed),
            ("failed", SessionPhase::Failed("command failed".to_string())),
            ("planned", SessionPhase::Planned),
        ];
        for (id, phase) in phases {
            let mut session = SessionState::new(
                id.to_string(),
                tmp.path().to_path_buf(),
                "cruise.yaml".to_string(),
                "task".to_string(),
            );
            session.phase = phase;
            session.exec = true;
            manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        }

        // When: disposing each session after execution has returned
        for id in ["completed", "failed", "planned"] {
            let (saved, fingerprint) = manager
                .load_with_fingerprint(id)
                .unwrap_or_else(|e| panic!("{e:?}"));
            manager.dispose_exec_session_if_owned(id, &saved.phase, fingerprint);
        }

        // Then: all terminal exec sessions are removed
        for id in ["completed", "failed", "planned"] {
            assert!(
                !manager.sessions_dir().join(id).exists(),
                "terminal exec session {id} should be deleted"
            );
        }
    }

    #[test]
    fn test_dispose_exec_session_keeps_resumable_sessions() {
        // Given: exec sessions paused in Running and Suspended phases
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        for (id, phase) in [
            ("running", SessionPhase::Running),
            ("suspended", SessionPhase::Suspended),
        ] {
            let mut session = SessionState::new(
                id.to_string(),
                tmp.path().to_path_buf(),
                "cruise.yaml".to_string(),
                "task".to_string(),
            );
            session.phase = phase;
            session.exec = true;
            manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        }

        // When: disposing sessions that can be resumed
        for id in ["running", "suspended"] {
            let (saved, fingerprint) = manager
                .load_with_fingerprint(id)
                .unwrap_or_else(|e| panic!("{e:?}"));
            manager.dispose_exec_session_if_owned(id, &saved.phase, fingerprint);
        }

        // Then: both sessions remain available by explicit ID
        assert!(manager.sessions_dir().join("running").exists());
        assert!(manager.sessions_dir().join("suspended").exists());
    }

    #[test]
    fn test_dispose_exec_session_rejects_path_traversal() {
        // Given: an exec-looking directory outside the sessions directory
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join("data"));
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(outside.join("state.json"), "not json").unwrap_or_else(|e| panic!("{e:?}"));
        // When: disposal receives a traversal ID
        let fingerprint = crate::session::SessionStateFingerprint::from_bytes(b"");
        manager.dispose_exec_session_if_owned("../../outside", &SessionPhase::Planned, fingerprint);

        // Then: the attacker-selected directory is untouched
        assert!(outside.exists());
        assert!(outside.join("state.json").exists());
    }

    #[test]
    fn test_dispose_exec_session_removes_missing_or_corrupt_state_directory() {
        // Given: an exec session directory whose state cannot be loaded
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let id = "corrupt";
        let session_dir = manager.sessions_dir().join(id);
        fs::create_dir_all(&session_dir).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(session_dir.join("state.json"), "not json").unwrap_or_else(|e| panic!("{e:?}"));
        // When: disposal cannot load the state
        let fingerprint = crate::session::SessionStateFingerprint::from_bytes(b"");
        manager.dispose_exec_session_if_owned(id, &SessionPhase::Planned, fingerprint);

        // Then: best-effort cleanup still removes the transient directory
        assert!(!session_dir.exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_exec_failure_does_not_leave_failed_session() {
        // Given: a workflow command that exits unsuccessfully
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);
        let config_path = tmp.path().join("cruise.yaml");
        fs::write(&config_path, single_command_config("fail", "exit 1"))
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: exec runs the failing workflow
        let result = run(exec_args_no_op(&config_path)).await;

        // Then: the original execution failure is returned
        assert!(
            result.is_err(),
            "a failing exec command must return an error"
        );
        // And: terminal exec state is not retained for future auto-selection
        let manager =
            SessionManager::new(crate::paths::data_dir().unwrap_or_else(|e| panic!("{e:?}")));
        let repo_canonical = repo.canonicalize().unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            manager
                .list()
                .unwrap_or_else(|e| panic!("{e:?}"))
                .iter()
                .all(|s| !s.base_dir.canonicalize().is_ok_and(|p| p == repo_canonical))
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_exec_non_git_directory_does_not_leave_planned_session() {
        // Given: a non-git directory and an otherwise valid exec config
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let not_a_repo = tmp.path().join("not-a-repo");
        fs::create_dir(&not_a_repo).unwrap_or_else(|e| panic!("{e:?}"));
        process.set_current_dir(&not_a_repo);
        let config_path = tmp.path().join("cruise.yaml");
        fs::write(&config_path, single_command_config("noop", "printf noop"))
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: exec attempts to run outside a git repository
        let result = run(exec_args_no_op(&config_path)).await;

        // Then: workspace validation fails
        assert!(result.is_err(), "exec outside a git repo must fail");
        // And: the Planned preflight session is cleaned up
        let manager =
            SessionManager::new(crate::paths::data_dir().unwrap_or_else(|e| panic!("{e:?}")));
        assert!(
            manager
                .list()
                .unwrap_or_else(|e| panic!("{e:?}"))
                .iter()
                .all(|s| s.base_dir != not_a_repo)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_exec_runs_on_dirty_working_tree() {
        // Given: a git repo with uncommitted tracked changes (dirty tree)
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        // Keep config OUTSIDE the repo so the untracked config file itself
        // does not interfere with the dirty-tree check.
        let config_content = single_command_config("write", "printf exec-result > exec-out.txt");
        let config_path = tmp.path().join("cruise.yaml");
        fs::write(&config_path, &config_content).unwrap_or_else(|e| panic!("{e:?}"));

        // Make the tree dirty by modifying a committed (tracked) file.
        fs::write(repo.join("README.md"), "modified").unwrap_or_else(|e| panic!("{e:?}"));

        // When: exec is called on a dirty tree
        let result = run(exec_args_no_op(&config_path)).await;

        // Then: execution succeeds directly in the existing repository
        assert!(
            result.is_ok(),
            "exec should allow a dirty working tree: {result:?}"
        );
        assert_eq!(
            fs::read_to_string(repo.join("exec-out.txt")).unwrap_or_else(|e| panic!("{e:?}")),
            "exec-result"
        );

        // And: pre-existing tracked changes remain untouched
        assert_eq!(
            fs::read_to_string(repo.join("README.md")).unwrap_or_else(|e| panic!("{e:?}")),
            "modified"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_exec_dry_run_prints_flow_without_executing() {
        // Given: a git repo with a config that would write a file
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let config_content =
            single_command_config("write", "printf should-not-exist > dry-run-out.txt");
        let config_path = repo.join("cruise.yaml");
        fs::write(&config_path, &config_content).unwrap_or_else(|e| panic!("{e:?}"));

        let args = ExecArgs {
            input: None,
            config: Some(
                config_path
                    .to_str()
                    .unwrap_or_else(|| panic!("non-utf8"))
                    .to_string(),
            ),
            max_retries: None,
            rate_limit_retries: 0,
            dry_run: true,
        };

        // When: exec is called with --dry-run
        let result = run(args).await;

        // Then: it succeeds without executing the workflow
        assert!(
            result.is_ok(),
            "dry-run should return Ok without executing steps: {result:?}"
        );
        assert!(
            !repo.join("dry-run-out.txt").exists(),
            "dry-run must not execute steps or write any files"
        );

        // And: no session is persisted (dry-run exits before session creation)
        let manager =
            SessionManager::new(crate::paths::data_dir().unwrap_or_else(|e| panic!("{e:?}")));
        let sessions = manager.list().unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            sessions.iter().all(|s| s.base_dir != repo),
            "dry-run must not create a session"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_exec_fails_fast_when_group_max_retries_unreachable() {
        // Given: a config whose group max_retries can never take effect
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let config_path = tmp.path().join("cruise.yaml");
        fs::write(&config_path, group_retry_budget_config_with(5))
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: exec is called (non-dry-run)
        let result = run(exec_args_no_op(&config_path)).await;

        // Then: it fails fast, naming the group and both values
        assert!(
            result.is_err(),
            "expected an unreachable group max_retries to fail fast: {result:?}"
        );
        let message = result.map_or_else(|e| e.to_string(), |()| String::new());
        assert!(
            message.contains("review"),
            "error should name the offending group, got: {message}"
        );
        assert!(
            !repo.join("out.txt").exists(),
            "no step should have executed"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_exec_dry_run_also_fails_fast_when_group_max_retries_unreachable() {
        // Given: the same offending config, but requested via --dry-run
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let config_path = tmp.path().join("cruise.yaml");
        fs::write(&config_path, group_retry_budget_config_with(5))
            .unwrap_or_else(|e| panic!("{e:?}"));

        let mut args = exec_args_no_op(&config_path);
        args.dry_run = true;

        // When: exec is called with --dry-run
        let result = run(args).await;

        // Then: --dry-run does not bypass the fail-fast validation
        assert!(
            result.is_err(),
            "dry-run should still surface the unreachable group max_retries error: {result:?}"
        );
        assert!(
            !repo.join("out.txt").exists(),
            "dry-run must not execute any step"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_exec_succeeds_when_group_max_retries_equals_ceiling() {
        // Given: a config whose group max_retries exactly equals the effective
        // ceiling of 3 (R == G, reachable)
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let config_path = tmp.path().join("cruise.yaml");
        fs::write(&config_path, group_retry_budget_config_with(3))
            .unwrap_or_else(|e| panic!("{e:?}"));

        // When: exec is called (non-dry-run)
        let result = run(exec_args_no_op(&config_path)).await;

        // Then: R == G is reachable, so validation passes and steps execute
        assert!(
            result.is_ok(),
            "R == G should be accepted, not rejected as unreachable: {result:?}"
        );
        assert!(
            repo.join("out.txt").exists(),
            "the workflow should have executed past validation"
        );
    }
}
