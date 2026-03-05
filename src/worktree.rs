use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{CruiseError, Result};

pub struct WorktreeContext {
    pub path: PathBuf,
    pub branch: String,
    pub original_dir: PathBuf,
}

/// Create a new git worktree for isolated workflow execution.
pub fn setup_worktree(original_dir: &Path, input: Option<&str>) -> Result<WorktreeContext> {
    ensure_git_repo(original_dir)?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let branch = if let Some(inp) = input.filter(|s| !s.is_empty()) {
        let sanitized = sanitize_branch_name(inp);
        if sanitized.is_empty() {
            format!("cruise/{}", timestamp)
        } else {
            format!("cruise/{}-{}", timestamp, sanitized)
        }
    } else {
        format!("cruise/{}", timestamp)
    };

    let repo_name = original_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");
    let worktree_dir_name = format!("{}-cruise-{}", repo_name, timestamp);
    let worktree_path = original_dir
        .parent()
        .unwrap_or(original_dir)
        .join(&worktree_dir_name);

    let output = Command::new("git")
        .args(["worktree", "add", "-b", &branch])
        .arg(&worktree_path)
        .current_dir(original_dir)
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::WorktreeError(format!(
            "git worktree add failed: {}",
            stderr.trim()
        )));
    }

    copy_worktree_includes(original_dir, &worktree_path)?;

    Ok(WorktreeContext {
        path: worktree_path,
        branch,
        original_dir: original_dir.to_path_buf(),
    })
}

/// Remove the worktree and delete its branch.
pub fn cleanup_worktree(ctx: &WorktreeContext) -> Result<()> {
    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&ctx.path)
        .current_dir(&ctx.original_dir)
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("warning: git worktree remove failed: {}", stderr.trim());
    }

    let output = Command::new("git")
        .args(["branch", "-D", &ctx.branch])
        .current_dir(&ctx.original_dir)
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {}", e)))?;

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
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {}", e)))?;

    if !output.status.success() {
        return Err(CruiseError::NotGitRepository);
    }

    Ok(())
}

/// Sanitize a string for use in a git branch name.
/// Keeps only ASCII alphanumerics and hyphens, collapses runs of non-matching
/// characters into a single hyphen, strips leading/trailing hyphens, and
/// truncates to 30 characters.
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

    // Collapse consecutive hyphens and strip leading/trailing ones.
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

        // Strip trailing slash to get the actual relative path.
        let pattern = line.trim_end_matches('/');

        // Reject absolute paths and path traversal attempts.
        if std::path::Path::new(pattern).is_absolute() || pattern.split('/').any(|c| c == "..") {
            continue;
        }

        let source = original_dir.join(pattern);
        let dest = worktree_dir.join(pattern);

        if !source.exists() {
            continue;
        }

        // Skip symlinks to avoid following links outside the repository.
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

/// Metadata about an existing cruise worktree.
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: String,
}

/// A cruise worktree that has a saved state file and can be resumed.
pub struct ResumableWorktree {
    pub info: WorktreeInfo,
    pub current_step: String,
}

/// Parse `git worktree list --porcelain` output and return all worktrees
/// whose branch matches `refs/heads/cruise/`.
fn list_cruise_worktrees(repo_dir: &Path) -> Result<Vec<WorktreeInfo>> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_dir)
        .output()
        .map_err(|e| CruiseError::WorktreeError(format!("failed to run git: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::WorktreeError(format!(
            "git worktree list failed: {}",
            stderr.trim()
        )));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in text.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            // Flush the previous block if it had a cruise/ branch.
            if let (Some(path), Some(branch)) = (current_path.take(), current_branch.take()) {
                worktrees.push(WorktreeInfo { path, branch });
            }
            current_path = Some(PathBuf::from(path_str));
            current_branch = None;
        } else if let Some(branch_ref) = line.strip_prefix("branch ")
            && branch_ref.starts_with("refs/heads/cruise/")
        {
            current_branch = Some(
                branch_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch_ref)
                    .to_string(),
            );
        }
    }
    // Flush the last block.
    if let (Some(path), Some(branch)) = (current_path, current_branch) {
        worktrees.push(WorktreeInfo { path, branch });
    }

    Ok(worktrees)
}

