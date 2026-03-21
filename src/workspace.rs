use std::path::{Path, PathBuf};
use std::process::Command;

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
/// expected branch and clean for a fresh run.
///
/// # Errors
///
/// Returns an error if worktree setup fails, if the current-branch session is
/// on a different branch than expected, if `HEAD` is detached, or if a fresh
/// current-branch run starts from a dirty working tree.
pub fn prepare_execution_workspace(
    manager: &SessionManager,
    session: &mut SessionState,
    workspace_mode: WorkspaceMode,
) -> Result<ExecutionWorkspace> {
    match workspace_mode {
        WorkspaceMode::Worktree => {
            let worktrees_dir = manager.worktrees_dir();
            let (ctx, reused) = worktree::setup_session_worktree(
                &session.base_dir,
                &session.id,
                &session.input,
                &worktrees_dir,
                session.worktree_branch.as_deref(),
            )?;
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
        return Err(CruiseError::Other(
            "current-branch mode requires a clean working tree, but the repository is dirty"
                .to_string(),
        ));
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
        &["status", "--porcelain"],
        "git status --porcelain failed",
    )?
    .is_empty())
}

fn git_stdout(repo_dir: &Path, args: &[&str], context: &str) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_dir)
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
    use crate::test_support::{init_git_repo, make_session, run_git_ok};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_prepare_execution_workspace_worktree_mode_creates_session_worktree() {
        // Given: a planned session in a clean git repository
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_home = tmp.path().join(".cruise");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        let manager = SessionManager::new(cruise_home);
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
        // Given: a fresh session targeting the current branch
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_home = tmp.path().join(".cruise");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        let manager = SessionManager::new(cruise_home);
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
        // Given: a fresh current-branch session with uncommitted changes in the base repo
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_home = tmp.path().join(".cruise");
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        fs::write(repo.join("dirty.txt"), "dirty").unwrap_or_else(|e| panic!("{e:?}"));
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
    fn test_prepare_execution_workspace_current_branch_mode_rejects_detached_head() {
        // Given: a fresh current-branch session on a detached HEAD
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_home = tmp.path().join(".cruise");
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
