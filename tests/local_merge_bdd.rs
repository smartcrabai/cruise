//! Real-git application contracts for the local squash merge lifecycle.
#![expect(clippy::unwrap_used, reason = "isolated contract fixtures")]

use std::path::{Path, PathBuf};
use std::process::Command;

use cruise::application::{CruiseApplication, LocalMergeMode, LocalMergeOutcome};
use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use cucumber::{World as _, given, then, when};

const ID: &str = "20261009000000";

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_COUNT", "0")
        .output()
        .unwrap();
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
        state.forked_from_branch = Some("main".into());
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
    fn kept(&self) {
        assert!(self.manager.load(ID).is_ok());
        assert!(self.worktree.is_dir());
        assert!(!git(&self.base, &["branch", "--list", &self.branch]).is_empty());
    }
    fn cleaned(&self) {
        assert!(self.manager.load(ID).is_err());
        assert!(!self.worktree.exists());
        assert!(git(&self.base, &["branch", "--list", &self.branch]).is_empty());
    }
}

#[derive(Default, cucumber::World)]
pub struct LocalMergeWorld {
    fixture: Option<Fixture>,
    head: String,
    branch_head: String,
    status: String,
    outcome: Option<LocalMergeOutcome>,
    error: Option<String>,
}

impl std::fmt::Debug for LocalMergeWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalMergeWorld").finish_non_exhaustive()
    }
}

#[given("a completed local worktree session without a pull request")]
fn session(w: &mut LocalMergeWorld) {
    w.fixture = Some(Fixture::new());
}

#[given("the session has committed and uncommitted changes")]
fn changes(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    std::fs::write(f.worktree.join("a.txt"), "committed\n").unwrap();
    git(&f.worktree, &["add", "a.txt"]);
    git(&f.worktree, &["commit", "-m", "work"]);
    std::fs::write(f.worktree.join("b.txt"), "uncommitted\n").unwrap();
}

#[given("the base has a tracked modification")]
fn dirty(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    std::fs::write(f.base.join("shared.txt"), "local changes\n").unwrap();
}

#[given("the base and session have conflicting commits")]
fn conflict(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    std::fs::write(f.worktree.join("shared.txt"), "session\n").unwrap();
    git(&f.worktree, &["add", "shared.txt"]);
    git(&f.worktree, &["commit", "-m", "session change"]);
    std::fs::write(f.base.join("shared.txt"), "base\n").unwrap();
    git(&f.base, &["add", "shared.txt"]);
    git(&f.base, &["commit", "-m", "base change"]);
}

#[given("the session worktree is locked against removal")]
fn lock(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    git(&f.base, &["worktree", "lock", f.worktree.to_str().unwrap()]);
}

#[given("the base is checked out on a different branch")]
fn wrong_branch(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    git(&f.base, &["checkout", "-b", "other"]);
}

#[given("an untracked base file would be overwritten by the merge")]
fn untracked_collision(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    std::fs::write(f.base.join("a.txt"), "precious\n").unwrap();
}

#[when("I merge locally and clean up")]
fn merge(w: &mut LocalMergeWorld) {
    invoke(w, LocalMergeMode::MergeAndCleanup);
}

#[when("I stage the local merge")]
fn stage(w: &mut LocalMergeWorld) {
    invoke(w, LocalMergeMode::Stage);
}

fn invoke(w: &mut LocalMergeWorld, mode: LocalMergeMode) {
    let f = w.fixture.as_ref().unwrap();
    w.head = git(&f.base, &["rev-parse", "HEAD"]);
    w.branch_head = git(&f.worktree, &["rev-parse", "HEAD"]);
    w.status = git(&f.base, &["status", "--porcelain"]);
    w.outcome = None;
    w.error = None;
    match f.app.local_merge(ID, mode) {
        Ok(outcome) => w.outcome = Some(outcome),
        Err(error) => w.error = Some(error.to_string()),
    }
}

