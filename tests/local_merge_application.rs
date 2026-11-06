//! Real-git contract tests for the planned application-level local merge API.
use std::path::{Path, PathBuf};
use std::process::Command;

use cruise::application::{CruiseApplication, LocalMergeMode, LocalMergeOutcome};
use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;

const ID: &str = "20261009000000";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_COUNT", "0")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
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
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base");
        std::fs::create_dir(&base).unwrap();
        git(&base, &["init"]);
        git(&base, &["config", "user.name", "Test"]);
        git(&base, &["config", "user.email", "test@example.com"]);
        std::fs::write(base.join("shared.txt"), "original\n").unwrap();
        git(&base, &["add", "."]);
        git(&base, &["commit", "-m", "initial"]);
        git(&base, &["branch", "-M", "main"]);
        let (ctx, _) = cruise::worktree::setup_session_worktree(
            &base,
            ID,
            "task",
            &tmp.path().join("worktrees"),
            None,
        )
        .unwrap();
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
        let mut json = serde_json::to_value(state).unwrap();
        json["forked_from_branch"] = serde_json::json!("main");
        let state: SessionState = serde_json::from_value(json).unwrap();
        manager.create(&state).unwrap();
        let app = CruiseApplication::new(manager.clone());
        Self {
            _tmp: tmp,
            base,
            worktree: ctx.path,
            branch: ctx.branch,
            manager,
            app,
        }
    }

    fn changes(&self) {
        std::fs::write(self.worktree.join("a.txt"), "committed\n").unwrap();
        git(&self.worktree, &["add", "a.txt"]);
        git(&self.worktree, &["commit", "-m", "work"]);
        std::fs::write(self.worktree.join("b.txt"), "uncommitted\n").unwrap();
    }

    fn kept(&self) {
        assert!(self.manager.load(ID).is_ok(), "session must remain");
        assert!(self.worktree.is_dir(), "worktree must remain");
        assert!(
            !git(&self.base, &["branch", "--list", &self.branch]).is_empty(),
            "branch must remain"
        );
    }
}

#[test]
fn stage_preserves_head_and_session_but_stages_committed_and_uncommitted_work() {
    let f = Fixture::new();
    f.changes();
    let head = git(&f.base, &["rev-parse", "HEAD"]);
    let branch_head = git(&f.worktree, &["rev-parse", "HEAD"]);
    assert!(matches!(
        f.app.local_merge(ID, LocalMergeMode::Stage).unwrap(),
        LocalMergeOutcome::Staged
    ));
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(&f.base, &["diff", "--cached", "--name-only"]),
        "a.txt\nb.txt"
    );
    assert_ne!(git(&f.worktree, &["rev-parse", "HEAD"]), branch_head);
    assert_eq!(git(&f.worktree, &["status", "--porcelain"]), "");
    assert!(
        serde_json::to_value(f.manager.load(ID).unwrap())
            .unwrap()
            .get("local_merge_commit")
            .is_none()
    );
    f.kept();
}

#[test]
fn merge_creates_one_squash_commit_then_removes_session_worktree_and_branch() {
    let f = Fixture::new();
    f.changes();
    let head = git(&f.base, &["rev-parse", "HEAD"]);
    assert!(matches!(
        f.app
            .local_merge(ID, LocalMergeMode::MergeAndCleanup)
            .unwrap(),
        LocalMergeOutcome::Merged
    ));
    assert_eq!(
        git(&f.base, &["rev-list", "--count", &format!("{head}..HEAD")]),
        "1"
    );
    assert_eq!(git(&f.base, &["log", "-1", "--format=%s"]), "task");
    assert_eq!(
        std::fs::read_to_string(f.base.join("a.txt")).unwrap(),
        "committed\n"
    );
    assert_eq!(
        std::fs::read_to_string(f.base.join("b.txt")).unwrap(),
        "uncommitted\n"
    );
    assert!(f.manager.load(ID).is_err());
    assert!(!f.worktree.exists());
    assert_eq!(git(&f.base, &["branch", "--list", &f.branch]), "");
}

