//! Real-git contract tests for the planned application-level local merge API.
use std::path::{Path, PathBuf};
use std::process::Command;

use cruise::application::{CruiseApplication, LocalMergeMode, LocalMergeOutcome};
use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;

const ID: &str = "20261009000000";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn git(dir: &Path, args: &[&str]) -> TestResult<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_COUNT", "0")
        .output()?;
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

macro_rules! git {
    ($dir:expr, $args:expr $(,)?) => {
        git($dir, $args)?
    };
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    worktree: PathBuf,
    branch: String,
    manager: SessionManager,
    app: CruiseApplication,
}

impl Fixture {
    fn new() -> TestResult<Self> {
        let tmp = tempfile::tempdir()?;
        let base = tmp.path().join("base");
        std::fs::create_dir(&base)?;
        git!(&base, &["init"]);
        git!(&base, &["config", "user.name", "Test"]);
        git!(&base, &["config", "user.email", "test@example.com"]);
        std::fs::write(base.join("shared.txt"), "original\n")?;
        git!(&base, &["add", "."]);
        git!(&base, &["commit", "-m", "initial"]);
        git!(&base, &["branch", "-M", "main"]);
        let (ctx, _) = cruise::worktree::setup_session_worktree(
            &base,
            ID,
            "task",
            &tmp.path().join("worktrees"),
            None,
            &cruise::worktree::WorktreeBase::default(),
        )?;
        let manager = SessionManager::new(tmp.path().join("sessions"));
        let mut state = SessionState::new(
            ID.into(),
            base.clone(),
            SessionConfigRef::BuiltinSnapshot { name: None },
            "task".into(),
        );
        state.phase = SessionPhase::Completed;
        state.worktree_path = Some(ctx.path.clone());
        state.worktree_branch = Some(ctx.branch.clone());
        let mut json = serde_json::to_value(state)?;
        json["forked_from_branch"] = serde_json::json!("main");
        let state: SessionState = serde_json::from_value(json)?;
        manager.create(&state)?;
        let app = CruiseApplication::new(manager.clone());
        Ok(Self {
            _tmp: tmp,
            base,
            worktree: ctx.path,
            branch: ctx.branch,
            manager,
            app,
        })
    }

    fn changes(&self) -> TestResult {
        std::fs::write(self.worktree.join("a.txt"), "committed\n")?;
        git!(&self.worktree, &["add", "a.txt"]);
        git!(&self.worktree, &["commit", "-m", "work"]);
        std::fs::write(self.worktree.join("b.txt"), "uncommitted\n")?;
        Ok(())
    }

    fn kept(&self) -> TestResult {
        assert!(self.manager.load(ID).is_ok(), "session must remain");
        assert!(self.worktree.is_dir(), "worktree must remain");
        assert!(
            !git!(&self.base, &["branch", "--list", &self.branch]).is_empty(),
            "branch must remain"
        );
        Ok(())
    }
}

