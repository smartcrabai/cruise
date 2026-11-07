//! End-to-end contract for the non-interactive diff command.
use cruise::session::{SessionManager, SessionState};
use cruise::session_config::SessionConfigRef;
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn git(dir: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git").current_dir(dir).args(args).output()?;
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn diff_command_reports_committed_and_uncommitted_changes_and_stat() -> Result<()> {
    let root = TempDir::new()?;
    let repo = root.path().join("repo");
    let worktree = root.path().join("worktree");
    fs::create_dir(&repo)?;
    git(&repo, &["init", "-q"])?;
    git(&repo, &["config", "user.name", "Test"])?;
    git(&repo, &["config", "user.email", "test@example.invalid"])?;
    fs::write(repo.join("tracked.txt"), "before\n")?;
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-qm", "base"])?;
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-qb",
            "session-test",
            worktree.to_str().ok_or("non-UTF-8 worktree path")?,
        ],
    )?;
    fs::write(worktree.join("committed.txt"), "committed content\n")?;
    git(&worktree, &["add", "."])?;
    git(&worktree, &["commit", "-qm", "session change"])?;
    fs::write(worktree.join("tracked.txt"), "after\n")?;
    fs::write(worktree.join("new.txt"), "untracked content\n")?;
    let id = "20260101000000000";
    let mut state = SessionState::new(
        id.into(),
        repo,
        SessionConfigRef::File {
            path: root.path().join("workflow.yaml"),
        },
        "diff fixture".into(),
    );
    state.worktree_branch = Some("session-test".into());
    state.worktree_path = Some(worktree);
    SessionManager::new(root.path().join("data/cruise")).create(&state)?;
    let run = |stat: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        cmd.env("HOME", root.path())
            .env("XDG_DATA_HOME", root.path().join("data"))
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_STATE_HOME", root.path().join("state"))
            .args(["diff", id]);
        if stat {
            cmd.arg("--stat");
        }
        cmd.output()
    };
    let patch = run(false)?;
    assert!(
        patch.status.success(),
        "{}",
        String::from_utf8_lossy(&patch.stderr)
    );
    let patch = String::from_utf8(patch.stdout)?;
    for expected in [
        "+++ b/committed.txt",
        "+after",
        "+++ b/new.txt",
        "+untracked content",
    ] {
        assert!(patch.contains(expected), "missing {expected}: {patch}");
    }
    let stat = run(true)?;
    assert!(
        stat.status.success(),
        "{}",
        String::from_utf8_lossy(&stat.stderr)
    );
    let stat = String::from_utf8(stat.stdout)?;
    for expected in ["committed.txt", "tracked.txt", "new.txt | untracked"] {
        assert!(stat.contains(expected), "missing {expected}: {stat}");
    }
    assert!(!stat.contains("diff --git"), "{stat}");

    let wt = root.path().join("worktree");
    fs::write(wt.join("tracked.txt"), ("large line\n").repeat(150_000))?;
    let large = run(false)?;
    assert!(large.status.success());
    assert!(String::from_utf8_lossy(&large.stderr).contains("warning: diff truncated"));
    assert!(large.stdout.len() <= cruise::branch_diff::MAX_PATCH_BYTES);
    fs::write(wt.join("tracked.txt"), "after\n")?;
    for n in 0..=cruise::branch_diff::UNTRACKED_FILE_LIMIT {
        fs::write(wt.join(format!("extra-{n:04}.txt")), "x\n")?;
    }
    let many = run(false)?;
    assert!(many.status.success());
    assert!(many.stdout.len() < cruise::branch_diff::MAX_PATCH_BYTES);
    let warning = String::from_utf8_lossy(&many.stderr);
    assert!(warning.contains("warning: diff truncated"), "{warning}");
    assert!(!warning.contains("bytes"), "{warning}");
    Ok(())
}