#[test]
fn dirty_base_is_refused_before_session_worktree_is_committed() {
    for staged in [false, true] {
        let f = Fixture::new();
        f.changes();
        std::fs::write(f.base.join("shared.txt"), "local changes\n").unwrap();
        if staged {
            git(&f.base, &["add", "shared.txt"]);
        }
        let head = git(&f.base, &["rev-parse", "HEAD"]);
        let branch_head = git(&f.worktree, &["rev-parse", "HEAD"]);
        let status = git(&f.base, &["status", "--porcelain"]);
        let error = f
            .app
            .local_merge(ID, LocalMergeMode::MergeAndCleanup)
            .unwrap_err();
        assert!(
            error.to_string().to_lowercase().contains("uncommitted"),
            "{error}"
        );
        assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), head);
        assert_eq!(git(&f.base, &["status", "--porcelain"]), status);
        assert_eq!(git(&f.worktree, &["rev-parse", "HEAD"]), branch_head);
        assert_eq!(
            std::fs::read_to_string(f.worktree.join("b.txt")).unwrap(),
            "uncommitted\n"
        );
        f.kept();
    }
}

#[test]
fn conflict_reports_path_and_restores_base_without_losing_session() {
    let f = Fixture::new();
    std::fs::write(f.worktree.join("shared.txt"), "session\n").unwrap();
    git(&f.worktree, &["add", "shared.txt"]);
    git(&f.worktree, &["commit", "-m", "session change"]);
    std::fs::write(f.base.join("shared.txt"), "base\n").unwrap();
    git(&f.base, &["add", "shared.txt"]);
    git(&f.base, &["commit", "-m", "base change"]);
    let head = git(&f.base, &["rev-parse", "HEAD"]);
    let error = f
        .app
        .local_merge(ID, LocalMergeMode::MergeAndCleanup)
        .unwrap_err();
    assert!(error.to_string().contains("shared.txt"), "{error}");
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(&f.base, &["status", "--porcelain", "--untracked-files=no"]),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(f.base.join("shared.txt")).unwrap(),
        "base\n"
    );
    f.kept();
}

#[test]
fn untracked_base_file_is_preserved_when_squash_would_overwrite_it() {
    let f = Fixture::new();
    f.changes();
    std::fs::write(f.base.join("a.txt"), "precious\n").unwrap();
    let head = git(&f.base, &["rev-parse", "HEAD"]);
    assert!(f.app.local_merge(ID, LocalMergeMode::Stage).is_err());
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(&f.base, &["status", "--porcelain", "--untracked-files=no"]),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(f.base.join("a.txt")).unwrap(),
        "precious\n"
    );
    f.kept();
}

#[test]
fn switched_session_checkout_is_never_staged_or_cleaned() {
    for mode in [LocalMergeMode::Stage, LocalMergeMode::MergeAndCleanup] {
        let f = Fixture::new();
        f.changes();
        git(&f.worktree, &["checkout", "-b", "other"]);
        let base_head = git(&f.base, &["rev-parse", "HEAD"]);
        let other_head = git(&f.worktree, &["rev-parse", "HEAD"]);
        let session_head = git(&f.base, &["rev-parse", &f.branch]);
        let status = git(&f.worktree, &["status", "--porcelain"]);
        assert!(f.app.local_merge(ID, mode).is_err());
        assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), base_head);
        assert_eq!(git(&f.worktree, &["rev-parse", "HEAD"]), other_head);
        assert_eq!(git(&f.base, &["rev-parse", &f.branch]), session_head);
        assert_eq!(git(&f.worktree, &["status", "--porcelain"]), status);
        f.kept();
    }
}