#[test]
fn stage_preserves_head_and_session_but_stages_committed_and_uncommitted_work() -> TestResult {
    let f = Fixture::new()?;
    f.changes()?;
    let head = git!(&f.base, &["rev-parse", "HEAD"]);
    let branch_head = git!(&f.worktree, &["rev-parse", "HEAD"]);
    assert!(matches!(
        f.app.local_merge(ID, LocalMergeMode::Stage)?,
        LocalMergeOutcome::Staged
    ));
    assert_eq!(git!(&f.base, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git!(&f.base, &["diff", "--cached", "--name-only"]),
        "a.txt\nb.txt"
    );
    assert_ne!(git!(&f.worktree, &["rev-parse", "HEAD"]), branch_head);
    assert_eq!(git!(&f.worktree, &["status", "--porcelain"]), "");
    assert!(
        serde_json::to_value(f.manager.load(ID)?)?
            .get("local_merge_commit")
            .is_none()
    );
    f.kept()?;
    Ok(())
}

#[test]
fn merge_creates_one_squash_commit_then_removes_session_worktree_and_branch() -> TestResult {
    let f = Fixture::new()?;
    f.changes()?;
    let head = git!(&f.base, &["rev-parse", "HEAD"]);
    assert!(matches!(
        f.app.local_merge(ID, LocalMergeMode::MergeAndCleanup)?,
        LocalMergeOutcome::Merged
    ));
    assert_eq!(
        git!(&f.base, &["rev-list", "--count", &format!("{head}..HEAD")]),
        "1"
    );
    assert_eq!(git!(&f.base, &["log", "-1", "--format=%s"]), "task");
    assert_eq!(
        std::fs::read_to_string(f.base.join("a.txt"))?,
        "committed\n"
    );
    assert_eq!(
        std::fs::read_to_string(f.base.join("b.txt"))?,
        "uncommitted\n"
    );
    assert!(f.manager.load(ID).is_err());
    assert!(!f.worktree.exists());
    assert_eq!(git!(&f.base, &["branch", "--list", &f.branch]), "");
    Ok(())
}

#[test]
fn dirty_base_is_refused_before_session_worktree_is_committed() -> TestResult {
    for staged in [false, true] {
        let f = Fixture::new()?;
        f.changes()?;
        std::fs::write(f.base.join("shared.txt"), "local changes\n")?;
        if staged {
            git!(&f.base, &["add", "shared.txt"]);
        }
        let head = git!(&f.base, &["rev-parse", "HEAD"]);
        let branch_head = git!(&f.worktree, &["rev-parse", "HEAD"]);
        let status = git!(&f.base, &["status", "--porcelain"]);
        let error = f
            .app
            .local_merge(ID, LocalMergeMode::MergeAndCleanup)
            .err()
            .ok_or("expected an error")?;
        assert!(
            error.to_string().to_lowercase().contains("uncommitted"),
            "{error}"
        );
        assert_eq!(git!(&f.base, &["rev-parse", "HEAD"]), head);
        assert_eq!(git!(&f.base, &["status", "--porcelain"]), status);
        assert_eq!(git!(&f.worktree, &["rev-parse", "HEAD"]), branch_head);
        assert_eq!(
            std::fs::read_to_string(f.worktree.join("b.txt"))?,
            "uncommitted\n"
        );
        f.kept()?;
    }
    Ok(())
}

#[test]
fn conflict_reports_path_and_restores_base_without_losing_session() -> TestResult {
    let f = Fixture::new()?;
    std::fs::write(f.worktree.join("shared.txt"), "session\n")?;
    git!(&f.worktree, &["add", "shared.txt"]);
    git!(&f.worktree, &["commit", "-m", "session change"]);
    std::fs::write(f.base.join("shared.txt"), "base\n")?;
    git!(&f.base, &["add", "shared.txt"]);
    git!(&f.base, &["commit", "-m", "base change"]);
    let head = git!(&f.base, &["rev-parse", "HEAD"]);
    let error = f
        .app
        .local_merge(ID, LocalMergeMode::MergeAndCleanup)
        .err()
        .ok_or("expected an error")?;
    assert!(error.to_string().contains("shared.txt"), "{error}");
    assert_eq!(git!(&f.base, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git!(&f.base, &["status", "--porcelain", "--untracked-files=no"]),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(f.base.join("shared.txt"))?,
        "base\n"
    );
    f.kept()?;
    Ok(())
}

#[test]
fn untracked_base_file_is_preserved_when_squash_would_overwrite_it() -> TestResult {
    let f = Fixture::new()?;
    f.changes()?;
    std::fs::write(f.base.join("a.txt"), "precious\n")?;
    let head = git!(&f.base, &["rev-parse", "HEAD"]);
    assert!(f.app.local_merge(ID, LocalMergeMode::Stage).is_err());
    assert_eq!(git!(&f.base, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git!(&f.base, &["status", "--porcelain", "--untracked-files=no"]),
        ""
    );
    assert_eq!(std::fs::read_to_string(f.base.join("a.txt"))?, "precious\n");
    f.kept()?;
    Ok(())
}

#[test]
fn switched_session_checkout_is_never_staged_or_cleaned() -> TestResult {
    for mode in [LocalMergeMode::Stage, LocalMergeMode::MergeAndCleanup] {
        let f = Fixture::new()?;
        f.changes()?;
        git!(&f.worktree, &["checkout", "-b", "other"]);
        let base_head = git!(&f.base, &["rev-parse", "HEAD"]);
        let other_head = git!(&f.worktree, &["rev-parse", "HEAD"]);
        let session_head = git!(&f.base, &["rev-parse", &f.branch]);
        let status = git!(&f.worktree, &["status", "--porcelain"]);
        assert!(f.app.local_merge(ID, mode).is_err());
        assert_eq!(git!(&f.base, &["rev-parse", "HEAD"]), base_head);
        assert_eq!(git!(&f.worktree, &["rev-parse", "HEAD"]), other_head);
        assert_eq!(git!(&f.base, &["rev-parse", &f.branch]), session_head);
        assert_eq!(git!(&f.worktree, &["status", "--porcelain"]), status);
        f.kept()?;
    }
    Ok(())
}

#[test]
fn cleanup_retry_preserves_changed_worktree_and_advanced_branch() -> TestResult {
    for advance in [false, true] {
        let f = Fixture::new()?;
        f.changes()?;
        let mut state = f.manager.load(ID)?;
        let outcome = cruise::local_merge::merge_session(
            &mut state,
            LocalMergeMode::MergeAndCleanup,
            |s| f.manager.save(s),
            |_| {
                Err(cruise::error::CruiseError::Other(
                    "injected cleanup failure".into(),
                ))
            },
        )?;
        assert!(matches!(
            outcome,
            LocalMergeOutcome::MergedCleanupFailed { .. }
        ));
        let head = git!(&f.base, &["rev-parse", "HEAD"]);
        std::fs::write(f.worktree.join("new.txt"), "new work\n")?;
        if advance {
            git!(&f.worktree, &["add", "new.txt"]);
            git!(&f.worktree, &["commit", "-m", "new work"]);
        }
        assert!(
            f.app
                .local_merge(ID, LocalMergeMode::MergeAndCleanup)
                .is_err()
        );
        assert_eq!(git!(&f.base, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            std::fs::read_to_string(f.worktree.join("new.txt"))?,
            "new work\n"
        );
        f.kept()?;
    }
    Ok(())
}

#[test]
fn cleanup_retry_allows_absent_worktree() -> TestResult {
    let f = Fixture::new()?;
    f.changes()?;
    let mut state = f.manager.load(ID)?;
    cruise::local_merge::merge_session(
        &mut state,
        LocalMergeMode::MergeAndCleanup,
        |s| f.manager.save(s),
        |_| {
            Err(cruise::error::CruiseError::Other(
                "injected cleanup failure".into(),
            ))
        },
    )?;
    let head = git!(&f.base, &["rev-parse", "HEAD"]);
    git!(
        &f.base,
        &[
            "worktree",
            "remove",
            f.worktree.to_str().ok_or("worktree path is not UTF-8")?,
        ],
    );
    assert!(matches!(
        f.app.local_merge(ID, LocalMergeMode::MergeAndCleanup)?,
        LocalMergeOutcome::Merged
    ));
    assert_eq!(git!(&f.base, &["rev-parse", "HEAD"]), head);
    assert!(f.manager.load(ID).is_err());
    assert_eq!(git!(&f.base, &["branch", "--list", &f.branch]), "");
    Ok(())
}

#[test]
fn persisted_intent_recovers_after_marker_write_failure() -> TestResult {
    let f = Fixture::new()?;
    f.changes()?;
    let mut state = f.manager.load(ID)?;
    let error = cruise::local_merge::merge_session(
        &mut state,
        LocalMergeMode::MergeAndCleanup,
        |s| {
            if s.local_merge_commit.is_some() {
                Err(cruise::error::CruiseError::Other(
                    "marker write failure".into(),
                ))
            } else {
                f.manager.save(s)
            }
        },
        |_| panic!("cleanup before marker"),
    )
    .err()
    .ok_or("expected an error")?;
    assert!(error.to_string().contains("marker"));
    let head = git!(&f.base, &["rev-parse", "HEAD"]);
    assert!(f.manager.load(ID)?.local_merge_commit.is_none());
    assert!(matches!(
        f.app.local_merge(ID, LocalMergeMode::MergeAndCleanup)?,
        LocalMergeOutcome::Merged
    ));
    assert_eq!(git!(&f.base, &["rev-parse", "HEAD"]), head);
    assert!(f.manager.load(ID).is_err());
    Ok(())
}

#[test]
fn persisted_intent_before_commit_never_matches_parent_head() -> TestResult {
    let f = Fixture::new()?;
    f.changes()?;
    let head = git!(&f.base, &["rev-parse", "HEAD"]);
    let mut state = f.manager.load(ID)?;
    let _ = cruise::local_merge::merge_session(
        &mut state,
        LocalMergeMode::MergeAndCleanup,
        |s| {
            f.manager.save(s)?;
            Err(cruise::error::CruiseError::Other("interrupted".into()))
        },
        |_| panic!("cleanup before commit"),
    )
    .err()
    .ok_or("expected an error")?;
    assert!(f.manager.load(ID)?.local_merge_intent.is_some());
    assert!(
        f.app
            .local_merge(ID, LocalMergeMode::MergeAndCleanup)
            .is_err()
    );
    assert_eq!(git!(&f.base, &["rev-parse", "HEAD"]), head);
    f.kept()?;
    Ok(())
}

#[test]
fn recovery_rejects_unrelated_commit_with_same_parent_and_tree() -> TestResult {
    let f = Fixture::new()?;
    f.changes()?;
    let mut state = f.manager.load(ID)?;
    let _ = cruise::local_merge::merge_session(
        &mut state,
        LocalMergeMode::MergeAndCleanup,
        |s| {
            if s.local_merge_commit.is_some() {
                Err(cruise::error::CruiseError::Other("marker failure".into()))
            } else {
                f.manager.save(s)
            }
        },
        |_| panic!("cleanup before marker"),
    )
    .err()
    .ok_or("expected an error")?;
    git!(&f.base, &["commit", "--amend", "-m", "unrelated"]);
    let unrelated = git!(&f.base, &["rev-parse", "HEAD"]);
    assert!(
        f.app
            .local_merge(ID, LocalMergeMode::MergeAndCleanup)
            .is_err()
    );
    assert_eq!(git!(&f.base, &["rev-parse", "HEAD"]), unrelated);
    f.kept()?;
    Ok(())
}