#[then("both changes are staged without a base commit and the session is retained")]
fn staged(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    assert_eq!(w.outcome, Some(LocalMergeOutcome::Staged));
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), w.head);
    assert_eq!(
        git(&f.base, &["diff", "--cached", "--name-only"]),
        "a.txt\nb.txt"
    );
    assert_eq!(git(&f.base, &["show", ":a.txt"]), "committed");
    assert_eq!(git(&f.base, &["show", ":b.txt"]), "uncommitted");
    assert_ne!(git(&f.worktree, &["rev-parse", "HEAD"]), w.branch_head);
    assert_eq!(git(&f.worktree, &["status", "--porcelain"]), "");
    assert!(f.manager.load(ID).unwrap().local_merge_commit.is_none());
    f.kept();
}

#[then("the wrong branch is reported and both checkouts are retained unchanged")]
fn wrong_branch_refused(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    let error = w.error.as_ref().unwrap();
    assert!(error.contains("other") && error.contains("main"), "{error}");
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), w.head);
    assert_eq!(git(&f.base, &["status", "--porcelain"]), w.status);
    assert_eq!(git(&f.worktree, &["rev-parse", "HEAD"]), w.branch_head);
    assert_eq!(
        std::fs::read_to_string(f.worktree.join("b.txt")).unwrap(),
        "uncommitted\n"
    );
    f.kept();
}

#[then("the untracked base file and session are retained")]
fn collision_refused(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    assert!(w.error.is_some());
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), w.head);
    assert_eq!(
        git(&f.base, &["status", "--porcelain", "--untracked-files=no"]),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(f.base.join("a.txt")).unwrap(),
        "precious\n"
    );
    assert_eq!(
        std::fs::read_to_string(f.worktree.join("b.txt")).unwrap(),
        "uncommitted\n"
    );
    f.kept();
}

#[then("one squash commit contains both changes and the session is cleaned up")]
fn merged(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    assert!(matches!(w.outcome, Some(LocalMergeOutcome::Merged)));
    assert_eq!(
        git(
            &f.base,
            &["rev-list", "--count", &format!("{}..HEAD", w.head)]
        ),
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
    f.cleaned();
}

#[then("the dirty base and session are unchanged")]
fn refused(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    assert!(
        w.error
            .as_ref()
            .unwrap()
            .to_lowercase()
            .contains("uncommitted")
    );
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), w.head);
    assert_eq!(git(&f.base, &["status", "--porcelain"]), w.status);
    assert_eq!(git(&f.worktree, &["rev-parse", "HEAD"]), w.branch_head);
    assert_eq!(
        std::fs::read_to_string(f.base.join("shared.txt")).unwrap(),
        "local changes\n"
    );
    assert_eq!(
        std::fs::read_to_string(f.worktree.join("b.txt")).unwrap(),
        "uncommitted\n"
    );
    f.kept();
}

#[then("the conflict is reported and the base is restored with the session retained")]
fn restored(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    assert!(w.error.as_ref().unwrap().contains("shared.txt"));
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), w.head);
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

#[then("the merge commit is recorded and cleanup fails")]
fn cleanup_failed(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    assert!(matches!(
        w.outcome,
        Some(LocalMergeOutcome::MergedCleanupFailed { .. })
    ));
    let head = git(&f.base, &["rev-parse", "HEAD"]);
    assert_eq!(
        git(
            &f.base,
            &["rev-list", "--count", &format!("{}..HEAD", w.head)]
        ),
        "1"
    );
    assert_eq!(
        f.manager.load(ID).unwrap().local_merge_commit.as_deref(),
        Some(head.as_str())
    );
    f.kept();
    w.head = head;
}

#[when("I unlock the session worktree and retry cleanup")]
fn retry(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    git(
        &f.base,
        &["worktree", "unlock", f.worktree.to_str().unwrap()],
    );
    w.outcome = Some(
        f.app
            .local_merge(ID, LocalMergeMode::MergeAndCleanup)
            .unwrap(),
    );
}

#[then("cleanup succeeds without another commit")]
fn retry_succeeds(w: &mut LocalMergeWorld) {
    let f = w.fixture.as_ref().unwrap();
    assert!(matches!(w.outcome, Some(LocalMergeOutcome::Merged)));
    assert_eq!(git(&f.base, &["rev-parse", "HEAD"]), w.head);
    f.cleaned();
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    LocalMergeWorld::cucumber()
        .max_concurrent_scenarios(1)
        .fail_on_skipped()
        .run_and_exit(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/local_merge.feature"
        ))
        .await;
}
