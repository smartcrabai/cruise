/// Temporary GitHub repository clones for `--repo` sessions.
///
/// A repo-backed session has no permanent local checkout: the repository is
/// cloned into `<data_dir>/clones/{session_id}/` for planning, removed once the
/// plan is approved, cloned again for execution, and removed after the PR has
/// been created. The clone directory doubles as the session's `base_dir`, so
/// the existing worktree + PR machinery works on top of it unchanged.
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{CruiseError, Result};
use crate::session::{SessionManager, SessionState};

/// Validate an `owner/repository` GitHub repo spec.
///
/// # Errors
///
/// Returns an error if `spec` is not of the form `<owner>/<repository>` with
/// both parts non-empty and limited to alphanumerics, `-`, `_` and `.`.
/// Leading dashes are rejected so the spec can never be parsed as a flag by
/// `gh`, and dot-only parts (`.`, `..`) are rejected as never-valid names.
pub fn validate_repo_spec(spec: &str) -> Result<()> {
    fn valid_part(part: &str) -> bool {
        !part.is_empty()
            && !part.starts_with('-')
            && part.chars().any(|c| c != '.')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    }

    let valid = spec
        .split_once('/')
        .is_some_and(|(owner, name)| valid_part(owner) && valid_part(name));

    if valid {
        Ok(())
    } else {
        Err(CruiseError::Other(format!(
            "invalid repository '{spec}': expected <owner>/<repository>"
        )))
    }
}

