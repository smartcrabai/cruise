use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{CruiseError, Result};

pub struct WorktreeContext {
    pub path: PathBuf,
    pub branch: String,
    pub original_dir: PathBuf,
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
pub fn setup_session_worktree(
    base_dir: &Path,
    session_id: &str,
    input: &str,
    worktrees_dir: &Path,
    existing_branch: Option<&str>,
) -> Result<(WorktreeContext, bool)> {
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
            },
            true,
        ));
    }

    let branch = default_branch_name(session_id, input);
    fs::create_dir_all(worktrees_dir)?;

    let output = Command::new("git")
        .args(["worktree", "add", "-b", &branch])
        .arg(&worktree_path)
        .current_dir(base_dir)
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::WorktreeError(format!(
            "git worktree add failed: {}",
            stderr.trim()
        )));
    }

    copy_worktree_includes(base_dir, &worktree_path)?;

    Ok((
        WorktreeContext {
            path: worktree_path,
            branch,
            original_dir: base_dir.to_path_buf(),
        },
        false,
    ))
}

/// Remove the worktree and delete its branch.
pub fn cleanup_worktree(ctx: &WorktreeContext) -> Result<()> {
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&ctx.path)
        .current_dir(&ctx.original_dir)
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("warning: git worktree remove failed: {}", stderr.trim());
    }

    let output = Command::new("git")
        .args(["branch", "-D", &ctx.branch])
        .current_dir(&ctx.original_dir)
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("warning: git branch -D failed: {}", stderr.trim());
    }

    Ok(())
}

fn ensure_git_repo(dir: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
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
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
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
    use tempfile::TempDir;

    fn init_git_repo(dir: &Path) {
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir)
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

    #[test]
    fn test_setup_session_worktree_and_cleanup() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("myrepo");
        fs::create_dir(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);

        let worktrees_dir = tmp.path().join("worktrees");
        let session_id = "20260306143000";
        let (ctx, reused) =
            setup_session_worktree(&repo, session_id, "test task", &worktrees_dir, None)
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
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("myrepo");
        fs::create_dir(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);

        let worktrees_dir = tmp.path().join("worktrees");
        let session_id = "20260306143001";
        let (ctx, _) = setup_session_worktree(&repo, session_id, "", &worktrees_dir, None)
            .unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(ctx.branch, format!("cruise/{session_id}"));
        cleanup_worktree(&ctx).unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[test]
    fn test_setup_session_worktree_not_git_repo() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let worktrees_dir = tmp.path().join("worktrees");
        let result =
            setup_session_worktree(tmp.path(), "20260306143000", "task", &worktrees_dir, None);
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
