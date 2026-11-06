use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use crate::error::{CruiseError, Result};

static WORKTREE_BASE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeBase {
    pub branch: Option<String>,
    pub auto_fetch: bool,
}

#[derive(Debug, Clone)]
pub struct WorktreeContext {
    pub path: PathBuf,
    pub branch: String,
    pub original_dir: PathBuf,
    pub owns_branch: bool,
}

/// Generate the default branch name for a session worktree.
fn default_branch_name(session_id: &str, input: &str) -> String {
    if input.is_empty() {
        format!("cruise/{session_id}")
    } else {
        let sanitized = sanitize_branch_name(input);
        if sanitized.is_empty() {
            format!("cruise/{session_id}")
        } else {
            format!("cruise/{session_id}-{sanitized}")
        }
    }
}

/// Create or reuse a git worktree at `~/.cruise/worktrees/{session_id}/`.
///
/// If the worktree directory already exists (e.g. resuming a session),
/// it is reused. `existing_branch` overrides the branch name when reusing.
///
/// # Errors
///
/// Returns an error if `base_dir` is not a git repository, the git worktree
/// command fails, or file I/O fails while copying worktree includes.
pub fn setup_session_worktree(
    base_dir: &Path,
    session_id: &str,
    input: &str,
    worktrees_dir: &Path,
    existing_branch: Option<&str>,
    base: &WorktreeBase,
) -> Result<(WorktreeContext, bool)> {
    let (ctx, reused, _) = setup_session_worktree_with_ref(
        base_dir,
        session_id,
        input,
        worktrees_dir,
        existing_branch,
        base,
    )?;
    Ok((ctx, reused))
}

pub(crate) fn setup_session_worktree_with_ref(
    base_dir: &Path,
    session_id: &str,
    input: &str,
    worktrees_dir: &Path,
    existing_branch: Option<&str>,
    base: &WorktreeBase,
) -> Result<(WorktreeContext, bool, Option<String>)> {
    ensure_git_repo(base_dir)?;

    let worktree_path = worktrees_dir.join(session_id);

    // Reuse existing worktree directory if present.
    if worktree_path.is_dir() {
        let branch = existing_branch.map_or_else(
            || default_branch_name(session_id, input),
            std::string::ToString::to_string,
        );
        return Ok((
            WorktreeContext {
                path: worktree_path,
                branch,
                original_dir: base_dir.to_path_buf(),
                owns_branch: true,
            },
            true,
            None,
        ));
    }

    let branch = default_branch_name(session_id, input);
    fs::create_dir_all(worktrees_dir)?;

    let guard = WORKTREE_BASE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let start = resolve_start_point(base_dir, base)?;
    let mut command = Command::new("git");
    command
        .args(["worktree", "add", "-b", &branch])
        .arg(&worktree_path);
    if let Some(ref start) = start {
        command.arg(start);
    }
    let output = command
        .current_dir(base_dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::WorktreeError(format!(
            "git worktree add failed: {}",
            stderr.trim()
        )));
    }
    drop(guard);

    copy_worktree_includes(base_dir, &worktree_path)?;

    Ok((
        WorktreeContext {
            path: worktree_path,
            branch,
            original_dir: base_dir.to_path_buf(),
            owns_branch: true,
        },
        false,
        start,
    ))
}

