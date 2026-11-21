type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
use cruise::manager_git::{self, ApplyOutcome, IntegrationIntent};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

const GOAL: &str = "refs/heads/cruise/goal/integration-test";

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn repo() -> Result<(TempDir, String)> {
    let root = TempDir::new()?;
    git(root.path(), &["init", "-q"])?;
    git(root.path(), &["config", "user.name", "Integration Test"])?;
    git(root.path(), &["config", "user.email", "test@example.com"])?;
    fs::write(root.path().join("shared.txt"), "base\n")?;
    git(root.path(), &["add", "."])?;
    git(root.path(), &["commit", "-qm", "base"])?;
    let base = git(root.path(), &["rev-parse", "HEAD"])?;
    Ok((root, base))
}

fn child(repo: &Path, base: &str, file: &str, content: &str) -> Result<String> {
    git(repo, &["checkout", "-q", "--detach", base])?;
    fs::write(repo.join(file), content)?;
    git(repo, &["add", file])?;
    git(repo, &["commit", "-qm", "child"])?;
    git(repo, &["rev-parse", "HEAD"])
}

#[test]
fn persisted_intent_recovers_after_apply_without_touching_worktree() -> Result<()> {
    let (root, base) = repo()?;
    let repo = root.path();
    let result = child(repo, &base, "child.txt", "child\n")?;
    git(repo, &["checkout", "-q", "--detach", &base])?;
    manager_git::initialize(repo, GOAL, &base)?;
    manager_git::initialize(repo, GOAL, &base)?;
    let intent = manager_git::prepare_integration(repo, GOAL, &base, &base, &result)?;
    assert_ne!(intent.prepared_tip, base);
    assert_eq!(git(repo, &["rev-parse", GOAL])?, base);
    assert!(!manager_git::integration_already_applied(repo, &intent)?);
    // Persist and reload the complete intent as a restarted caller would.
    let state = repo.join("intent.json");
    fs::write(&state, serde_json::to_vec(&intent)?)?;
    let recovered: IntegrationIntent = serde_json::from_slice(&fs::read(&state)?)?;
    assert_eq!(
        manager_git::apply_integration(repo, &recovered)?,
        ApplyOutcome::Applied
    );
    assert_eq!(git(repo, &["rev-parse", GOAL])?, intent.prepared_tip);
    let recovered_again: IntegrationIntent = serde_json::from_slice(&fs::read(&state)?)?;
    assert!(manager_git::integration_already_applied(
        repo,
        &recovered_again
    )?);
    assert_eq!(
        manager_git::apply_integration(repo, &recovered_again)?,
        ApplyOutcome::AlreadyApplied
    );
    assert_eq!(git(repo, &["rev-parse", "HEAD"])?, base);
    assert!(!repo.join("child.txt").exists());
    assert_eq!(git(repo, &["show", &format!("{GOAL}:child.txt")])?, "child");
    Ok(())
}

#[test]
fn stale_intent_cannot_overwrite_a_moved_ref() -> Result<()> {
    let (root, base) = repo()?;
    let repo = root.path();
    let result = child(repo, &base, "child.txt", "child\n")?;
    let other = child(repo, &base, "other.txt", "other\n")?;
    manager_git::initialize(repo, GOAL, &base)?;
    let intent = manager_git::prepare_integration(repo, GOAL, &base, &base, &result)?;
    git(repo, &["update-ref", GOAL, &other, &base])?;
    assert!(!manager_git::integration_already_applied(repo, &intent)?);
    assert!(manager_git::apply_integration(repo, &intent).is_err());
    assert_eq!(git(repo, &["rev-parse", GOAL])?, other);
    assert!(manager_git::initialize(repo, GOAL, &base).is_err());
    assert_eq!(git(repo, &["rev-parse", GOAL])?, other);
    Ok(())
}

#[test]
fn conflicting_child_preserves_goal_ref_and_worktree() -> Result<()> {
    let (root, base) = repo()?;
    let repo = root.path();
    let result = child(repo, &base, "shared.txt", "child\n")?;
    let goal_tip = child(repo, &base, "shared.txt", "goal\n")?;
    manager_git::initialize(repo, GOAL, &goal_tip)?;
    let before = git(repo, &["status", "--porcelain"])?;
    assert!(manager_git::prepare_integration(repo, GOAL, &goal_tip, &base, &result).is_err());
    assert_eq!(git(repo, &["rev-parse", GOAL])?, goal_tip);
    assert_eq!(git(repo, &["rev-parse", "HEAD"])?, goal_tip);
    assert_eq!(git(repo, &["status", "--porcelain"])?, before);
    assert_eq!(fs::read_to_string(repo.join("shared.txt"))?, "goal\n");
    Ok(())
}