/// Clone `repo` into `clone_path` using `gh repo clone` (so `gh`'s
/// authentication is reused). A partially created directory is removed on
/// failure.
///
/// # Errors
///
/// Returns an error if `gh` cannot be spawned or the clone fails.
pub fn clone_repo(repo: &str, clone_path: &Path) -> Result<()> {
    if let Some(parent) = clone_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let output = Command::new("gh")
        .args(["repo", "clone", repo])
        .arg(clone_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh repo clone: {e}")))?;

    if !output.status.success() {
        let _ = std::fs::remove_dir_all(clone_path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::Other(format!(
            "gh repo clone {} failed: {}",
            repo,
            stderr.trim()
        )));
    }
    Ok(())
}

/// Config path to persist on a repo-backed session.
///
/// Configs that live inside the temporary clone return `None`: the caller is
/// expected to copy the YAML into the session directory instead, so it stays
/// readable after the clone is removed.
#[must_use]
pub fn persistent_config_path(
    source: &crate::resolver::ConfigSource,
    clone_path: &Path,
) -> Option<PathBuf> {
    source
        .path()
        .filter(|p| !p.starts_with(clone_path))
        .cloned()
}

/// Ensure the session's temporary clone exists at
/// `<data_dir>/clones/{session_id}/`, cloning it if necessary.
///
/// Returns the clone path and whether an existing clone was reused (e.g. when
/// resuming a suspended session).
///
/// # Errors
///
/// Returns an error if the session has no repository or the clone fails.
pub fn ensure_session_clone(
    manager: &SessionManager,
    session: &SessionState,
) -> Result<(PathBuf, bool)> {
    let repo = session.repo.as_deref().ok_or_else(|| {
        CruiseError::Other(format!("session {} has no repository to clone", session.id))
    })?;
    let clone_path = manager.clones_dir().join(&session.id);
    if clone_path.is_dir() {
        if clone_path.join(".git").exists() {
            return Ok((clone_path, true));
        }
        // A previous clone attempt was interrupted and left a partial
        // directory behind; discard it and clone again.
        std::fs::remove_dir_all(&clone_path)?;
    }
    clone_repo(repo, &clone_path)?;
    Ok((clone_path, false))
}

/// Re-create the temporary clone for a repo-backed session if it is missing
/// and point `base_dir` at it. No-op for sessions without a repository.
///
/// Returns `true` when a fresh clone was created. The caller is responsible
/// for persisting the session if it cares about the updated `base_dir`.
///
/// # Errors
///
/// Returns an error if the clone fails.
pub fn ensure_repo_session_workspace(
    manager: &SessionManager,
    session: &mut SessionState,
) -> Result<bool> {
    if session.repo.is_none() {
        return Ok(false);
    }
    let (clone_path, reused) = ensure_session_clone(manager, session)?;
    session.base_dir = clone_path;
    Ok(!reused)
}

/// Remove the session's temporary clone and any worktree created on top of it.
///
/// The worktree is removed first because its metadata lives inside the clone's
/// `.git` directory. All failures are logged as warnings -- cleanup is
/// best-effort.
pub fn cleanup_session_workspace(manager: &SessionManager, session: &SessionState) {
    if let Some(ctx) = session.worktree_context() {
        if ctx.original_dir.is_dir()
            && let Err(e) = crate::worktree::cleanup_worktree(&ctx)
        {
            eprintln!(
                "warning: failed to remove worktree for {}: {}",
                session.id, e
            );
        }
        if ctx.path.exists()
            && let Err(e) = std::fs::remove_dir_all(&ctx.path)
        {
            eprintln!(
                "warning: failed to remove worktree directory for {}: {}",
                session.id, e
            );
        }
    }

    let clone_path = manager.clones_dir().join(&session.id);
    if clone_path.exists()
        && let Err(e) = std::fs::remove_dir_all(&clone_path)
    {
        eprintln!("warning: failed to remove clone for {}: {}", session.id, e);
    }
}

/// Post-approval cleanup for repo-backed sessions: the temporary clone (and
/// any planning worktree on top of it) is removed; execution re-clones later.
///
/// Clears `worktree_path` on the session (the branch name is kept so the
/// execution worktree reuses it). No-op for sessions without a repository.
/// The caller is responsible for persisting the session afterwards.
pub fn cleanup_after_approval(manager: &SessionManager, session: &mut SessionState) {
    if session.repo.is_none() {
        return;
    }
    cleanup_session_workspace(manager, session);
    session.worktree_path = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::make_session;
    use tempfile::TempDir;

    #[test]
    fn test_validate_repo_spec_accepts_owner_slash_repo() {
        for spec in [
            "owner/repo",
            "smart-crab.ai/some_repo",
            "a/b",
            "Owner123/repo.name",
        ] {
            assert!(
                validate_repo_spec(spec).is_ok(),
                "expected '{spec}' to be valid"
            );
        }
    }

    #[test]
    fn test_validate_repo_spec_rejects_invalid_specs() {
        for spec in [
            "",
            "owner",
            "/repo",
            "owner/",
            "owner/repo/extra",
            "owner repo/x",
            "owner/repo name",
            "https://github.com/owner/repo",
            "-u/repo",
            "owner/-repo",
            "./repo",
            "owner/..",
            "../repo",
        ] {
            assert!(
                validate_repo_spec(spec).is_err(),
                "expected '{spec}' to be rejected"
            );
        }
    }

    #[test]
    fn test_ensure_session_clone_requires_repo() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let session = make_session("20260607100000", tmp.path());

        let err = ensure_session_clone(&manager, &session)
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        assert!(err.to_string().contains("no repository"));
    }

    #[test]
    fn test_ensure_session_clone_reuses_existing_directory() {
        // Given: a repo session whose clone directory already exists
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let mut session = make_session("20260607100001", tmp.path());
        session.repo = Some("owner/repo".to_string());
        let clone_path = manager.clones_dir().join(&session.id);
        std::fs::create_dir_all(clone_path.join(".git")).unwrap_or_else(|e| panic!("{e:?}"));

        // When: ensuring the clone
        let (path, reused) =
            ensure_session_clone(&manager, &session).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the existing directory is reused without invoking gh
        assert!(reused);
        assert_eq!(path, clone_path);
    }

    #[test]
    fn test_cleanup_session_workspace_removes_clone_dir() {
        // Given: a repo session with an on-disk clone directory
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let mut session = make_session("20260607100002", tmp.path());
        session.repo = Some("owner/repo".to_string());
        let clone_path = manager.clones_dir().join(&session.id);
        std::fs::create_dir_all(&clone_path).unwrap_or_else(|e| panic!("{e:?}"));
        std::fs::write(clone_path.join("file.txt"), "x").unwrap_or_else(|e| panic!("{e:?}"));

        // When: cleaning up the workspace
        cleanup_session_workspace(&manager, &session);

        // Then: the clone directory is gone
        assert!(!clone_path.exists());
    }

    #[test]
    fn test_cleanup_after_approval_noop_without_repo() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let mut session = make_session("20260607100003", tmp.path());
        session.worktree_path = Some(tmp.path().join("wt"));

        cleanup_after_approval(&manager, &mut session);

        // Non-repo sessions keep their planning worktree after approval.
        assert!(session.worktree_path.is_some());
    }

    #[test]
    fn test_cleanup_after_approval_removes_clone_and_clears_worktree_path() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().to_path_buf());
        let mut session = make_session("20260607100004", tmp.path());
        session.repo = Some("owner/repo".to_string());
        session.worktree_branch = Some("cruise/20260607100004-task".to_string());
        let clone_path = manager.clones_dir().join(&session.id);
        std::fs::create_dir_all(&clone_path).unwrap_or_else(|e| panic!("{e:?}"));

        cleanup_after_approval(&manager, &mut session);

        assert!(!clone_path.exists());
        assert!(session.worktree_path.is_none());
        // Branch name is kept so the execution worktree reuses it.
        assert_eq!(
            session.worktree_branch.as_deref(),
            Some("cruise/20260607100004-task")
        );
    }
}