#[test]
fn cleanup_retry_preserves_changed_worktree_and_advanced_branch() {
    for advance in [false, true] {
        let f = Fixture::new();
        f.changes();
        let mut state = f.manager.load(ID).unwrap();
        let outcome = cruise::local_merge::merge_session(
            &mut state, LocalMergeMode::MergeAndCleanup,
            |s| f.manager.save(s),
            |_| Err(cruise::error::CruiseError::Other("injected cleanup failure".into())),
        ).unwrap();
        assert!(matches!(outcome, LocalMergeOutcome::MergedCleanupFailed { .. }));
        let head = git(&f.base, &["rev-parse", "HEAD"]);
        std::fs::write(f.worktree.join("new.txt"), "new work\n").unwrap();
        if advance {
            git(&f.worktree, &["add", "new.txt"]);
            git(&f.worktree, &["commit", "-m", "new work"]);
        }
        assert!(f.app.local_merge(ID, LocalMergeMode::MergeAndCleanup).is_err());
        assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), head);
        assert_eq!(std::fs::read_to_string(f.worktree.join("new.txt")).unwrap(), "new work\n");
        f.kept();
    }
}

#[test]
fn cleanup_retry_allows_absent_worktree() {
    let f = Fixture::new();
    f.changes();
    let mut state = f.manager.load(ID).unwrap();
    cruise::local_merge::merge_session(
        &mut state, LocalMergeMode::MergeAndCleanup,
        |s| f.manager.save(s),
        |_| Err(cruise::error::CruiseError::Other("injected cleanup failure".into())),
    ).unwrap();
    let head = git(&f.base, &["rev-parse", "HEAD"]);
    git(&f.base, &["worktree", "remove", f.worktree.to_str().unwrap()]);
    assert!(matches!(f.app.local_merge(ID, LocalMergeMode::MergeAndCleanup).unwrap(), LocalMergeOutcome::Merged));
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), head);
    assert!(f.manager.load(ID).is_err());
    assert_eq!(git(&f.base, &["branch", "--list", &f.branch]), "");
}

#[test]
fn persisted_intent_recovers_after_marker_write_failure() {
    let f = Fixture::new();
    f.changes();
    let mut state = f.manager.load(ID).unwrap();
    let error = cruise::local_merge::merge_session(
        &mut state, LocalMergeMode::MergeAndCleanup,
        |s| if s.local_merge_commit.is_some() {
            Err(cruise::error::CruiseError::Other("marker write failure".into()))
        } else { f.manager.save(s) },
        |_| panic!("cleanup before marker"),
    ).unwrap_err();
    assert!(error.to_string().contains("marker"));
    let head = git(&f.base, &["rev-parse", "HEAD"]);
    assert!(f.manager.load(ID).unwrap().local_merge_commit.is_none());
    assert!(matches!(f.app.local_merge(ID, LocalMergeMode::MergeAndCleanup).unwrap(), LocalMergeOutcome::Merged));
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), head);
    assert!(f.manager.load(ID).is_err());
}

#[test]
fn persisted_intent_before_commit_never_matches_parent_head() {
    let f = Fixture::new();
    f.changes();
    let head = git(&f.base, &["rev-parse", "HEAD"]);
    let mut state = f.manager.load(ID).unwrap();
    let _ = cruise::local_merge::merge_session(
        &mut state, LocalMergeMode::MergeAndCleanup,
        |s| { f.manager.save(s)?; Err(cruise::error::CruiseError::Other("interrupted".into())) },
        |_| panic!("cleanup before commit"),
    ).unwrap_err();
    assert!(f.manager.load(ID).unwrap().local_merge_intent.is_some());
    assert!(f.app.local_merge(ID, LocalMergeMode::MergeAndCleanup).is_err());
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), head);
    f.kept();
}

#[test]
fn recovery_rejects_unrelated_commit_with_same_parent_and_tree() {
    let f = Fixture::new();
    f.changes();
    let mut state = f.manager.load(ID).unwrap();
    let _ = cruise::local_merge::merge_session(
        &mut state, LocalMergeMode::MergeAndCleanup,
        |s| if s.local_merge_commit.is_some() {
            Err(cruise::error::CruiseError::Other("marker failure".into()))
        } else { f.manager.save(s) },
        |_| panic!("cleanup before marker"),
    ).unwrap_err();
    git(&f.base, &["commit", "--amend", "-m", "unrelated"]);
    let unrelated = git(&f.base, &["rev-parse", "HEAD"]);
    assert!(f.app.local_merge(ID, LocalMergeMode::MergeAndCleanup).is_err());
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), unrelated);
    f.kept();
}
