//! Production-connected approval boundary scenarios, using isolated Git repositories.
use cruise::manager::{self, Check, Criterion, GoalStatus, Proposal, Task};
use cucumber::{World as _, given, then, when};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{fmt::Write as _, fs, path::Path, process::Command, time::Duration};

// The embedded fixture module also supports libtest-only subprocess entrypoints.
#[allow(dead_code, unused_imports)]
#[path = "support/manager_cli.rs"]
mod support;

#[derive(Debug, Default, cucumber::World)]
struct World {
    root: Option<tempfile::TempDir>,
    first: Option<Proposal>,
    current: Option<Proposal>,
    goal_ids: Vec<String>,
    verification: Option<Verification>,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

impl World {
    fn repo(&self) -> &Path {
        self.root.as_ref().unwrap().path()
    }
    fn input(repo: &Path) -> Proposal {
        Proposal {
            id: "provider-id".into(),
            revision: 42,
            objective: "Deliver goal".into(),
            target_branch: "main".into(),
            target_sha: git(repo, &["rev-parse", "HEAD"]),
            exclusions: vec![],
            criteria: vec![Criterion {
                id: "c".into(),
                description: "Works".into(),
            }],
            checks: vec![Check {
                id: "check".into(),
                criterion_ids: vec!["c".into()],
                argv: vec!["true".into()],
            }],
            tasks: vec![Task {
                id: "task".into(),
                instructions: "Implement".into(),
                dependencies: vec![],
            }],
            workflow: "builtin:tdd".into(),
            contract: None,
        }
    }
}

#[given("an isolated repository with a manager proposal")]
fn repository(world: &mut World) {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path();
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.name", "Contract Test"]);
    git(repo, &["config", "user.email", "contract@example.test"]);
    std::fs::write(repo.join("file"), "base\n").unwrap();
    git(repo, &["add", "file"]);
    git(repo, &["commit", "-qm", "base"]);
    let sha = git(repo, &["rev-parse", "HEAD"]);
    git(
        repo,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    );
    git(repo, &["update-ref", "refs/remotes/origin/main", &sha]);
    world.root = Some(root);
    let p = manager::save_proposal(world.repo(), World::input(world.repo())).unwrap();
    let snapshot = manager::snapshot(world.repo()).unwrap();
    assert_eq!(snapshot.proposal.unwrap().id, p.id);
    assert!(snapshot.goals.is_empty());
    world.first = Some(p.clone());
    world.current = Some(p);
}

#[when("the proposal is replaced by a newer preview")]
fn replace(world: &mut World) {
    let current = manager::save_proposal(world.repo(), World::input(world.repo())).unwrap();
    let first = world.first.as_ref().unwrap();
    assert_ne!(first.id, current.id);
    assert_eq!(first.revision + 1, current.revision);
    world.current = Some(current);
}

#[when("approvals are attempted with the old identity and an old revision")]
fn stale(world: &mut World) {
    let first = world.first.as_ref().unwrap();
    let current = world.current.as_ref().unwrap();
    assert!(manager::approve(world.repo(), &first.id, first.revision).is_err());
    assert!(manager::approve(world.repo(), &current.id, first.revision).is_err());
}

#[then("the newer preview remains and no goal or child attempt exists")]
fn no_goal(world: &mut World) {
    let snapshot = manager::snapshot(world.repo()).unwrap();
    let p = snapshot.proposal.unwrap();
    let expected = world.current.as_ref().unwrap();
    assert_eq!((p.id, p.revision), (expected.id.clone(), expected.revision));
    assert!(snapshot.goals.is_empty());
}

#[when("the current proposal is approved twice")]
fn approve(world: &mut World) {
    let p = world.current.as_ref().unwrap();
    for _ in 0..2 {
        world.goal_ids.push(
            manager::approve(world.root.as_ref().unwrap().path(), &p.id, p.revision)
                .unwrap()
                .id,
        );
    }
}

#[then(
    "one active goal exists for that exact proposal at its frozen target tip without child attempts"
)]
fn one_goal(world: &mut World) {
    let snapshot = manager::snapshot(world.repo()).unwrap();
    assert!(snapshot.proposal.is_none());
    assert_eq!(snapshot.goals.len(), 1);
    let goal = &snapshot.goals[0];
    let p = world.current.as_ref().unwrap();
    assert_eq!(world.goal_ids, [goal.id.clone(), goal.id.clone()]);
    assert_eq!(
        (&goal.proposal.id, goal.proposal.revision),
        (&p.id, p.revision)
    );
    assert_eq!(goal.status, GoalStatus::Active);
    let contract = p
        .contract
        .as_ref()
        .expect("proposal has a frozen execution contract");
    assert_eq!(
        goal.execution.integrated_tip.as_deref(),
        Some(p.target_sha.as_str())
    );
    assert_eq!(goal.execution.anchor.as_ref(), Some(&contract.anchor));
    assert_eq!(
        goal.execution.origin.as_deref(),
        Some(contract.origin.as_str())
    );
    assert_eq!(
        serde_json::to_value(goal.execution.workflow.as_ref()).unwrap(),
        serde_json::to_value(Some(&contract.workflow)).unwrap()
    );
    assert!(goal.execution.attempts.is_empty());
}

