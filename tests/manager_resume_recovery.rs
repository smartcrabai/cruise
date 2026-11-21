use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use cruise::session::{ManagedChildOwner, SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn cli(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cruise"))
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env_remove("CRUISE_CONFIG")
        .env_remove("CRUISE_DAEMON_PORT")
        .stdin(Stdio::null())
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
}
fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
        .trim()
        .to_owned()
}
struct Fixture {
    home: TempDir,
    repo: PathBuf,
    record: PathBuf,
    marker: PathBuf,
    sessions: SessionManager,
}
fn setup_repo(root: &Path) -> (PathBuf, String) {
    fs::create_dir_all(root.join("config/cruise"))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    fs::write(
        root.join("config/cruise/default.yaml"),
        "check_base_ci: false\n",
    )
    .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let repo = root.join("repo");
    fs::create_dir(&repo).unwrap_or_else(|error| panic!("test operation failed: {error}"));
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.test"]);
    fs::write(repo.join("file"), "base\n")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    git(&repo, &["add", "file"]);
    git(&repo, &["commit", "-qm", "base"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["update-ref", "refs/heads/cruise/goal/g", &sha]);
    let origin = "https://github.com/example/project.git";
    git(&repo, &["remote", "add", "origin", origin]);
    (repo, sha)
}

fn write_goal_record(root: &Path, common: &Path, goal: &Value) -> PathBuf {
    let key = Sha256::digest(common.to_string_lossy().as_bytes())
        .iter()
        .fold(String::new(), |mut output, byte| {
            use std::fmt::Write;
            write!(output, "{byte:02x}")
                .unwrap_or_else(|error| panic!("format digest failed: {error}"));
            output
        });
    let record = root.join("data/cruise/manager").join(format!("{key}.json"));
    fs::create_dir_all(
        record
            .parent()
            .unwrap_or_else(|| panic!("expected parent directory")),
    )
    .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    fs::write(
        &record,
        serde_json::to_vec_pretty(
            &json!({"repository":common,"transcript":[],"proposal":null,"goals":[goal]}),
        )
        .unwrap_or_else(|error| panic!("test operation failed: {error}")),
    )
    .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    record
}

impl Fixture {
    fn new(status: &str, pending: bool, starting: bool) -> Self {
        let home = TempDir::new().unwrap_or_else(|error| panic!("test operation failed: {error}"));
        let root = home.path();
        let (repo, sha) = setup_repo(root);
        let origin = "https://github.com/example/project.git";
        let anchor = repo
            .canonicalize()
            .unwrap_or_else(|error| panic!("test operation failed: {error}"));
        let common = repo
            .join(".git")
            .canonicalize()
            .unwrap_or_else(|error| panic!("test operation failed: {error}"));
        let sessions = SessionManager::new(root.join("data/cruise"));
        fs::create_dir_all(sessions.worktrees_dir())
            .unwrap_or_else(|error| panic!("test operation failed: {error}"));
        let worktree = sessions.worktrees_dir().join("child");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "cruise/test-child",
                worktree
                    .to_str()
                    .unwrap_or_else(|| panic!("expected UTF-8 path")),
                &sha,
            ],
        );
        let marker = root.join("started");
        let release = root.join("release");
        #[cfg(not(windows))]
        let command = format!(
            "echo started >> '{}'; while [ ! -f '{}' ]; do sleep 0.1; done",
            marker.display(),
            release.display()
        );
        #[cfg(windows)]
        let command = format!(
            "echo started>>\"{}\" & powershell -NoProfile -Command \"while (!(Test-Path -LiteralPath '{}')) {{ Start-Sleep -Milliseconds 100 }}\"",
            marker.display(),
            release.display().to_string().replace('\'', "''")
        );
        let workflow: cruise::config::WorkflowConfig = serde_json::from_value(json!({
            "create_pr":false, "cleanup_after_pr":false, "force_exec":false,
            "steps":{"run":{"command":command}}
        }))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
        let workflow_json = serde_json::to_value(&workflow)
            .unwrap_or_else(|error| panic!("test operation failed: {error}"));
        let owner = ManagedChildOwner {
            goal_id: "g".into(),
            task_id: "task".into(),
            attempt: 1,
            revision: 1,
            base_sha: sha.clone(),
        };
        let mut child = SessionState::new(
            "child".into(),
            anchor.clone(),
            SessionConfigRef::InlineSnapshot,
            "input".into(),
        );
        child.managed_child = Some(owner.clone());
        child.phase = SessionPhase::Suspended;
        child.input_as_plan = true;
        child.worktree_path = Some(worktree);
        child.worktree_branch = Some("cruise/test-child".into());
        child.worktree_base_ref = Some(sha.clone());
        sessions
            .create_with_config(&child, &workflow)
            .unwrap_or_else(|error| panic!("test operation failed: {error}"));
        fs::write(
            sessions.sessions_dir().join("child/manager-owner.json"),
            serde_json::to_vec(&owner)
                .unwrap_or_else(|error| panic!("test operation failed: {error}")),
        )
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
        fs::write(
            child.plan_path(&sessions.sessions_dir()),
            "# Goal\n\ngoal\n\n# Task\n\nwork\n\n# Acceptance criteria\n\n- c: works\n\n# Exclusions\n\n",
        )
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
        let proposal = json!({"id":"p", "revision":1, "objective":"goal", "target_branch":"main", "target_sha":sha,
            "exclusions":[], "criteria":[{"id":"c","description":"works"}],
            "checks":[{"id":"check","criterion_ids":["c"],"argv":["true"]}],
            "tasks":[{"id":"task","instructions":"work","dependencies":[]}], "workflow":"builtin:tdd",
            "contract":{"anchor":anchor,"origin":origin,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project","workflow":workflow_json}});
        let attempt = json!({"task_id":"task","attempt":1,"session_id":"child","base_sha":sha,"launch_started":true,
            "resume_pending":pending,"resume_starting":starting,"result_sha":null,"integrated":false,"integration_intent":null});
        let goal = json!({"id":"g","proposal":proposal,"status":status,"reason":null,
            "execution":{"anchor":anchor,"origin":origin,"workflow":workflow_json,"integrated_tip":sha,"attempts":[attempt],
            "paused_from":if status == "paused" { json!("active") } else { Value::Null }, "paused_reason":null,"publication":null,"publication_history":[]}});
        let record = write_goal_record(root, &common, &goal);
        Self {
            home,
            repo,
            record,
            marker,
            sessions,
        }
    }
    fn start(&self) -> Value {
        let out = cli(self.home.path(), &["daemon", "start", "--port", "0"]);
        assert!(
            out.status.success(),
            "start: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(
            &fs::read(self.home.path().join("state/cruise/daemon/instance.json"))
                .unwrap_or_else(|error| panic!("test operation failed: {error}")),
        )
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
    }
    fn stop(&self) {
        let out = cli(self.home.path(), &["daemon", "stop", "--force"]);
        assert!(
            out.status.success(),
            "stop: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn goal(&self) -> Value {
        serde_json::from_slice::<Value>(
            &fs::read(&self.record)
                .unwrap_or_else(|error| panic!("test operation failed: {error}")),
        )
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))["goals"][0]
            .clone()
    }
    async fn action(&self, instance: &Value, action: Value) -> reqwest::Response {
        reqwest::Client::new()
            .post(format!(
                "{}api/manager/action",
                instance["origin"]
                    .as_str()
                    .unwrap_or_else(|| panic!("expected string"))
            ))
            .bearer_auth(
                instance["token"]
                    .as_str()
                    .unwrap_or_else(|| panic!("expected string")),
            )
            .json(&json!({"cwd":self.repo,"action":action}))
            .send()
            .await
            .unwrap_or_else(|error| panic!("test operation failed: {error}"))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::write(self.home.path().join("release"), "release");
        let _ = cli(self.home.path(), &["daemon", "stop", "--force"]);
    }
}
async fn eventually(mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for daemon");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn install_integration_intent(
    fixture: &Fixture,
    applied: bool,
    foreign_ref: bool,
) -> (String, String) {
    let mut child = fixture
        .sessions
        .load("child")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let worktree = child
        .worktree_path
        .as_ref()
        .unwrap_or_else(|| panic!("expected worktree"));
    let base = git(worktree, &["rev-parse", "HEAD"]);
    fs::write(worktree.join("file"), "child result\n")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    git(worktree, &["add", "file"]);
    git(worktree, &["commit", "-qm", "child result"]);
    let result = git(worktree, &["rev-parse", "HEAD"]);
    let tree = git(worktree, &["rev-parse", "HEAD^{tree}"]);
    let prepared = git(
        &fixture.repo,
        &[
            "commit-tree",
            &tree,
            "-p",
            &base,
            "-p",
            &result,
            "-m",
            "Integrate Goal task",
        ],
    );
    let reference = if foreign_ref {
        "refs/heads/cruise/goal/other"
    } else {
        "refs/heads/cruise/goal/g"
    };
    if applied {
        git(&fixture.repo, &["update-ref", reference, &prepared]);
    }
    child.phase = SessionPhase::Completed;
    fixture
        .sessions
        .save(&child)
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let mut record: Value = serde_json::from_slice(
        &fs::read(&fixture.record).unwrap_or_else(|error| panic!("test operation failed: {error}")),
    )
    .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let attempt = &mut record["goals"][0]["execution"]["attempts"][0];
    attempt["result_sha"] = json!(result);
    attempt["integration_intent"] = json!({"goal_ref":reference,"expected_tip":base,
        "child_base":base,"child_result":result,"prepared_tip":prepared});
    fs::write(
        &fixture.record,
        serde_json::to_vec_pretty(&record)
            .unwrap_or_else(|error| panic!("test operation failed: {error}")),
    )
    .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    (base, prepared)
}

#[tokio::test]
async fn startup_recovers_applied_integration_for_paused_goal() {
    let fixture = Fixture::new("paused", false, false);
    let (_, prepared) = install_integration_intent(&fixture, true, false);
    fixture.start();
    let goal = fixture.goal();
    assert_eq!(goal["status"], "paused");
    assert_eq!(goal["execution"]["attempts"][0]["integrated"], true);
    assert_eq!(goal["execution"]["integrated_tip"], prepared);
    fixture.stop();
    fixture.start();
    assert_eq!(
        fixture.goal()["execution"]["attempts"][0]["integrated"],
        true
    );
}

#[tokio::test]
async fn startup_leaves_unapplied_paused_integration_pending_across_restart() {
    let fixture = Fixture::new("paused", false, false);
    let (base, _) = install_integration_intent(&fixture, false, false);
    for _ in 0..2 {
        fixture.start();
        let goal = fixture.goal();
        assert_eq!(goal["status"], "paused");
        assert_eq!(goal["execution"]["paused_from"], "active");
        assert_eq!(goal["execution"]["attempts"][0]["integrated"], false);
        assert_eq!(
            git(&fixture.repo, &["rev-parse", "refs/heads/cruise/goal/g"]),
            base
        );
        fixture.stop();
    }
}

#[tokio::test]
async fn startup_resumes_pending_active_integration() {
    let fixture = Fixture::new("active", false, false);
    let (_, prepared) = install_integration_intent(&fixture, false, false);
    fixture.start();
    eventually(|| fixture.goal()["execution"]["attempts"][0]["integrated"] == true).await;
    assert_eq!(fixture.goal()["execution"]["integrated_tip"], prepared);
    assert_eq!(
        git(&fixture.repo, &["rev-parse", "refs/heads/cruise/goal/g"]),
        prepared
    );
}

#[tokio::test]
async fn startup_rejects_intent_for_another_goal_ref() {
    let fixture = Fixture::new("paused", false, false);
    let (base, _) = install_integration_intent(&fixture, true, true);
    fixture.start();
    assert_eq!(
        fixture.goal()["execution"]["attempts"][0]["integrated"],
        false
    );
    assert_eq!(fixture.goal()["execution"]["paused_from"], "blocked");
    assert_eq!(
        git(&fixture.repo, &["rev-parse", "refs/heads/cruise/goal/g"]),
        base
    );
}

#[tokio::test]
async fn startup_blocks_drifted_goal_ref_without_moving_it() {
    let fixture = Fixture::new("paused", false, false);
    install_integration_intent(&fixture, false, false);
    let drift = git(&fixture.repo, &["rev-parse", "cruise/test-child"]);
    git(
        &fixture.repo,
        &["update-ref", "refs/heads/cruise/goal/g", &drift],
    );
    fixture.start();
    let goal = fixture.goal();
    assert_eq!(goal["status"], "paused");
    assert_eq!(goal["execution"]["paused_from"], "blocked");
    assert_eq!(goal["execution"]["attempts"][0]["integrated"], false);
    assert_eq!(
        git(&fixture.repo, &["rev-parse", "refs/heads/cruise/goal/g"]),
        drift
    );
}

#[tokio::test]
async fn startup_recovers_applied_intent_without_child_session() {
    let fixture = Fixture::new("paused", false, false);
    let (_, prepared) = install_integration_intent(&fixture, true, false);
    fs::remove_dir_all(fixture.sessions.sessions_dir().join("child"))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    fixture.start();
    let goal = fixture.goal();
    assert_eq!(goal["execution"]["attempts"][0]["integrated"], true);
    assert_eq!(goal["execution"]["integrated_tip"], prepared);
}

#[tokio::test]
async fn paused_pending_resume_survives_restart_then_runs_once_after_goal_resume() {
    let fixture = Fixture::new("paused", true, false);
    let first = fixture.start();
    assert!(
        !fixture
            .action(
                &first,
                json!({"type":"resume_task","goal_id":"g","task_id":"task"})
            )
            .await
            .status()
            .is_success()
    );
    assert_eq!(
        fixture.goal()["execution"]["attempts"][0]["resume_pending"],
        true
    );
    assert!(!fixture.marker.exists());
    fixture.stop();
    let second = fixture.start();
    let goal = fixture.goal();
    assert_eq!(goal["status"], "paused");
    assert_eq!(goal["execution"]["attempts"][0]["resume_pending"], true);
    assert_eq!(
        fixture
            .sessions
            .load("child")
            .unwrap_or_else(|error| panic!("test operation failed: {error}"))
            .phase,
        SessionPhase::Suspended
    );
    assert!(!fixture.marker.exists());
    let response = fixture
        .action(&second, json!({"type":"resume","goal_id":"g"}))
        .await;
    assert!(
        response.status().is_success(),
        "resume: {}",
        response
            .text()
            .await
            .unwrap_or_else(|error| panic!("test operation failed: {error}"))
    );
    let started = Instant::now();
    eventually(|| {
        assert!(
            started.elapsed() <= Duration::from_secs(15),
            "resume did not start: goal={} child={:?} daemon_log={}",
            fixture.goal(),
            fixture
                .sessions
                .load("child")
                .unwrap_or_else(|error| panic!("test operation failed: {error}"))
                .phase,
            fs::read_to_string(fixture.home.path().join("state/cruise/daemon/daemon.log"))
                .unwrap_or_default()
        );
        fixture.marker.exists()
    })
    .await;
    assert_eq!(
        fs::read_to_string(&fixture.marker)
            .unwrap_or_else(|error| panic!("test operation failed: {error}"))
            .lines()
            .collect::<Vec<_>>(),
        vec!["started"]
    );
    let goal = fixture.goal();
    assert_eq!(
        goal["execution"]["attempts"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .len(),
        1
    );
    assert_eq!(goal["execution"]["attempts"][0]["session_id"], "child");
    assert_eq!(goal["execution"]["attempts"][0]["resume_pending"], false);
    let paused = fixture
        .action(&second, json!({"type":"pause","goal_id":"g"}))
        .await;
    assert!(paused.status().is_success());
    eventually(|| {
        matches!(
            fixture
                .sessions
                .load("child")
                .unwrap_or_else(|error| panic!("test operation failed: {error}"))
                .phase,
            SessionPhase::Suspended | SessionPhase::Failed(_)
        )
    })
    .await;
    assert_eq!(fixture.goal()["status"], "paused");
    assert_eq!(
        fs::read_to_string(&fixture.marker)
            .unwrap_or_else(|error| panic!("test operation failed: {error}"))
            .lines()
            .collect::<Vec<_>>(),
        vec!["started"]
    );
}

#[tokio::test]
async fn explicit_child_resume_runs_existing_attempt_and_pause_stops_it() {
    let fixture = Fixture::new("active", false, false);
    let instance = fixture.start();
    let response = fixture
        .action(
            &instance,
            json!({"type":"resume_task","goal_id":"g","task_id":"task"}),
        )
        .await;
    assert!(
        response.status().is_success(),
        "resume child: {}",
        response
            .text()
            .await
            .unwrap_or_else(|error| panic!("test operation failed: {error}"))
    );
    eventually(|| fixture.marker.exists()).await;
    let goal = fixture.goal();
    assert_eq!(
        goal["execution"]["attempts"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .len(),
        1
    );
    assert_eq!(goal["execution"]["attempts"][0]["session_id"], "child");
    assert_eq!(
        fs::read_to_string(&fixture.marker)
            .unwrap_or_else(|error| panic!("test operation failed: {error}"))
            .lines()
            .collect::<Vec<_>>(),
        vec!["started"]
    );
    let response = fixture
        .action(&instance, json!({"type":"pause","goal_id":"g"}))
        .await;
    assert!(response.status().is_success());
    eventually(|| {
        matches!(
            fixture
                .sessions
                .load("child")
                .unwrap_or_else(|error| panic!("test operation failed: {error}"))
                .phase,
            SessionPhase::Suspended | SessionPhase::Failed(_)
        )
    })
    .await;
    assert_eq!(fixture.goal()["status"], "paused");
    assert_eq!(
        fs::read_to_string(&fixture.marker)
            .unwrap_or_else(|error| panic!("test operation failed: {error}"))
            .lines()
            .collect::<Vec<_>>(),
        vec!["started"]
    );
}

#[tokio::test]
async fn interrupted_resume_dispatch_blocks_without_reexecuting_child() {
    let fixture = Fixture::new("active", false, true);
    let _instance = fixture.start();
    eventually(|| fixture.goal()["status"] == "blocked").await;
    let goal = fixture.goal();
    assert!(
        !goal["reason"]
            .as_str()
            .unwrap_or_else(|| panic!("expected string"))
            .is_empty(),
        "{goal}"
    );
    assert_eq!(goal["execution"]["attempts"][0]["resume_starting"], true);
    assert_eq!(
        fixture
            .sessions
            .load("child")
            .unwrap_or_else(|error| panic!("test operation failed: {error}"))
            .phase,
        SessionPhase::Suspended
    );
    assert!(!fixture.marker.exists());
}

#[tokio::test]
async fn invalid_child_plan_records_durable_resume_failure_without_duplicate_attempt() {
    let fixture = Fixture::new("active", false, false);
    let child = fixture
        .sessions
        .load("child")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let plan = child.plan_path(&fixture.sessions.sessions_dir());
    fs::write(&plan, "# Changed without approval\n")
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let instance = fixture.start();
    let response = fixture
        .action(
            &instance,
            json!({"type":"resume_task","goal_id":"g","task_id":"task"}),
        )
        .await;
    assert!(
        response.status().is_success(),
        "resume request: {}",
        response
            .text()
            .await
            .unwrap_or_else(|error| panic!("test operation failed: {error}"))
    );
    eventually(|| {
        fixture.goal()["reason"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty())
    })
    .await;
    let goal = fixture.goal();
    let reason = goal["reason"]
        .as_str()
        .unwrap_or_else(|| panic!("expected string"))
        .to_owned();
    assert_eq!(goal["status"], "active");
    assert_eq!(
        goal["execution"]["attempts"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .len(),
        1
    );
    assert_eq!(goal["execution"]["attempts"][0]["session_id"], "child");
    assert_eq!(goal["execution"]["attempts"][0]["resume_pending"], false);
    assert_eq!(goal["execution"]["attempts"][0]["resume_starting"], false);
    assert_eq!(
        fs::read_to_string(&plan).unwrap_or_else(|error| panic!("test operation failed: {error}")),
        "# Changed without approval\n"
    );
    assert!(!fixture.marker.exists());
    fixture.stop();
    let _restarted = fixture.start();
    let recovered = fixture.goal();
    assert_eq!(recovered["status"], "active");
    assert_eq!(recovered["reason"], reason);
    assert_eq!(
        recovered["execution"]["attempts"]
            .as_array()
            .unwrap_or_else(|| panic!("expected array"))
            .len(),
        1
    );
    assert_eq!(recovered["execution"]["attempts"][0]["session_id"], "child");
    assert!(!fixture.marker.exists());
}
