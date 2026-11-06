use std::path::{Path, PathBuf};
use std::process::Command;

use console::style;

use crate::error::{CruiseError, Result};
use crate::session::{SessionManager, SessionState, WorkspaceMode};
use crate::worktree;

#[derive(Debug, Clone)]
pub enum ExecutionWorkspace {
    Worktree {
        ctx: worktree::WorktreeContext,
        reused: bool,
    },
    CurrentBranch {
        path: PathBuf,
    },
}

impl ExecutionWorkspace {
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Worktree { ctx, .. } => &ctx.path,
            Self::CurrentBranch { path } => path,
        }
    }
}

/// Prepare the filesystem location where a session should execute.
///
/// In worktree mode this creates or reuses the session worktree. In
/// current-branch mode this validates that the base repository is on the
/// expected branch and clean for a fresh run unless dirty execution is
/// explicitly allowed by the session.
///
/// # Errors
///
/// Returns an error if worktree setup fails, if the current-branch session is
/// on a different branch than expected, if `HEAD` is detached, or if a fresh
/// current-branch run starts from a dirty working tree without permission.
pub fn prepare_execution_workspace(
    manager: &SessionManager,
    session: &mut SessionState,
    workspace_mode: WorkspaceMode,
) -> Result<ExecutionWorkspace> {
    match workspace_mode {
        WorkspaceMode::Worktree => {
            let (ctx, reused) = if let Some(target) = &session.pr_target {
                crate::pr_review_fix::validate_origin(target, &session.base_dir)?;
                worktree::setup_pr_worktree(
                    &session.base_dir,
                    &session.id,
                    &manager.worktrees_dir(),
                    &target.head_branch,
                )?
            } else {
                setup_session_worktree(manager, session)?
            };
            Ok(ExecutionWorkspace::Worktree { ctx, reused })
        }
        WorkspaceMode::CurrentBranch => {
            validate_current_branch_session(session)?;
            Ok(ExecutionWorkspace::CurrentBranch {
                path: session.base_dir.clone(),
            })
        }
    }
}

pub(crate) fn setup_session_worktree(
    manager: &SessionManager,
    session: &mut SessionState,
) -> Result<(worktree::WorktreeContext, bool)> {
    let worktrees_dir = manager.worktrees_dir();
    let base = if worktrees_dir.join(&session.id).is_dir() {
        worktree::WorktreeBase::default()
    } else {
        crate::workflow_defaults::WorkflowDefaults::load(Some(&session.base_dir))?.worktree_base()
    };
    let (ctx, reused, selected_ref) = worktree::setup_session_worktree_with_ref(
        &session.base_dir,
        &session.id,
        &session.worktree_branch_source(),
        &worktrees_dir,
        session.worktree_branch.as_deref(),
        &base,
    )?;
    if !reused {
        session.worktree_base_branch = base.branch;
        session.worktree_base_ref = selected_ref;
    }
    Ok((ctx, reused))
}

pub fn update_session_workspace(session: &mut SessionState, workspace: &ExecutionWorkspace) {
    match workspace {
        ExecutionWorkspace::Worktree { ctx, .. } => {
            session.worktree_path = Some(ctx.path.clone());
            session.worktree_branch = Some(ctx.branch.clone());
        }
        ExecutionWorkspace::CurrentBranch { .. } => {
            session.worktree_path = None;
            session.worktree_branch = None;
            session.pr_url = None;
        }
    }
}

fn validate_current_branch_session(session: &mut SessionState) -> Result<()> {
    let current_branch = current_branch_name(&session.base_dir)?;

    match session.target_branch.as_deref() {
        Some(target_branch) if current_branch != target_branch => {
            return Err(CruiseError::Other(format!(
                "current-branch mode expected branch `{target_branch}`, but found `{current_branch}`"
            )));
        }
        None => {
            session.target_branch = Some(current_branch);
        }
        _ => {}
    }

    if session.current_step.is_none() && is_working_tree_dirty(&session.base_dir)? {
        if !session.allow_dirty_working_tree {
            return Err(CruiseError::Other(
                "current-branch mode requires a clean working tree, but the repository is dirty"
                    .to_string(),
            ));
        }
        crate::status_eprintln!(
            "{} working tree is dirty; running on top of the existing uncommitted changes",
            style("!").yellow().bold()
        );
    }

    Ok(())
}