struct Verification {
    fixture: support::Fixture,
    before: Value,
    retried: Value,
    approved: Value,
}

impl std::fmt::Debug for Verification {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Verification")
            .field("repo", &self.fixture.repo)
            .finish_non_exhaustive()
    }
}

impl Verification {
    fn seed() -> Self {
        let fixture = support::Fixture::new();
        let repo = &fixture.repo;
        let mut input = World::input(repo);
        input.checks[0].argv = vec!["false".into()];
        let proposal = manager::save_proposal(repo, input).unwrap();
        let goal = manager::approve(repo, &proposal.id, proposal.revision).unwrap();
        let mut snapshot = serde_json::to_value(manager::snapshot(repo).unwrap()).unwrap();
        let common = repo.join(".git").canonicalize().unwrap();
        let base = &proposal.target_sha;
        fs::write(repo.join("integrated-change"), "original task result\n").unwrap();
        git(repo, &["add", "integrated-change"]);
        git(
            repo,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.test",
                "commit",
                "-qm",
                "Integrated task result",
            ],
        );
        let sha = git(repo, &["rev-parse", "HEAD"]);
        let branch = format!("cruise/goal/{}", goal.id);
        git(repo, &["update-ref", &format!("refs/heads/{branch}"), &sha]);
        let stored = &mut snapshot["goals"][0];
        stored["status"] = json!("blocked");
        stored["reason"] = json!("Approved check failed");
        let execution = &mut stored["execution"];
        execution["integrated_tip"] = json!(sha);
        execution
            .as_object_mut()
            .unwrap()
            .remove("corrective_discarded");
        execution["attempts"] = json!([{
            "task_id":"task", "attempt":1, "session_id":"original-child", "base_sha":base,
            "launch_started":true, "result_sha":sha, "integrated":true
        }]);
        execution["publication"] = json!({
            "tip":sha, "worktree":fixture.home.join("previous-verification"), "branch":branch,
            "phase":"Ready", "forge_kind":"GitHub", "forge_host":"github.com",
            "forge_repository":"example/project", "checks":[{
                "check_id":"check", "verified_tip":sha, "argv":["false"], "exit_code":1,
                "cancelled":false, "stdout_log":"out", "stderr_log":"err"
            }]
        });
        let dir = fixture.home.join("data/cruise/manager");
        fs::create_dir_all(&dir).unwrap();
        let digest = Sha256::digest(common.to_string_lossy().as_bytes());
        let mut key = String::with_capacity(digest.len() * 2);
        for byte in digest {
            write!(key, "{byte:02x}").unwrap();
        }
        fs::write(
            dir.join(format!("{key}.json")),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        let before = fixture.snapshot()["goals"][0].clone();
        assert_eq!(before["execution"]["corrective_discarded"], json!([]));
        Self {
            fixture,
            before,
            retried: Value::Null,
            approved: Value::Null,
        }
    }

    fn goal(&self) -> Value {
        self.fixture.snapshot()["goals"][0].clone()
    }

    async fn preview(&self, instructions: &str) -> Value {
        self.fixture
            .api(Some(json!({
                "type":"preview_corrective", "goal_id":self.before["id"],
                "expected_tip":self.before["execution"]["integrated_tip"],
                "task":{"id":"fix", "instructions":instructions, "dependencies":["task"]}
            })))
            .await
    }