/// Create or reuse a worktree on an existing PR head without resetting its branch.
pub fn setup_pr_worktree(
    base_dir: &Path,
    session_id: &str,
    worktrees_dir: &Path,
    head_branch: &str,
) -> Result<(WorktreeContext, bool)> {
    ensure_git_repo(base_dir)?;
    let valid = Command::new("git")
        .args(["check-ref-format", "--branch", head_branch])
        .current_dir(base_dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {e}")))?;
    if !valid.status.success() || head_branch.starts_with('-') {
        return Err(CruiseError::WorktreeError(format!(
            "invalid PR head branch: {head_branch}"
        )));
    }
    let path = worktrees_dir.join(session_id);
    let ctx = WorktreeContext {
        path: path.clone(),
        branch: head_branch.to_string(),
        original_dir: base_dir.to_path_buf(),
        owns_branch: false,
    };
    let refspec = format!("+refs/heads/{head_branch}:refs/remotes/origin/{head_branch}");
    git_output(base_dir, &["fetch", "origin", &refspec])?;
    if path.is_dir() {
        let actual = git_output(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        if actual.trim() != head_branch {
            return Err(CruiseError::WorktreeError(format!(
                "existing worktree is not on PR head branch {head_branch}"
            )));
        }
        git_output(
            base_dir,
            &[
                "branch",
                "--set-upstream-to",
                &format!("origin/{head_branch}"),
                head_branch,
            ],
        )?;
        return Ok((ctx, true));
    }

    let local_ref = format!("refs/heads/{head_branch}");
    let remote_ref = format!("refs/remotes/origin/{head_branch}");
    let exists = git_status(base_dir, &["show-ref", "--verify", "--quiet", &local_ref])?;
    let mut fast_forward = false;
    if exists {
        if !git_status(
            base_dir,
            &["merge-base", "--is-ancestor", &remote_ref, &local_ref],
        )? {
            if git_status(
                base_dir,
                &["merge-base", "--is-ancestor", &local_ref, &remote_ref],
            )? {
                fast_forward = true;
            } else {
                return Err(CruiseError::WorktreeError(format!(
                    "local branch {head_branch} has diverged from origin/{head_branch}"
                )));
            }
        }
    }
    fs::create_dir_all(worktrees_dir)?;
    let path_arg = path.to_str().ok_or_else(|| {
        CruiseError::WorktreeError("worktree path is not valid UTF-8".to_string())
    })?;
    if exists {
        git_output(base_dir, &["worktree", "add", path_arg, head_branch])?;
    } else {
        git_output(
            base_dir,
            &[
                "worktree",
                "add",
                "--track",
                "-b",
                head_branch,
                path_arg,
                &remote_ref,
            ],
        )?;
    }
    let setup = (|| {
        if fast_forward {
            git_output(&path, &["merge", "--ff-only", &remote_ref])?;
        }
        git_output(
            base_dir,
            &[
                "branch",
                "--set-upstream-to",
                &format!("origin/{head_branch}"),
                head_branch,
            ],
        )?;
        copy_worktree_includes(base_dir, &path)?;
        Ok(())
    })();
    if let Err(error) = setup {
        // The branch belongs to the user, even when this invocation created it.
        let _ = cleanup_worktree(&ctx);
        return Err(error);
    }
    Ok((ctx, false))
}

fn git_output(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {e}")))?;
    if !output.status.success() {
        return Err(CruiseError::WorktreeError(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn git_status(dir: &Path, args: &[&str]) -> Result<bool> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {e}")))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(CruiseError::WorktreeError(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))),
    }
}

pub(crate) fn validate_base_branch_name(
    repo_dir: &Path,
    name: &str,
) -> std::result::Result<(), String> {
    validate_base_branch_name_with(name, || {
        let output = Command::new("git")
            .args(["check-ref-format", "--branch", name])
            .current_dir(repo_dir)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("failed to run git: {e}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }
        Ok(())
    })
    .map_err(|error| error.unwrap_or_else(|| "branch name is empty or begins with '-'".to_string()))
}

pub(crate) fn validate_base_branch_name_with<E>(
    name: &str,
    check: impl FnOnce() -> std::result::Result<(), E>,
) -> std::result::Result<(), Option<E>> {
    if name.is_empty() || name.starts_with('-') {
        return Err(None);
    }
    check().map_err(Some)
}

fn ref_exists(dir: &Path, full_ref: &str) -> bool {
    Command::new("git")
        .args(["show-ref", "--verify", "--quiet", full_ref])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn resolve_start_point(base_dir: &Path, base: &WorktreeBase) -> Result<Option<String>> {
    let Some(name) = base.branch.as_deref() else {
        return Ok(None);
    };
    validate_base_branch_name(base_dir, name).map_err(|msg| {
        CruiseError::WorktreeError(format!("invalid base branch `{name}`: {msg}"))
    })?;
    let remote = format!("refs/remotes/origin/{name}");
    if base.auto_fetch {
        let refspec = format!("+refs/heads/{name}:{remote}");
        let fetched = Command::new("git")
            .args(["fetch", "origin", &refspec])
            .current_dir(base_dir)
            .stdin(std::process::Stdio::null())
            .output();
        match fetched {
            Ok(output) if output.status.success() => {
                if ref_exists(base_dir, &remote) {
                    return Ok(Some(remote));
                }
            }
            Ok(output) => crate::status_eprintln!(
                "warning: auto_fetch failed for origin/{name}; using local refs: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Err(error) => crate::status_eprintln!(
                "warning: auto_fetch failed for origin/{name}; using local refs: {error}"
            ),
        }
    }
    let local = format!("refs/heads/{name}");
    if ref_exists(base_dir, &local) {
        return Ok(Some(local));
    }
    if ref_exists(base_dir, &remote) {
        return Ok(Some(remote));
    }
    Err(CruiseError::WorktreeError(format!(
        "base branch does not exist: {name}"
    )))
}

/// Remove the worktree and delete its branch.
///
/// # Errors
///
/// Returns an error if the `git` process cannot be spawned or either cleanup command exits
/// unsuccessfully.
pub fn cleanup_worktree(ctx: &WorktreeContext) -> Result<()> {
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&ctx.path)
        .current_dir(&ctx.original_dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::WorktreeError(format!(
            "git worktree remove failed: {}",
            stderr.trim()
        )));
    }

    if !ctx.owns_branch {
        return Ok(());
    }

    let output = Command::new("git")
        .args(["branch", "-D", &ctx.branch])
        .current_dir(&ctx.original_dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::WorktreeError(format!(
            "git branch -D failed: {}",
            stderr.trim()
        )));
    }

    Ok(())
}

fn ensure_git_repo(dir: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        return Err(CruiseError::NotGitRepository);
    }

    Ok(())
}

/// Sanitize a string for use in a git branch name.
fn sanitize_branch_name(input: &str) -> String {
    let raw: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    let sanitized = raw
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    sanitized.chars().take(30).collect()
}

/// Read `.worktreeinclude` from `original_dir` and copy the listed
/// files/directories into `worktree_dir` at the same relative paths.
fn copy_worktree_includes(original_dir: &Path, worktree_dir: &Path) -> Result<()> {
    let include_file = original_dir.join(".worktreeinclude");

    if !include_file.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&include_file)?;

    for line in content.lines() {
        let line = line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let pattern = line.trim_end_matches('/');

        if std::path::Path::new(pattern).is_absolute() || pattern.split('/').any(|c| c == "..") {
            continue;
        }

        let source = original_dir.join(pattern);
        let dest = worktree_dir.join(pattern);

        if !source.exists() {
            continue;
        }

        if source
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            continue;
        }

        if source.is_dir() {
            copy_dir_recursive(&source, &dest)?;
        } else {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &dest)?;
        }
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::lock_process;
    use tempfile::TempDir;

    fn init_git_repo(dir: &Path) {
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir)
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap_or_else(|e| panic!("git command failed: {e:?}"));
        };
        run(&["init"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        fs::write(dir.join("README.md"), "init").unwrap_or_else(|e| panic!("{e:?}"));
        run(&["add", "."]);
        run(&["commit", "-m", "init"]);
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        git_output(dir, args).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn pr_worktree_preserves_branch_and_handles_divergence() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap();
        let remote = tmp.path().join("remote.git");
        git(tmp.path(), &["init", "--bare", remote.to_str().unwrap()]);
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        init_git_repo(&repo);
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&repo, &["push", "origin", "HEAD:refs/heads/pr-head"]);
        let worktrees = tmp.path().join("worktrees");
        let (ctx, reused) = setup_pr_worktree(&repo, "one", &worktrees, "pr-head").unwrap();
        assert!(!reused && !ctx.owns_branch);
        assert_eq!(
            git(&ctx.path, &["rev-parse", "--abbrev-ref", "@{upstream}"]).trim(),
            "origin/pr-head"
        );
        assert!(
            setup_pr_worktree(&repo, "one", &worktrees, "pr-head")
                .unwrap()
                .1
        );
        cleanup_worktree(&ctx).unwrap();
        assert!(
            git_status(
                &repo,
                &["show-ref", "--verify", "--quiet", "refs/heads/pr-head"]
            )
            .unwrap()
        );

        // Advance remote while the local branch stays behind.
        git(&repo, &["commit", "--allow-empty", "-m", "remote advance"]);
        git(&repo, &["push", "origin", "HEAD:refs/heads/pr-head"]);
        let (ctx, _) = setup_pr_worktree(&repo, "two", &worktrees, "pr-head").unwrap();
        assert_eq!(
            git(&ctx.path, &["rev-parse", "HEAD"]),
            git(&repo, &["rev-parse", "HEAD"])
        );
        git(
            &ctx.path,
            &["commit", "--allow-empty", "-m", "local advance"],
        );
        let local_head = git(&ctx.path, &["rev-parse", "HEAD"]);
        cleanup_worktree(&ctx).unwrap();
        let (ctx, _) = setup_pr_worktree(&repo, "three", &worktrees, "pr-head").unwrap();
        assert_eq!(git(&ctx.path, &["rev-parse", "HEAD"]), local_head);
        cleanup_worktree(&ctx).unwrap();

        git(
            &repo,
            &["commit", "--allow-empty", "-m", "remote divergence"],
        );
        git(&repo, &["push", "origin", "HEAD:refs/heads/pr-head"]);
        let err = setup_pr_worktree(&repo, "four", &worktrees, "pr-head").unwrap_err();
        assert!(err.to_string().contains("diverged"));
        assert!(!worktrees.join("four").exists());
        assert_eq!(git(&repo, &["rev-parse", "pr-head"]), local_head);
    }

    fn setup_refspec_fixture(tmp: &TempDir) -> (PathBuf, PathBuf) {
        let source = tmp.path().join("source");
        let origin = tmp.path().join("origin.git");
        let clone = tmp.path().join("clone");
        fs::create_dir(&source).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&source);
        git(&source, &["checkout", "-b", "feature"]);
        git(
            tmp.path(),
            &[
                "init",
                "--bare",
                origin
                    .to_str()
                    .unwrap_or_else(|| panic!("test path must be UTF-8")),
            ],
        );
        git(
            &source,
            &[
                "remote",
                "add",
                "origin",
                origin
                    .to_str()
                    .unwrap_or_else(|| panic!("test path must be UTF-8")),
            ],
        );
        git(
            &source,
            &["push", "origin", "refs/heads/feature:refs/heads/feature"],
        );
        git(
            tmp.path(),
            &[
                "clone",
                "--branch",
                "feature",
                origin
                    .to_str()
                    .unwrap_or_else(|| panic!("test path must be UTF-8")),
                clone
                    .to_str()
                    .unwrap_or_else(|| panic!("test path must be UTF-8")),
            ],
        );
        git(
            &clone,
            &[
                "config",
                "--replace-all",
                "remote.origin.fetch",
                "+refs/heads/other:refs/remotes/origin/other",
            ],
        );
        (source, clone)
    }

    #[test]
    fn auto_fetch_updates_tracking_ref_outside_configured_refspec() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let (source, clone) = setup_refspec_fixture(&tmp);
        fs::write(source.join("README.md"), "updated").unwrap_or_else(|e| panic!("{e:?}"));
        git(&source, &["add", "README.md"]);
        git(&source, &["commit", "-m", "update"]);
        git(
            &source,
            &["push", "origin", "refs/heads/feature:refs/heads/feature"],
        );
        git(&source, &["tag", "feature"]);
        git(
            &source,
            &["push", "origin", "refs/tags/feature:refs/tags/feature"],
        );

        let (ctx, reused) = setup_session_worktree(
            &clone,
            "fetch-test",
            "",
            &tmp.path().join("worktrees"),
            None,
            &WorktreeBase {
                branch: Some("feature".to_string()),
                auto_fetch: true,
            },
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!reused);
        assert_eq!(
            fs::read_to_string(ctx.path.join("README.md"))
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}")),
            "updated"
        );

        git(&clone, &["update-ref", "-d", "refs/remotes/origin/feature"]);
        let (second, reused) = setup_session_worktree(
            &clone,
            "fetch-test-no-tracking-ref",
            "",
            &tmp.path().join("worktrees"),
            None,
            &WorktreeBase {
                branch: Some("feature".to_string()),
                auto_fetch: true,
            },
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!reused);
        assert_eq!(
            fs::read_to_string(second.path.join("README.md"))
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}")),
            "updated"
        );
        assert!(ref_exists(&clone, "refs/remotes/origin/feature"));
    }

    #[test]
    fn auto_fetch_failure_falls_back_to_local_branch() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(tmp.path());
        let output = Command::new("git")
            .args(["branch", "fallback"])
            .current_dir(tmp.path())
            .output()
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(output.status.success());
        let base = WorktreeBase {
            branch: Some("fallback".to_string()),
            auto_fetch: true,
        };
        assert_eq!(
            resolve_start_point(tmp.path(), &base)
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}")),
            Some("refs/heads/fallback".to_string())
        );
    }

    #[test]
    fn test_setup_session_worktree_and_cleanup() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("myrepo");
        fs::create_dir(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);

        let worktrees_dir = tmp.path().join("worktrees");
        let session_id = "20260306143000";
        let (ctx, reused) = setup_session_worktree(
            &repo,
            session_id,
            "test task",
            &worktrees_dir,
            None,
            &WorktreeBase::default(),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        assert!(!reused, "should not be reused on first creation");
        assert!(ctx.path.exists(), "worktree directory should exist");
        assert_eq!(ctx.path, worktrees_dir.join(session_id));
        assert!(
            ctx.branch.starts_with("cruise/"),
            "branch should start with cruise/"
        );
        assert!(
            ctx.branch.contains("test-task"),
            "branch should contain sanitized input"
        );

        cleanup_worktree(&ctx).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!ctx.path.exists(), "worktree directory should be removed");
    }

    #[test]
    fn test_setup_session_worktree_empty_input() {
        let _lock = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("myrepo");
        fs::create_dir(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);

        let worktrees_dir = tmp.path().join("worktrees");
        let session_id = "20260306143001";
        let (ctx, _) = setup_session_worktree(
            &repo,
            session_id,
            "",
            &worktrees_dir,
            None,
            &WorktreeBase::default(),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(ctx.branch, format!("cruise/{session_id}"));
        cleanup_worktree(&ctx).unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[test]
    fn test_setup_session_worktree_not_git_repo() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let worktrees_dir = tmp.path().join("worktrees");
        let result = setup_session_worktree(
            tmp.path(),
            "20260306143000",
            "task",
            &worktrees_dir,
            None,
            &WorktreeBase::default(),
        );
        assert!(
            matches!(result, Err(CruiseError::NotGitRepository)),
            "expected NotGitRepository error"
        );
    }

    #[test]
    fn test_sanitize_branch_name() {
        assert_eq!(sanitize_branch_name("hello world"), "hello-world");
        assert_eq!(sanitize_branch_name("fix/bug-123"), "fix-bug-123");
        assert_eq!(sanitize_branch_name("test!@#$%"), "test");
        assert_eq!(sanitize_branch_name("a--b"), "a-b");
        assert_eq!(sanitize_branch_name("-leading"), "leading");
    }

    #[test]
    fn test_branch_name_truncation() {
        let long = "a".repeat(50);
        let result = sanitize_branch_name(&long);
        assert_eq!(result.len(), 30);
    }

    #[test]
    fn test_branch_name_empty_input() {
        assert_eq!(sanitize_branch_name(""), "");
    }

    #[test]
    fn test_copy_worktree_includes() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap_or_else(|e| panic!("{e:?}"));
        fs::create_dir_all(&dst).unwrap_or_else(|e| panic!("{e:?}"));

        fs::write(src.join(".worktreeinclude"), ".env\n").unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(src.join(".env"), "SECRET=123").unwrap_or_else(|e| panic!("{e:?}"));

        copy_worktree_includes(&src, &dst).unwrap_or_else(|e| panic!("{e:?}"));

        assert!(dst.join(".env").exists());
        assert_eq!(
            fs::read_to_string(dst.join(".env")).unwrap_or_else(|e| panic!("{e:?}")),
            "SECRET=123"
        );
    }

    #[test]
    fn test_copy_worktree_includes_directory() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap_or_else(|e| panic!("{e:?}"));
        fs::create_dir_all(&dst).unwrap_or_else(|e| panic!("{e:?}"));

        fs::write(src.join(".worktreeinclude"), ".cruise/\n").unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = src.join(".cruise");
        fs::create_dir_all(&cruise_dir).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(cruise_dir.join("config.yaml"), "key: value").unwrap_or_else(|e| panic!("{e:?}"));

        copy_worktree_includes(&src, &dst).unwrap_or_else(|e| panic!("{e:?}"));

        assert!(dst.join(".cruise").join("config.yaml").exists());
    }

    #[test]
    fn test_copy_worktree_includes_missing_file() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap_or_else(|e| panic!("{e:?}"));
        fs::create_dir_all(&dst).unwrap_or_else(|e| panic!("{e:?}"));

        let result = copy_worktree_includes(&src, &dst);
        assert!(result.is_ok());
    }

    #[test]
    fn test_copy_worktree_includes_comments() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap_or_else(|e| panic!("{e:?}"));
        fs::create_dir_all(&dst).unwrap_or_else(|e| panic!("{e:?}"));

        fs::write(
            src.join(".worktreeinclude"),
            "# this is a comment\n\n# another comment\n.env\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(src.join(".env"), "SECRET=123").unwrap_or_else(|e| panic!("{e:?}"));

        copy_worktree_includes(&src, &dst).unwrap_or_else(|e| panic!("{e:?}"));

        assert!(dst.join(".env").exists());
    }
}