fn current_branch_name(repo_dir: &Path) -> Result<String> {
    let branch = git_stdout(
        repo_dir,
        &["rev-parse", "--abbrev-ref", "HEAD"],
        "git rev-parse --abbrev-ref HEAD failed",
    )?;

    if branch == "HEAD" {
        return Err(CruiseError::Other(
            "current-branch mode requires an attached branch; HEAD is detached".to_string(),
        ));
    }

    Ok(branch)
}

fn is_working_tree_dirty(repo_dir: &Path) -> Result<bool> {
    Ok(!git_stdout(
        repo_dir,
        &["status", "--porcelain", "--untracked-files=no"],
        "git status --porcelain failed",
    )?
    .is_empty())
}

fn git_stdout(repo_dir: &Path, args: &[&str], context: &str) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git {}: {e}", args.join(" "))))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::Other(format!("{context}: {}", stderr.trim())));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, init_git_repo, lock_process, make_session, run_git_ok};
    use std::fs;
    use tempfile::TempDir;

    fn base_test_repo(tmp: &TempDir, defaults: &str) -> (PathBuf, SessionManager) {
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo)
            .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        init_git_repo(&repo);
        fs::create_dir_all(repo.join(".cruise"))
            .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        fs::write(repo.join(".cruise/default.yaml"), defaults)
            .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        (repo, SessionManager::new(tmp.path().join("data")))
    }

    fn head(repo: &Path, revision: &str) -> String {
        git_stdout(repo, &["rev-parse", revision], "rev-parse failed")
            .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
    }

    #[test]
    fn test_worktree_base_config_selects_local_branch() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        let _config = EnvGuard::set("XDG_CONFIG_HOME", tmp.path().join("config"));
        let (repo, manager) = base_test_repo(&tmp, "base_branch: main\nauto_fetch: false\n");
        run_git_ok(&repo, &["update-ref", "refs/remotes/origin/main", "main"]);
        run_git_ok(
            &repo,
            &["commit", "--allow-empty", "-m", "advance local base"],
        );
        let main = head(&repo, "main");
        run_git_ok(&repo, &["checkout", "-b", "feature"]);
        run_git_ok(&repo, &["commit", "--allow-empty", "-m", "feature"]);
        let mut session = make_session("20260321120101", &repo);
        let workspace =
            prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree)
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        assert_eq!(head(workspace.path(), "HEAD"), main);
        assert_eq!(
            session.worktree_base_ref.as_deref(),
            Some("refs/heads/main")
        );
    }

    #[test]
    fn test_worktree_base_fetch_updates_remote_and_uses_it() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        let _config = EnvGuard::set("XDG_CONFIG_HOME", tmp.path().join("config"));
        let (repo, manager) = base_test_repo(&tmp, "base_branch: main\nauto_fetch: true\n");
        let remote = tmp.path().join("remote.git");
        run_git_ok(
            tmp.path(),
            &[
                "clone",
                "--bare",
                repo.to_str()
                    .unwrap_or_else(|| panic!("test path must be UTF-8")),
                remote
                    .to_str()
                    .unwrap_or_else(|| panic!("test path must be UTF-8")),
            ],
        );
        run_git_ok(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                remote
                    .to_str()
                    .unwrap_or_else(|| panic!("test path must be UTF-8")),
            ],
        );
        run_git_ok(&repo, &["fetch", "origin", "main"]);
        let old = head(&repo, "main");
        let writer = tmp.path().join("writer");
        run_git_ok(
            tmp.path(),
            &[
                "clone",
                remote
                    .to_str()
                    .unwrap_or_else(|| panic!("test path must be UTF-8")),
                writer
                    .to_str()
                    .unwrap_or_else(|| panic!("test path must be UTF-8")),
            ],
        );
        run_git_ok(&writer, &["config", "user.email", "test@example.com"]);
        run_git_ok(&writer, &["config", "user.name", "Test"]);
        run_git_ok(&writer, &["commit", "--allow-empty", "-m", "advance"]);
        run_git_ok(&writer, &["push", "origin", "main"]);
        let new = head(&writer, "HEAD");
        let mut session = make_session("20260321120102", &repo);
        let workspace =
            prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree)
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        assert_eq!(head(&repo, "main"), old);
        assert_eq!(head(workspace.path(), "HEAD"), new);
        assert_eq!(head(&repo, "refs/remotes/origin/main"), new);
        assert_eq!(
            session.worktree_base_ref.as_deref(),
            Some("refs/remotes/origin/main")
        );
    }

    #[test]
    fn test_worktree_base_missing_branch_is_rejected() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        let _config = EnvGuard::set("XDG_CONFIG_HOME", tmp.path().join("config"));
        let (repo, manager) = base_test_repo(&tmp, "base_branch: nonexistent\n");
        let mut session = make_session("20260321120103", &repo);
        let result = prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree);
        assert!(
            matches!(result, Err(CruiseError::WorktreeError(ref message)) if message.contains("base branch does not exist: nonexistent")),
            "{result:?}"
        );
        assert!(!manager.worktrees_dir().join(&session.id).exists());
    }

    #[test]
    fn test_worktree_base_invalid_name_is_rejected() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        let _config = EnvGuard::set("XDG_CONFIG_HOME", tmp.path().join("config"));
        let (repo, manager) = base_test_repo(&tmp, "base_branch: '-x'\nauto_fetch: true\n");
        let mut session = make_session("20260321120104", &repo);
        let result = prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree);
        assert!(
            matches!(result, Err(CruiseError::WorktreeError(ref message)) if message.contains("invalid base branch")),
            "{result:?}"
        );
        assert!(!manager.worktrees_dir().join(&session.id).exists());
    }

    #[test]
    fn test_worktree_base_fetch_failure_falls_back_to_local() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        let _config = EnvGuard::set("XDG_CONFIG_HOME", tmp.path().join("config"));
        let (repo, manager) = base_test_repo(&tmp, "base_branch: main\nauto_fetch: true\n");
        run_git_ok(&repo, &["update-ref", "refs/remotes/origin/main", "main"]);
        run_git_ok(
            &repo,
            &["commit", "--allow-empty", "-m", "advance local base"],
        );
        let main = head(&repo, "main");
        run_git_ok(&repo, &["checkout", "-b", "feature"]);
        run_git_ok(&repo, &["commit", "--allow-empty", "-m", "feature"]);
        let mut session = make_session("20260321120105", &repo);
        let workspace =
            prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree)
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        assert_eq!(head(workspace.path(), "HEAD"), main);
        assert_eq!(
            session.worktree_base_ref.as_deref(),
            Some("refs/heads/main")
        );
    }

    #[test]
    fn test_worktree_base_uses_remote_when_local_branch_missing() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        let _config = EnvGuard::set("XDG_CONFIG_HOME", tmp.path().join("config"));
        let (repo, manager) = base_test_repo(&tmp, "base_branch: main\nauto_fetch: false\n");
        let main = head(&repo, "main");
        run_git_ok(&repo, &["update-ref", "refs/remotes/origin/main", &main]);
        run_git_ok(&repo, &["checkout", "-b", "feature"]);
        run_git_ok(&repo, &["commit", "--allow-empty", "-m", "feature"]);
        run_git_ok(&repo, &["branch", "-D", "main"]);
        let mut session = make_session("20260321120106", &repo);
        let workspace =
            prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree)
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        assert_eq!(head(workspace.path(), "HEAD"), main);
        assert_eq!(
            session.worktree_base_ref.as_deref(),
            Some("refs/remotes/origin/main")
        );
    }

    #[test]
    fn test_worktree_base_reuse_preserves_recorded_base() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        let _config = EnvGuard::set("XDG_CONFIG_HOME", tmp.path().join("config"));
        let (repo, manager) = base_test_repo(&tmp, "base_branch: main\n");
        let mut session = make_session("20260321120107", &repo);
        let workspace =
            prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree)
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        update_session_workspace(&mut session, &workspace);
        assert_eq!(
            serde_json::to_value(&session)
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))["worktree_base_branch"],
            "main"
        );
        fs::write(
            repo.join(".cruise/default.yaml"),
            "base_branch: '-x'\nauto_fetch: true\n",
        )
        .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        let reused = prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree)
            .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        assert!(matches!(
            reused,
            ExecutionWorkspace::Worktree { reused: true, .. }
        ));
        assert_eq!(reused.path(), workspace.path());
        assert_eq!(
            session.worktree_base_ref.as_deref(),
            Some("refs/heads/main")
        );
        assert_eq!(
            serde_json::to_value(&session)
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))["worktree_base_branch"],
            "main"
        );
    }

    #[test]
    fn planned_issue_session_uses_issue_number_for_worktree_branch() {
        let _lock = lock_process();
        let tmp =
            TempDir::new().unwrap_or_else(|error| panic!("create temporary directory: {error}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("create repo: {error}"));
        init_git_repo(&repo);
        let manager = SessionManager::new(tmp.path().join("data"));
        let mut session = make_session("issue-branch", &repo);
        session.input = "# Formatted issue title\n\nIssue #12: details".into();
        let mut json = serde_json::to_value(session)
            .unwrap_or_else(|error| panic!("serialize session: {error}"));
        json["issue_number"] = serde_json::json!(12);
        let mut session = serde_json::from_value(json)
            .unwrap_or_else(|error| panic!("deserialize session: {error}"));
        let workspace =
            prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree)
                .unwrap_or_else(|error| panic!("prepare workspace: {error}"));
        match workspace {
            ExecutionWorkspace::Worktree { ctx, .. } => {
                assert_eq!(ctx.branch, "cruise/issue-branch-issue-12");
            }
            ExecutionWorkspace::CurrentBranch { .. } => panic!("expected worktree"),
        }
    }

    #[test]
    fn pr_workspace_rejects_clone_origin_mismatch_before_fetch_or_worktree() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|error| panic!("create temp dir: {error}"));
        let source = tmp.path().join("source");
        fs::create_dir_all(&source).unwrap_or_else(|error| panic!("create source dir: {error}"));
        init_git_repo(&source);
        let clone = tmp.path().join("clone");
        assert!(
            Command::new("git")
                .arg("clone")
                .arg(&source)
                .arg(&clone)
                .status()
                .unwrap_or_else(|error| panic!("clone repository: {error}"))
                .success()
        );
        run_git_ok(
            &clone,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/other/repo.git",
            ],
        );
        let manager = SessionManager::new(tmp.path().join("data"));
        let mut session = make_session("pr-mismatch", &clone);
        session.pr_target = Some(crate::pr_review_fix::PrTarget {
            number: 12,
            title: "Fix".into(),
            url: "https://github.com/owner/repo/pull/12".into(),
            head_branch: "main".into(),
            base_branch: "main".into(),
        });
        let fetch_head = clone.join(".git/FETCH_HEAD");
        let before = fs::read(&fetch_head).ok();
        let error = prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree)
            .err()
            .unwrap_or_else(|| panic!("expected origin mismatch"));
        assert!(
            error
                .to_string()
                .contains("origin fetch repository does not match")
        );
        assert_eq!(fs::read(&fetch_head).ok(), before);
        assert!(!manager.worktrees_dir().join("pr-mismatch").exists());

        run_git_ok(
            &clone,
            &[
                "remote",
                "set-url",
                "origin",
                "git@github.com:owner/repo.git",
            ],
        );
        assert!(
            crate::pr_review_fix::validate_origin(
                session
                    .pr_target
                    .as_ref()
                    .unwrap_or_else(|| panic!("missing PR target")),
                &clone
            )
            .is_ok()
        );
        run_git_ok(
            &clone,
            &[
                "remote",
                "set-url",
                "--push",
                "origin",
                "https://github.com/other/repo.git",
            ],
        );
        let error = prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree)
            .err()
            .unwrap_or_else(|| panic!("expected origin mismatch"));
        assert!(
            error
                .to_string()
                .contains("origin push repository does not match")
        );
        assert_eq!(fs::read(&fetch_head).ok(), before);
        assert!(!manager.worktrees_dir().join("pr-mismatch").exists());
    }

    #[test]
    fn test_prepare_execution_workspace_worktree_mode_creates_session_worktree() {
        let _lock = lock_process();
        // Given: a planned session in a clean git repository
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let data_dir = tmp.path().join(".local").join("share").join("cruise");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        let manager = SessionManager::new(data_dir);
        let mut session = make_session("20260321120000", &repo);

        // When: preparing a worktree execution workspace
        let workspace =
            prepare_execution_workspace(&manager, &mut session, WorkspaceMode::Worktree)
                .unwrap_or_else(|e| panic!("{e:?}"));
        update_session_workspace(&mut session, &workspace);

        // Then: the session points at the created worktree and execution runs there
        match &workspace {
            ExecutionWorkspace::Worktree { ctx, reused } => {
                assert!(!reused, "fresh runs should create a new worktree");
                assert!(ctx.path.exists(), "worktree path should exist");
                assert_eq!(workspace.path(), ctx.path.as_path());
                assert_eq!(session.worktree_path.as_deref(), Some(ctx.path.as_path()));
                assert_eq!(
                    session.worktree_branch.as_deref(),
                    Some(ctx.branch.as_str())
                );
            }
            ExecutionWorkspace::CurrentBranch { path } => {
                panic!(
                    "expected worktree workspace, got current branch at {}",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn test_prepare_execution_workspace_current_branch_mode_uses_base_repo_and_sets_target_branch()
    {
        let _lock = lock_process();
        // Given: a fresh session targeting the current branch
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let data_dir = tmp.path().join(".local").join("share").join("cruise");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        let manager = SessionManager::new(data_dir);
        let mut session = make_session("20260321120001", &repo);

        // When: preparing a current-branch execution workspace
        let workspace =
            prepare_execution_workspace(&manager, &mut session, WorkspaceMode::CurrentBranch)
                .unwrap_or_else(|e| panic!("{e:?}"));
        update_session_workspace(&mut session, &workspace);

        // Then: execution stays in the base repository and remembers the branch
        match &workspace {
            ExecutionWorkspace::CurrentBranch { path } => {
                assert_eq!(path, &repo);
                assert_eq!(workspace.path(), repo.as_path());
                assert_eq!(session.target_branch.as_deref(), Some("main"));
                assert!(session.worktree_path.is_none());
                assert!(session.worktree_branch.is_none());
            }
            ExecutionWorkspace::Worktree { ctx, .. } => {
                panic!(
                    "expected current branch workspace, got {}",
                    ctx.path.display()
                );
            }
        }
    }

    #[test]
    fn test_prepare_execution_workspace_current_branch_mode_rejects_dirty_tree_on_fresh_run() {
        let _lock = lock_process();
        // Given: a fresh current-branch session with uncommitted tracked changes in the base repo
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_home = tmp.path().join(".local").join("share").join("cruise");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        // Modify the committed README.md so the working tree has tracked changes.
        fs::write(repo.join("README.md"), "modified").unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(cruise_home);
        let mut session = make_session("20260321120002", &repo);

        // When: preparing the current-branch workspace
        let error =
            prepare_execution_workspace(&manager, &mut session, WorkspaceMode::CurrentBranch)
                .map_or_else(|e| e, |_| panic!("expected dirty tree to be rejected"));

        // Then: the dirty-tree validation explains why execution is blocked
        assert!(
            error.to_string().contains("dirty"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_prepare_execution_workspace_current_branch_mode_allows_dirty_tree_when_flag_set() {
        let _lock = lock_process();
        // Given: a fresh current-branch session that explicitly allows tracked changes
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_home = tmp.path().join(".local").join("share").join("cruise");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        fs::write(repo.join("README.md"), "modified").unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(cruise_home);
        let mut session = make_session("20260321120004", &repo);
        session.allow_dirty_working_tree = true;

        // When: preparing the current-branch workspace
        let workspace =
            prepare_execution_workspace(&manager, &mut session, WorkspaceMode::CurrentBranch)
                .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: execution stays in the base repository and captures its branch
        match &workspace {
            ExecutionWorkspace::CurrentBranch { path } => {
                assert_eq!(path, &repo);
                assert_eq!(session.target_branch.as_deref(), Some("main"));
            }
            ExecutionWorkspace::Worktree { ctx, .. } => {
                panic!(
                    "expected current branch workspace, got {}",
                    ctx.path.display()
                );
            }
        }
    }

    #[test]
    fn test_prepare_execution_workspace_current_branch_mode_rejects_detached_head() {
        let _lock = lock_process();
        // Given: a fresh current-branch session on a detached HEAD
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_home = tmp.path().join(".local").join("share").join("cruise");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        run_git_ok(&repo, &["checkout", "--detach"]);
        let manager = SessionManager::new(cruise_home);
        let mut session = make_session("20260321120003", &repo);

        // When: preparing the current-branch workspace
        let error =
            prepare_execution_workspace(&manager, &mut session, WorkspaceMode::CurrentBranch)
                .map_or_else(|e| e, |_| panic!("expected detached HEAD to be rejected"));

        // Then: the error tells the caller that an attached branch is required
        assert!(
            error.to_string().contains("detached"),
            "unexpected error: {error}"
        );
    }
}