    fn approval(&self, preview: &Value) -> Value {
        json!({"type":"approve_corrective", "goal_id":self.before["id"],
            "preview_id":preview["id"], "revision":preview["revision"]})
    }

    async fn assert_discard_is_stale(&self) {
        let action = self.approval(&self.before["execution"]["corrective_preview"]);
        let response = self.fixture.api_response(Some(action)).await;
        assert_eq!(response.status(), reqwest::StatusCode::CONFLICT);
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("stale or unknown corrective preview")
        );
    }

    fn assert_retry(&self, discarded: Value) {
        let old = &self.before["execution"];
        let new = &self.retried["execution"];
        assert_eq!(self.retried["status"], "active");
        assert_eq!(self.retried["proposal"], self.before["proposal"]);
        let mut attempts = old["attempts"].clone();
        for attempt in attempts.as_array_mut().unwrap() {
            attempt.as_object_mut().unwrap().remove("child_state");
        }
        assert_eq!(new["attempts"], attempts);
        assert_eq!(new["integrated_tip"], old["integrated_tip"]);
        assert!(new["publication"].is_null());
        assert_eq!(new["publication_history"], json!([old["publication"]]));
        assert!(new["corrective_preview"].is_null());
        assert_eq!(new["corrective_discarded"], discarded);
        assert_eq!(new["corrective_approvals"], json!([]));
    }
}

#[given("a blocked Manager goal with a failed check but no corrective preview")]
fn failed_goal(world: &mut World) {
    world.verification = Some(Verification::seed());
}

#[given("a blocked Manager goal with a failed check and an unapproved corrective preview")]
async fn failed_goal_with_preview(world: &mut World) {
    let mut verification = Verification::seed();
    let preview = verification.preview("Repair the first failure").await;
    verification.before = verification.goal();
    assert_eq!(
        verification.before["execution"]["corrective_preview"],
        preview
    );
    assert!(support::success(verification.fixture.manager(&["status"])).contains(PREVIEW_NOTICE));
    world.verification = Some(verification);
}

#[given("a paused Manager goal with a pending corrective preview")]
async fn paused_goal(world: &mut World) {
    failed_goal_with_preview(world).await;
    let verification = world.verification.as_mut().unwrap();
    let output = verification
        .fixture
        .manager(&["pause", verification.before["id"].as_str().unwrap()]);
    // The fixture has no live child to signal, but the pause intent is durable.
    assert!(
        output.status.success()
            || String::from_utf8_lossy(&output.stderr).contains("Goal pause was recorded"),
        "{output:?}"
    );
    verification.before = verification.goal();
    assert_eq!(verification.before["status"], "paused");
    assert!(verification.before["execution"]["corrective_preview"].is_object());
}

#[when("the operator retries goal verification")]
async fn retry(world: &mut World) {
    let verification = world.verification.as_mut().unwrap();
    verification.retried = verification
        .fixture
        .api(Some(
            json!({"type":"retry", "goal_id":verification.before["id"]}),
        ))
        .await;
}

#[when("the operator attempts to retry goal verification")]
fn rejected_retry(world: &mut World) {
    let verification = world.verification.as_ref().unwrap();
    let output = verification
        .fixture
        .manager(&["retry", verification.before["id"].as_str().unwrap()]);
    assert!(
        !output.status.success(),
        "unexpected successful retry: {output:?}"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("Goal is not blocked"));
}

#[then("the rejected retry leaves the goal unchanged")]
fn unchanged_goal(world: &mut World) {
    let verification = world.verification.as_ref().unwrap();
    assert_eq!(verification.goal(), verification.before);
}

#[then("the retry archives the failed verification without creating a discard entry")]
fn nothing_discarded(world: &mut World) {
    world.verification.as_ref().unwrap().assert_retry(json!([]));
}

#[then("the retry archives the failed verification and the preview without authorizing work")]
fn preview_discarded(world: &mut World) {
    let verification = world.verification.as_ref().unwrap();
    verification.assert_retry(json!([
        verification.before["execution"]["corrective_preview"]
    ]));
}

const PREVIEW_NOTICE: &str = "Corrective preview is not approved";

