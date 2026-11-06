//! End-to-end contracts through the production application service.
use cruise::{
    application::{ApplicationEvent, ApplicationEventSink, CruiseApplication, SessionAction},
    session::{PrSkippedReason, SessionManager, SessionPhase, SessionState},
    session_config::SessionConfigRef,
};
use cucumber::writer::Stats as _;
use cucumber::{World as _, given, then, when};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

#[derive(Default, cucumber::World)]
struct World {
    temp: Option<tempfile::TempDir>,
    manager: Option<SessionManager>,
    id: String,
    marker: PathBuf,
    worktree: PathBuf,
    error: Option<String>,
}
impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World")
            .field("id", &self.id)
            .field("error", &self.error)
            .finish()
    }
}
fn git(path: &Path, args: &[&str]) {
    let result = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git {args:?}: {error}"));
    assert!(
        result.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}
impl World {
    fn manager(&self) -> &SessionManager {
        self.manager
            .as_ref()
            .unwrap_or_else(|| panic!("fixture session manager is missing"))
    }
    fn state(&self) -> SessionState {
        self.manager()
            .load(&self.id)
            .unwrap_or_else(|error| panic!("failed to load session {}: {error}", self.id))
    }
}
fn fixture(w: &mut World, phase: SessionPhase) {
    let temp = tempfile::tempdir()
        .unwrap_or_else(|error| panic!("failed to create temporary directory: {error}"));
    let repo = temp.path().join("repo");
    std::fs::create_dir(&repo)
        .unwrap_or_else(|error| panic!("failed to create fixture repository: {error}"));
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.name", "BDD"]);
    git(&repo, &["config", "user.email", "bdd@example.test"]);
    std::fs::write(repo.join("base"), "base")
        .unwrap_or_else(|error| panic!("failed to write base file: {error}"));
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "base"]);
    let manager = SessionManager::new(temp.path().join("sessions"));
    let id = SessionManager::new_session_id();
    let (worktree, _) = cruise::worktree::setup_session_worktree(
        &repo,
        &id,
        "publication",
        &temp.path().join("worktrees"),
        None,
        &cruise::worktree::WorktreeBase::default(),
    )
    .unwrap_or_else(|error| panic!("failed to create fixture worktree: {error}"));
    let marker = temp.path().join("workflow-ran");
    let workflow = temp.path().join("workflow.yaml");
    std::fs::write(&workflow, format!("create_pr: true\ncleanup_after_pr: false\nsteps:\n  implement:\n    command: 'touch {}'\n", marker.display())).unwrap_or_else(|error| panic!("failed to write fixture workflow: {error}"));
    let mut state = SessionState::new(
        id.clone(),
        repo,
        SessionConfigRef::File { path: workflow },
        "publication".into(),
    );
    state.phase = phase;
    state.worktree_path = Some(worktree.path.clone());
    state.worktree_branch = Some(worktree.branch);
    manager
        .create(&state)
        .unwrap_or_else(|error| panic!("failed to create fixture session: {error}"));
    std::fs::write(state.plan_path(&manager.sessions_dir()), "# Plan\n")
        .unwrap_or_else(|error| panic!("failed to write fixture plan: {error}"));
    w.temp = Some(temp);
    w.manager = Some(manager);
    w.id = id;
    w.marker = marker;
    w.worktree = worktree.path;
}
#[given("a PR Failed session with an unchanged worktree")]
fn failed(w: &mut World) {
    fixture(w, SessionPhase::PrFailed("original error".into()));
}
#[given("a planned session with an unchanged worktree")]
fn planned(w: &mut World) {
    fixture(w, SessionPhase::Planned);
}
#[given("the session has no worktree")]
fn missing(w: &mut World) {
    let mut state = w.state();
    state.worktree_path = None;
    state.worktree_branch = None;
    w.manager()
        .save(&state)
        .unwrap_or_else(|error| panic!("failed to save session without worktree: {error}"));
}
#[given("the base repository is unavailable to Git")]
fn unavailable_base(w: &mut World) {
    let mut state = w.state();
    state.base_dir = w
        .temp
        .as_ref()
        .unwrap_or_else(|| panic!("fixture temporary directory is missing"))
        .path()
        .join("not-a-repository");
    std::fs::create_dir(&state.base_dir)
        .unwrap_or_else(|error| panic!("failed to create non-repository directory: {error}"));
    state.pr_url = Some("https://example.test/existing-pr".into());
    w.manager()
        .save(&state)
        .unwrap_or_else(|error| panic!("failed to save session with unavailable base: {error}"));
}
#[when("the user creates the PR")]
async fn create(w: &mut World) {
    let app = CruiseApplication::new(w.manager().clone());
    let sink: Arc<dyn ApplicationEventSink> = Arc::new(|_: ApplicationEvent| Ok(()));
    w.error = app
        .create_pr(&w.id, sink, None)
        .await
        .err()
        .map(|e| e.to_string());
}
#[then("the session is completed with the no-commits reason")]
fn completed(w: &mut World) {
    assert!(w.error.is_none(), "{:?}", w.error);
    let state = w.state();
    assert_eq!(state.phase, SessionPhase::Completed);
    assert_eq!(state.pr_skipped_reason, Some(PrSkippedReason::NoCommits));
    assert!(state.pr_url.is_none());
}
#[then("the workflow was not rerun")]
fn not_run(w: &mut World) {
    assert!(!w.marker.exists());
}
#[then("the worktree still exists")]
fn worktree_exists(w: &mut World) {
    assert!(w.worktree.exists());
}
#[then("the available actions include Create PR and Reset to Planned but not Retry")]
fn actions(w: &mut World) {
    let app = CruiseApplication::new(w.manager().clone());
    let actions = app.capabilities(&w.state());
    assert!(actions.contains(&SessionAction::CreatePr));
    assert!(actions.contains(&SessionAction::ResetToPlanned));
    assert!(!actions.contains(&SessionAction::Retry));
}
#[then("the operation is rejected and the planned session is unchanged")]
fn rejected_planned(w: &mut World) {
    assert!(
        w.error
            .as_deref()
            .unwrap_or_else(|| panic!("expected planned session operation to be rejected"))
            .contains("only to PR Failed")
    );
    assert_eq!(w.state().phase, SessionPhase::Planned);
}
#[then("the operation is rejected and the PR failure is preserved")]
fn rejected_missing(w: &mut World) {
    assert!(
        w.error
            .as_deref()
            .unwrap_or_else(|| panic!("expected missing worktree operation to be rejected"))
            .contains("worktree")
    );
    assert_eq!(
        w.state().phase,
        SessionPhase::PrFailed("original error".into())
    );
}
#[then("the publication fails and the PR Failed state is saved")]
fn publication_failed(w: &mut World) {
    let error = w
        .error
        .as_deref()
        .unwrap_or_else(|| panic!("publication must fail"));
    assert!(error.contains("git rev-parse HEAD failed"), "{error}");
    let state = w.state();
    assert!(
        matches!(state.phase, SessionPhase::PrFailed(ref message) if message.contains("git rev-parse HEAD failed"))
    );
    assert_eq!(
        state.pr_url.as_deref(),
        Some("https://example.test/existing-pr")
    );
}

#[tokio::main]
async fn main() {
    let writer = World::cucumber()
        .fail_on_skipped()
        .run("tests/features/pr_failed.feature")
        .await;
    assert!(writer.passed_steps() > 0, "no BDD steps executed");
    assert_eq!(writer.failed_steps(), 0);
    assert_eq!(writer.skipped_steps(), 0);
}