/// Find all cruise worktrees that have a resumable state file.
pub fn find_resumable_worktrees(
    repo_dir: &Path,
    config: &crate::config::WorkflowConfig,
) -> Result<Vec<ResumableWorktree>> {
    let state_path = match &config.state {
        Some(p) => p,
        None => return Ok(vec![]),
    };

    let cruise_worktrees = list_cruise_worktrees(repo_dir)?;
    let mut resumable = Vec::new();

    for info in cruise_worktrees {
        let candidate_state = info.path.join(state_path);
        if !candidate_state.exists() {
            continue;
        }
        match crate::state::WorkflowState::load(&candidate_state) {
            Ok(state) => {
                resumable.push(ResumableWorktree {
                    info,
                    current_step: state.current,
                });
            }
            Err(e) => {
                eprintln!(
                    "warning: failed to load state from {}: {}",
                    candidate_state.display(),
                    e
                );
            }
        }
    }

    Ok(resumable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Initialize a minimal git repository with one commit so that worktree
    /// operations have a valid HEAD to check out from.
    fn init_git_repo(dir: &Path) {
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git command failed");
        };
        run(&["init"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test"]);
        fs::write(dir.join("README.md"), "init").unwrap();
        run(&["add", "."]);
        run(&["commit", "-m", "init"]);
    }

    #[test]
    fn test_setup_and_cleanup_worktree() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("myrepo");
        fs::create_dir(&repo).unwrap();
        init_git_repo(&repo);

        let ctx = setup_worktree(&repo, Some("test task")).unwrap();
        assert!(ctx.path.exists(), "worktree directory should exist");
        assert!(
            ctx.branch.starts_with("cruise/"),
            "branch should start with cruise/"
        );
        assert!(
            ctx.branch.contains("test-task"),
            "branch should contain sanitized input"
        );

        cleanup_worktree(&ctx).unwrap();
        assert!(!ctx.path.exists(), "worktree directory should be removed");
    }

    #[test]
    fn test_setup_worktree_not_git_repo() {
        let tmp = TempDir::new().unwrap();
        let result = setup_worktree(tmp.path(), None);
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
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join(".worktreeinclude"), ".env\n").unwrap();
        fs::write(src.join(".env"), "SECRET=123").unwrap();

        copy_worktree_includes(&src, &dst).unwrap();

        assert!(dst.join(".env").exists());
        assert_eq!(fs::read_to_string(dst.join(".env")).unwrap(), "SECRET=123");
    }

    #[test]
    fn test_copy_worktree_includes_directory() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(src.join(".worktreeinclude"), ".cruise/\n").unwrap();
        let cruise_dir = src.join(".cruise");
        fs::create_dir_all(&cruise_dir).unwrap();
        fs::write(cruise_dir.join("config.yaml"), "key: value").unwrap();

        copy_worktree_includes(&src, &dst).unwrap();

        assert!(dst.join(".cruise").join("config.yaml").exists());
        assert_eq!(
            fs::read_to_string(dst.join(".cruise").join("config.yaml")).unwrap(),
            "key: value"
        );
    }

    #[test]
    fn test_copy_worktree_includes_missing_file() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        // No .worktreeinclude — should be a no-op, not an error.
        let result = copy_worktree_includes(&src, &dst);
        assert!(result.is_ok());
    }

    #[test]
    fn test_copy_worktree_includes_comments() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        fs::write(
            src.join(".worktreeinclude"),
            "# this is a comment\n\n# another comment\n.env\n",
        )
        .unwrap();
        fs::write(src.join(".env"), "SECRET=123").unwrap();

        copy_worktree_includes(&src, &dst).unwrap();

        assert!(dst.join(".env").exists());
    }

    fn run_git(dir: &Path, args: &[&str]) {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git command failed");
    }

    #[test]
    fn test_list_cruise_worktrees() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("myrepo");
        fs::create_dir(&repo).unwrap();
        init_git_repo(&repo);

        // Create a cruise/ worktree — should be detected.
        let wt_cruise = tmp.path().join("myrepo-cruise");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "cruise/test-task",
                wt_cruise.to_str().unwrap(),
            ],
        );

        // Create a non-cruise worktree — should NOT be detected.
        let wt_other = tmp.path().join("myrepo-other");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "feature/other",
                wt_other.to_str().unwrap(),
            ],
        );

        let result = list_cruise_worktrees(&repo).unwrap();
        assert_eq!(
            result.len(),
            1,
            "should detect exactly one cruise/ worktree"
        );
        assert_eq!(result[0].branch, "cruise/test-task");
        // Canonicalize both sides: macOS /var is a symlink to /private/var.
        assert_eq!(
            result[0].path.canonicalize().unwrap(),
            wt_cruise.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_find_resumable_worktrees() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("myrepo");
        fs::create_dir(&repo).unwrap();
        init_git_repo(&repo);

        let wt_path = tmp.path().join("myrepo-wt");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "cruise/test",
                wt_path.to_str().unwrap(),
            ],
        );

        // Write a state file inside the worktree using WorkflowState helpers.
        use crate::config::{StepConfig, WorkflowConfig};
        use crate::state::WorkflowState;
        use indexmap::IndexMap;
        use std::collections::HashMap;

        let state_rel = std::path::PathBuf::from(".cruise/state.json");
        let mut steps = IndexMap::new();
        steps.insert(
            "my-step".to_string(),
            StepConfig {
                prompt: Some("test".to_string()),
                ..Default::default()
            },
        );
        let cfg = WorkflowConfig {
            command: vec!["claude".to_string(), "-p".to_string()],
            model: None,
            plan: None,
            env: HashMap::new(),
            worktree: false,
            state: Some(state_rel.clone()),
            steps,
        };
        WorkflowState::new(cfg.clone(), "my-step".to_string())
            .save(&wt_path.join(&state_rel))
            .unwrap();

        let result = find_resumable_worktrees(&repo, &cfg).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].current_step, "my-step");
        // Canonicalize both sides: macOS /var is a symlink to /private/var.
        assert_eq!(
            result[0].info.path.canonicalize().unwrap(),
            wt_path.canonicalize().unwrap()
        );
    }

    #[test]
    fn test_find_resumable_worktrees_empty() {
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("myrepo");
        fs::create_dir(&repo).unwrap();
        init_git_repo(&repo);

        let wt_path = tmp.path().join("myrepo-wt");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "cruise/test",
                wt_path.to_str().unwrap(),
            ],
        );

        // No state file written — should return empty.
        use crate::config::{StepConfig, WorkflowConfig};
        use indexmap::IndexMap;
        use std::collections::HashMap;

        let mut steps = IndexMap::new();
        steps.insert(
            "step1".to_string(),
            StepConfig {
                command: Some(crate::config::StringOrVec::Single("echo hi".to_string())),
                ..Default::default()
            },
        );
        let cfg = WorkflowConfig {
            command: vec!["claude".to_string(), "-p".to_string()],
            model: None,
            plan: None,
            env: HashMap::new(),
            worktree: false,
            state: Some(std::path::PathBuf::from(".cruise/state.json")),
            steps,
        };

        let result = find_resumable_worktrees(&repo, &cfg).unwrap();
        assert!(result.is_empty(), "expected no resumable worktrees");
    }
}