#[then("Manager status no longer displays the unapproved preview")]
fn notice_removed(world: &mut World) {
    let verification = world.verification.as_ref().unwrap();
    assert!(!support::success(verification.fixture.manager(&["status"])).contains(PREVIEW_NOTICE));
    let goal = verification.goal();
    assert_eq!(
        goal["execution"].get("corrective_preview"),
        Some(&Value::Null)
    );
    assert_eq!(
        goal["execution"]["corrective_discarded"],
        verification.retried["execution"]["corrective_discarded"]
    );
}

#[when("verification fails again at the same integrated tip")]
async fn fails_again(world: &mut World) {
    let verification = world.verification.as_ref().unwrap();
    let goal = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let goal = verification.goal();
            if goal["status"] == "blocked" {
                break goal;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or_else(|err| {
        panic!(
            "verification did not re-block: {err}; {}",
            verification.goal()
        )
    });
    let new = &goal["execution"];
    let old = &verification.before["execution"];
    assert_eq!(new["integrated_tip"], old["integrated_tip"]);
    assert_eq!(
        new["publication"]["checks"][0]["verified_tip"], old["integrated_tip"],
        "verification did not record a failed check: {}",
        goal["reason"]
    );
    assert_eq!(new["publication"]["checks"][0]["exit_code"], 1, "{goal}");
    assert_ne!(
        new["publication"]["worktree"],
        old["publication"]["worktree"]
    );
}

#[then("approval of the discarded identity and revision is stale")]
async fn stale_discard(world: &mut World) {
    let verification = world.verification.as_ref().unwrap();
    let before = verification.goal();
    verification.assert_discard_is_stale().await;
    assert_eq!(verification.goal(), before);
}

#[when("the operator saves and approves a fresh corrective preview")]
async fn approve_fresh(world: &mut World) {
    let verification = world.verification.as_mut().unwrap();
    let fresh = verification.preview("Repair the new failure").await;
    let discarded = &verification.before["execution"]["corrective_preview"];
    assert_ne!(fresh["id"], discarded["id"]);
    assert_eq!(
        fresh["revision"].as_u64().unwrap(),
        discarded["revision"].as_u64().unwrap() + 1
    );
    verification.assert_discard_is_stale().await;
    verification.approved = verification
        .fixture
        .api(Some(verification.approval(&fresh)))
        .await;
    assert_eq!(
        verification.approved["execution"]["corrective_approvals"],
        json!([fresh])
    );
}

#[then(
    "only the fresh revision authorizes corrective work and the discarded approval remains stale"
)]
async fn only_fresh_approved(world: &mut World) {
    let verification = world.verification.as_ref().unwrap();
    let goal = &verification.approved;
    let execution = &goal["execution"];
    let fresh = &execution["corrective_approvals"][0];
    let original = &verification.before["proposal"];
    assert!(execution["corrective_preview"].is_null());
    assert_eq!(
        execution["corrective_discarded"],
        verification.retried["execution"]["corrective_discarded"]
    );
    assert_eq!(
        goal["proposal"]["tasks"],
        json!([original["tasks"][0], fresh["task"]])
    );
    assert_eq!(goal["proposal"]["checks"], original["checks"]);
    assert_eq!(goal["proposal"]["criteria"], original["criteria"]);
    assert_eq!(
        execution["publication_history"].as_array().unwrap().len(),
        2
    );
    verification.assert_discard_is_stale().await;
    let repeated = verification
        .fixture
        .api(Some(verification.approval(fresh)))
        .await;
    assert_eq!(repeated["proposal"]["tasks"], goal["proposal"]["tasks"]);
    assert_eq!(
        repeated["execution"]["corrective_approvals"],
        execution["corrective_approvals"]
    );
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Set once before Cucumber starts, never concurrently with other threads.
    let data = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("HOME", data.path());
        std::env::set_var("XDG_DATA_HOME", data.path().join("data"));
        std::env::set_var("XDG_CONFIG_HOME", data.path().join("config"));
        std::env::set_var("XDG_STATE_HOME", data.path().join("state"));
    }
    World::cucumber()
        .max_concurrent_scenarios(1)
        .fail_on_skipped()
        .run_and_exit(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/manager_approval.feature"
        ))
        .await;
}
