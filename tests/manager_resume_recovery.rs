use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
use base64::Engine as _;
use cruise::manager::{Goal, GoalStatus};
use cruise::session::{ManagedChildOwner, SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const TEST_TIMEOUT: Duration = Duration::from_secs(120);

fn cli(home: &Path, args: &[&str]) -> Output {
    cli_in(home, home, args)
}

fn cli_in(home: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_cruise"))
        .current_dir(cwd)
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

#[cfg(windows)]
fn windows_marker_command(marker: &Path, release: &Path) -> String {
    let quote_path = |path: &Path| path.to_string_lossy().replace('\'', "''");
    let script = format!(
        "$marker = '{}'; $release = '{}'; [System.IO.File]::AppendAllText($marker, 'started' + [Environment]::NewLine); while (!(Test-Path -LiteralPath $release)) {{ Start-Sleep -Milliseconds 100 }}",
        quote_path(marker),
        quote_path(release),
    );
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!(
        "powershell.exe -NoProfile -EncodedCommand {}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
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
                "cruise/child",
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
        let command = windows_marker_command(&marker, &release);
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
        child.worktree_branch = Some("cruise/child".into());
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
    async fn start(&self) -> Value {
        let deadline = Instant::now() + TEST_TIMEOUT;
        let out = cli(self.home.path(), &["daemon", "start", "--port", "0"]);
        let mut last_start_output = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let already_running = last_start_output.contains("daemon is already running at ");
        let mut retry_start = !out.status.success() && !already_running;
        if retry_start
            && !last_start_output.contains("timeout: global")
            && !last_start_output.contains("timed out starting daemon")
        {
            panic!("start: {last_start_output}");
        }

        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|error| panic!("test operation failed: {error}"));
        let instance_path = self.home.path().join("state/cruise/daemon/instance.json");
        let mut last_readiness_error = String::new();
        loop {
            if let Ok(bytes) = fs::read(&instance_path)
                && let Ok(instance) = serde_json::from_slice::<Value>(&bytes)
            {
                let origin = instance["origin"]
                    .as_str()
                    .unwrap_or_else(|| panic!("daemon instance has no origin: {instance}"));
                let token = instance["token"]
                    .as_str()
                    .unwrap_or_else(|| panic!("daemon instance has no token: {instance}"));
                let mut url = reqwest::Url::parse(&format!("{origin}api/manager/snapshot"))
                    .unwrap_or_else(|error| panic!("invalid daemon origin: {error}"));
                url.query_pairs_mut()
                    .append_pair("cwd", &self.repo.to_string_lossy());
                match client.get(url).bearer_auth(token).send().await {
                    Ok(response) if response.status().is_success() => return instance,
                    Ok(response) => {
                        let status = response.status();
                        let body = response.text().await.unwrap_or_default();
                        last_readiness_error = format!("HTTP {status}: {body}");
                        assert!(
                            !status.is_client_error()
                                && status != reqwest::StatusCode::SERVICE_UNAVAILABLE,
                            "Manager API failed readiness: {last_readiness_error}"
                        );
                    }
                    Err(error) => last_readiness_error = error.to_string(),
                }
            }

            if retry_start {
                let out = cli(self.home.path(), &["daemon", "start", "--port", "0"]);
                last_start_output = format!(
                    "{}{}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                );
                retry_start = !out.status.success()
                    && !last_start_output.contains("daemon is already running at ");
                if retry_start
                    && !last_start_output.contains("timeout: global")
                    && !last_start_output.contains("timed out starting daemon")
                {
                    panic!("start: {last_start_output}");
                }
            }
            assert!(
                Instant::now() < deadline,
                "daemon Manager API did not become ready; start={last_start_output}; readiness={last_readiness_error}"
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
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
async fn manager_snapshot(fixture: &Fixture, instance: &Value) -> Value {
    let mut url = reqwest::Url::parse(&format!(
        "{}api/manager/snapshot",
        instance["origin"]
            .as_str()
            .unwrap_or_else(|| panic!("expected daemon origin"))
    ))
    .unwrap_or_else(|error| panic!("invalid snapshot URL: {error}"));
    url.query_pairs_mut()
        .append_pair("cwd", &fixture.repo.to_string_lossy());
    let response = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"))
        .get(url)
        .bearer_auth(
            instance["token"]
                .as_str()
                .unwrap_or_else(|| panic!("expected daemon token")),
        )
        .send()
        .await
        .unwrap_or_else(|error| panic!("snapshot request failed: {error}"));
    assert!(
        response.status().is_success(),
        "snapshot returned {}",
        response.status()
    );
    response
        .json::<Value>()
        .await
        .unwrap_or_else(|error| panic!("parse snapshot: {error}"))
}
async fn wait_for_child_marker(fixture: &Fixture, instance: &Value) {
    let deadline = Instant::now() + TEST_TIMEOUT;
    while !fixture.marker.exists() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let snapshot = manager_snapshot(fixture, instance).await;
    let child = fixture
        .sessions
        .load("child")
        .unwrap_or_else(|error| panic!("load child after resume timeout: {error}"));
    let daemon_log = fs::read_to_string(fixture.home.path().join("state/cruise/daemon/daemon.log"))
        .unwrap_or_default();
    assert!(
        fixture.marker.exists(),
        "child marker timeout: goal={} child_phase={:?} child_runner={:?} snapshot={snapshot} daemon_log={daemon_log}",
        fixture.goal(),
        child.phase,
        child.live_foreign_runner_pid()
    );
}

#[tokio::test]
async fn manager_resume_task_continues_the_existing_child() {
    let fixture = Fixture::new("active", false, false);
    let instance = fixture.start().await;
    let previous = fixture
        .sessions
        .load("child")
        .unwrap_or_else(|err| panic!("load child session: {err}"));
    let output = cli_in(
        fixture.home.path(),
        &fixture.repo,
        &["manager", "resume", "g", "task"],
    );
    assert!(output.status.success(), "manager resume: {output:?}");
    wait_for_child_marker(&fixture, &instance).await;
    let snapshot = manager_snapshot(&fixture, &instance).await;
    let attempts = snapshot["goals"][0]["execution"]["attempts"]
        .as_array()
        .unwrap_or_else(|| panic!("read execution attempts"));
    assert_eq!(attempts.len(), 1, "{snapshot}");
    assert_eq!(attempts[0]["session_id"], "child");
    assert_eq!(
        fixture
            .sessions
            .load("child")
            .unwrap_or_else(|err| panic!("load child session: {err}"))
            .worktree_path,
        previous.worktree_path
    );
}

#[tokio::test]
async fn manager_retry_task_preserves_the_previous_child_and_reserves_a_fresh_attempt() {
    let fixture = Fixture::new("active", false, false);
    let instance = fixture.start().await;
    let old_worktree = fixture
        .sessions
        .load("child")
        .unwrap_or_else(|err| panic!("load child session: {err}"))
        .worktree_path
        .unwrap_or_else(|| panic!("find original worktree"));
    let marker = old_worktree.join("previous-attempt-evidence");
    fs::write(&marker, "keep this evidence")
        .unwrap_or_else(|err| panic!("write previous attempt marker: {err}"));
    let output = cli_in(
        fixture.home.path(),
        &fixture.repo,
        &["manager", "retry", "g", "task"],
    );
    assert!(output.status.success(), "manager retry: {output:?}");
    let snapshot = manager_snapshot(&fixture, &instance).await;
    let attempts = snapshot["goals"][0]["execution"]["attempts"]
        .as_array()
        .unwrap_or_else(|| panic!("read execution attempts"));
    assert_eq!(attempts.len(), 2, "{snapshot}");
    assert_eq!(attempts[0]["session_id"], "child");
    assert_eq!(attempts[1]["task_id"], "task");
    assert_eq!(attempts[1]["attempt"], 2);
    assert_ne!(
        attempts[1]["session_id"]
            .as_str()
            .unwrap_or_else(|| panic!("find retry session ID")),
        "child"
    );
    assert_eq!(
        fs::read_to_string(marker)
            .unwrap_or_else(|err| panic!("read previous attempt marker: {err}")),
        "keep this evidence"
    );
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::write(self.home.path().join("release"), "release");
        let _ = cli(self.home.path(), &["daemon", "stop", "--force"]);
    }
}
async fn eventually(mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + TEST_TIMEOUT;
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
    fixture.start().await;
    let goal = fixture.goal();
    assert_eq!(goal["status"], "paused");
    assert_eq!(goal["execution"]["attempts"][0]["integrated"], true);
    assert_eq!(goal["execution"]["integrated_tip"], prepared);
    fixture.stop();
    fixture.start().await;
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
        fixture.start().await;
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
    fixture.start().await;
    eventually(|| {
        let goal = fixture.goal();
        assert_ne!(
            goal["status"],
            "blocked",
            "pending integration blocked: {goal}; daemon_log={}",
            fs::read_to_string(fixture.home.path().join("state/cruise/daemon/daemon.log"))
                .unwrap_or_default()
        );
        goal["execution"]["attempts"][0]["integrated"] == true
    })
    .await;
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
    fixture.start().await;
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
    let drift = git(&fixture.repo, &["rev-parse", "cruise/child"]);
    git(
        &fixture.repo,
        &["update-ref", "refs/heads/cruise/goal/g", &drift],
    );
    fixture.start().await;
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
    fixture.start().await;
    let goal = fixture.goal();
    assert_eq!(goal["execution"]["attempts"][0]["integrated"], true);
    assert_eq!(goal["execution"]["integrated_tip"], prepared);
}

#[tokio::test]
async fn paused_pending_resume_survives_restart_then_runs_once_after_goal_resume() {
    let fixture = Fixture::new("paused", true, false);
    let first = fixture.start().await;
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
    let second = fixture.start().await;
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
            started.elapsed() <= TEST_TIMEOUT,
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
    let instance = fixture.start().await;
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
    wait_for_child_marker(&fixture, &instance).await;
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
async fn failed_child_is_visible_and_independent_work_continues() {
    let fixture = Fixture::new("active", false, false);
    let mut record = serde_json::from_slice::<Value>(
        &fs::read(&fixture.record).unwrap_or_else(|error| panic!("read Goal: {error}")),
    )
    .unwrap_or_else(|error| panic!("parse Goal: {error}"));
    record["goals"][0]["proposal"]["tasks"] = json!([
        {"id":"task", "instructions":"work", "dependencies":[]},
        {"id":"independent", "instructions":"independent work", "dependencies":[]},
        {"id":"dependent", "instructions":"dependent work", "dependencies":["task"]}
    ]);
    fs::write(
        &fixture.record,
        serde_json::to_vec(&record).unwrap_or_else(|error| panic!("serialize Goal: {error}")),
    )
    .unwrap_or_else(|error| panic!("write Goal: {error}"));
    let mut child = fixture
        .sessions
        .load("child")
        .unwrap_or_else(|error| panic!("load child: {error}"));
    child.phase = SessionPhase::Failed("workflow command failed".into());
    fixture
        .sessions
        .save(&child)
        .unwrap_or_else(|error| panic!("save failed child: {error}"));

    let instance = fixture.start().await;
    eventually(|| {
        fixture.goal()["execution"]["attempts"]
            .as_array()
            .is_some_and(|attempts| {
                attempts
                    .iter()
                    .any(|attempt| attempt["task_id"] == "independent")
            })
    })
    .await;
    let goal = fixture.goal();
    assert_eq!(goal["status"], "active");
    assert!(
        goal["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("`cruise manager resume g task`")),
        "{goal}"
    );
    let attempts = goal["execution"]["attempts"]
        .as_array()
        .unwrap_or_else(|| panic!("expected attempts"));
    assert!(
        attempts
            .iter()
            .any(|attempt| attempt["task_id"] == "independent"),
        "{goal}"
    );
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt["task_id"] != "dependent"),
        "{goal}"
    );
    wait_for_child_marker(&fixture, &instance).await;
    assert!(
        child.worktree_path.as_deref().is_some_and(Path::is_dir),
        "failed child worktree was not preserved"
    );

    let snapshot = manager_snapshot(&fixture, &instance).await;
    let failed_attempt = snapshot["goals"][0]["execution"]["attempts"]
        .as_array()
        .and_then(|attempts| attempts.iter().find(|attempt| attempt["task_id"] == "task"))
        .unwrap_or_else(|| panic!("failed attempt missing from snapshot: {snapshot}"));
    assert_eq!(failed_attempt["child_state"]["phase"], "Failed");
    assert_eq!(
        failed_attempt["child_state"]["failure"],
        "workflow command failed"
    );
}

#[tokio::test]
async fn interrupted_resume_dispatch_blocks_without_reexecuting_child() {
    let fixture = Fixture::new("active", false, true);
    let _instance = fixture.start().await;
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
    let instance = fixture.start().await;
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
    let _restarted = fixture.start().await;
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

const STALE_RESERVATION: &str = "Manager dispatch requires attention: interrupted Manager reservation no longer matches Goal tip";

fn edit_goal(fixture: &Fixture, edit: impl FnOnce(&mut Value)) {
    let mut record: Value = serde_json::from_slice(
        &fs::read(&fixture.record).unwrap_or_else(|error| panic!("read Goal: {error}")),
    )
    .unwrap_or_else(|error| panic!("parse Goal: {error}"));
    edit(&mut record["goals"][0]);
    // Renamed into place: a running daemon must never read a partial record.
    let staged = fixture.record.with_extension("staged");
    fs::write(
        &staged,
        serde_json::to_vec_pretty(&record).unwrap_or_else(|error| panic!("encode Goal: {error}")),
    )
    .unwrap_or_else(|error| panic!("write Goal: {error}"));
    fs::rename(&staged, &fixture.record).unwrap_or_else(|error| panic!("publish Goal: {error}"));
}

/// Move the Goal ref past the fixture base, as integrating another child does.
fn advance_goal_tip(fixture: &Fixture) -> String {
    fs::write(fixture.repo.join("file"), "goal change\n")
        .unwrap_or_else(|error| panic!("change Goal file: {error}"));
    git(&fixture.repo, &["commit", "-qam", "goal change"]);
    let tip = git(&fixture.repo, &["rev-parse", "HEAD"]);
    git(
        &fixture.repo,
        &["update-ref", "refs/heads/cruise/goal/g", &tip],
    );
    tip
}

/// Reserve attempt 2 at the fixture base, then leave the Goal tip ahead of that pin.
fn seed_stale_reservation(fixture: &Fixture) -> String {
    let base = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let tip = advance_goal_tip(fixture);
    edit_goal(fixture, |goal| {
        goal["execution"]["integrated_tip"] = json!(tip);
        goal["execution"]["attempts"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("expected attempts"))
            .push(json!({"task_id":"task","attempt":2,"session_id":"retry","base_sha":base,
                "launch_started":false,"result_sha":null,"integrated":false,"integration_intent":null}));
    });
    tip
}

fn retry_task(fixture: &Fixture) -> Output {
    cli_in(
        fixture.home.path(),
        &fixture.repo,
        &["manager", "retry", "g", "task"],
    )
}

async fn wait_for_single_launch(fixture: &Fixture) {
    eventually(|| {
        let goal = fixture.goal();
        assert_ne!(goal["status"], "blocked", "{goal}");
        fixture.marker.exists()
    })
    .await;
    assert_eq!(
        fs::read_to_string(&fixture.marker)
            .unwrap_or_else(|error| panic!("read launch marker: {error}"))
            .lines()
            .collect::<Vec<_>>(),
        vec!["started"]
    );
}

#[tokio::test]
async fn retry_reservation_follows_the_tip_when_a_sibling_integrates_first() {
    let fixture = Fixture::new("blocked", false, false);
    let base = git(&fixture.repo, &["rev-parse", "HEAD"]);
    // Attempt 1 completed, but its result conflicts with the Goal tip.
    let mut child = fixture
        .sessions
        .load("child")
        .unwrap_or_else(|error| panic!("load child: {error}"));
    let worktree = child
        .worktree_path
        .clone()
        .unwrap_or_else(|| panic!("expected worktree"));
    fs::write(worktree.join("file"), "child result\n")
        .unwrap_or_else(|error| panic!("write child result: {error}"));
    git(&worktree, &["commit", "-qam", "child result"]);
    let result = git(&worktree, &["rev-parse", "HEAD"]);
    child.phase = SessionPhase::Completed;
    fixture
        .sessions
        .save(&child)
        .unwrap_or_else(|error| panic!("save child: {error}"));
    let conflicting_tip = advance_goal_tip(&fixture);
    // A sibling completed while the Goal was blocked, so it is not integrated yet.
    // Its session reuses the fixture child's record with its own owner and worktree.
    let sibling = fixture.sessions.worktrees_dir().join("other");
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "-b",
            "cruise/other",
            sibling
                .to_str()
                .unwrap_or_else(|| panic!("expected UTF-8 path")),
            &base,
        ],
    );
    fs::write(sibling.join("other"), "other result\n")
        .unwrap_or_else(|error| panic!("write sibling result: {error}"));
    let mut other = child;
    other.id = "other".into();
    other.managed_child = Some(ManagedChildOwner {
        goal_id: "g".into(),
        task_id: "other".into(),
        attempt: 1,
        revision: 1,
        base_sha: base.clone(),
    });
    other.worktree_path = Some(sibling);
    other.worktree_branch = Some("cruise/other".into());
    fixture
        .sessions
        .create(&other)
        .unwrap_or_else(|error| panic!("create sibling: {error}"));
    edit_goal(&fixture, |goal| {
        goal["reason"] = json!("Child child integration requires attention: fixture conflict");
        goal["proposal"]["tasks"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("expected tasks"))
            .push(json!({"id":"other","instructions":"other work","dependencies":[]}));
        goal["execution"]["integrated_tip"] = json!(conflicting_tip);
        goal["execution"]["attempts"][0]["result_sha"] = json!(result);
        goal["execution"]["attempts"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("expected attempts"))
            .push(json!({"task_id":"other","attempt":1,"session_id":"other","base_sha":base,
                "launch_started":true,"result_sha":null,"integrated":false,"integration_intent":null}));
    });
    fixture.start().await;

    let output = retry_task(&fixture);
    assert!(output.status.success(), "manager retry: {output:?}");
    wait_for_single_launch(&fixture).await;

    let goal = fixture.goal();
    let attempts = goal["execution"]["attempts"]
        .as_array()
        .unwrap_or_else(|| panic!("expected attempts"));
    assert_eq!(attempts.len(), 3, "{goal}");
    assert_eq!(attempts[1]["integrated"], true, "{goal}");
    let tip = goal["execution"]["integrated_tip"]
        .as_str()
        .unwrap_or_else(|| panic!("expected Goal tip"));
    assert_ne!(tip, conflicting_tip, "{goal}");
    assert_eq!(attempts[2]["base_sha"], tip, "{goal}");
    let retried = fixture.sessions.worktrees_dir().join(
        attempts[2]["session_id"]
            .as_str()
            .unwrap_or_else(|| panic!("expected retry session")),
    );
    assert_eq!(git(&retried, &["rev-parse", "HEAD"]), tip);
}

#[tokio::test]
async fn retry_restarts_a_goal_blocked_by_a_stale_unpublished_reservation() {
    let fixture = Fixture::new("blocked", false, false);
    let tip = seed_stale_reservation(&fixture);
    edit_goal(&fixture, |goal| goal["reason"] = json!(STALE_RESERVATION));
    let mut expected: Goal = serde_json::from_value(fixture.goal())
        .unwrap_or_else(|error| panic!("parse Goal: {error}"));
    fixture.start().await;

    let output = retry_task(&fixture);
    assert!(output.status.success(), "manager retry: {output:?}");
    // Pending or already running: a repeated retry must not add another launch.
    assert!(!retry_task(&fixture).status.success());
    wait_for_single_launch(&fixture).await;

    expected.status = GoalStatus::Active;
    expected.reason = None;
    expected.execution.attempts[1].base_sha.clone_from(&tip);
    expected.execution.attempts[1].launch_started = Some(true);
    let actual: Goal = serde_json::from_value(fixture.goal())
        .unwrap_or_else(|error| panic!("parse Goal: {error}"));
    assert_eq!(json!(actual), json!(expected));
    let owner = fixture
        .sessions
        .load("retry")
        .unwrap_or_else(|error| panic!("load retried child: {error}"))
        .managed_child
        .unwrap_or_else(|| panic!("expected Manager owner"));
    assert_eq!(owner.base_sha, tip);
    assert_eq!(
        git(
            &fixture.sessions.worktrees_dir().join("retry"),
            &["rev-parse", "HEAD"]
        ),
        tip
    );
}

#[tokio::test]
async fn stale_reservation_that_may_have_started_stays_blocked() {
    let fixture = Fixture::new("active", false, false);
    let base = git(&fixture.repo, &["rev-parse", "HEAD"]);
    seed_stale_reservation(&fixture);
    // A session directory published under the old pin may belong to a started child.
    let session = fixture.sessions.sessions_dir().join("retry");
    fs::create_dir(&session).unwrap_or_else(|error| panic!("publish reservation: {error}"));
    fs::write(
        session.join("manager-owner.json"),
        json!({"goal_id":"g","task_id":"task","attempt":2,"revision":1,"base_sha":base})
            .to_string(),
    )
    .unwrap_or_else(|error| panic!("write owner marker: {error}"));
    fixture.start().await;
    eventually(|| fixture.goal()["status"] == "blocked").await;
    assert_eq!(fixture.goal()["reason"], STALE_RESERVATION);

    for launch_started in [json!(false), json!(true), Value::Null] {
        if launch_started != json!(false) {
            // Without the directory, only the recorded launch state protects the attempt.
            let _ = fs::remove_dir_all(&session);
        }
        edit_goal(&fixture, |goal| {
            goal["execution"]["attempts"][1]["launch_started"] = launch_started.clone();
        });
        let before = fixture.goal();
        assert_eq!(before["execution"]["attempts"][1]["base_sha"], base);
        assert!(
            !retry_task(&fixture).status.success(),
            "launch_started={launch_started}"
        );
        assert_eq!(fixture.goal(), before);
    }
    assert!(!fixture.marker.exists());
}

/// The fixture record and plan are what versions before corrections wrote. Reviewing a
/// correction adds no field to that record, and approval appends to that plan.
#[tokio::test]
async fn legacy_record_keeps_its_shape_and_its_failed_attempt_takes_a_correction() {
    let fixture = Fixture::new("active", false, false);
    let mut child = fixture
        .sessions
        .load("child")
        .unwrap_or_else(|error| panic!("load child: {error}"));
    child.phase = SessionPhase::Failed("fixture failure".into());
    child.current_step = Some("run".into());
    fixture
        .sessions
        .save(&child)
        .unwrap_or_else(|error| panic!("save failed child: {error}"));
    let plan = child.plan_path(&fixture.sessions.sessions_dir());
    let legacy_plan =
        fs::read_to_string(&plan).unwrap_or_else(|error| panic!("read legacy plan: {error}"));
    let instance = fixture.start().await;
    let manager = |args: &[&str]| {
        let out = cli_in(
            fixture.home.path(),
            &fixture.repo,
            &[&["manager"], args].concat(),
        );
        assert!(out.status.success(), "manager {args:?}: {out:?}");
    };

    manager(&[
        "corrective",
        "g",
        "task",
        "--resume",
        "--instructions",
        "Finish the work",
    ]);
    let reviewed = fixture.goal();
    let execution = &reviewed["execution"];
    let preview = &execution["corrective_preview"];
    assert_eq!(preview["resume"]["step"], "run", "{preview}");
    assert!(preview.get("contract_revision").is_none(), "{preview}");
    assert!(execution.get("evidence_runs").is_none(), "{execution}");
    for added in ["contract_revision", "corrections", "correction_pending"] {
        assert!(execution["attempts"][0].get(added).is_none(), "{execution}");
    }
    assert!(!fixture.marker.exists());

    manager(&["approve-corrective", "g", "--revision", "1", "--yes"]);
    wait_for_child_marker(&fixture, &instance).await;
    assert_eq!(
        fs::read_to_string(&plan).unwrap_or_else(|error| panic!("read plan: {error}")),
        format!("{legacy_plan}\n# Approved correction 1\n\nFinish the work\n")
    );
    assert_eq!(
        fixture.goal()["execution"]["attempts"][0]["corrections"],
        json!([1])
    );
}

/// An amendment approved between `retry` saving a reservation and its dispatch reaches
/// that child: a reservation with no published session is planned at launch.
#[tokio::test]
async fn unlaunched_reservation_is_planned_with_an_amendment_approved_before_dispatch() {
    let fixture = Fixture::new("active", false, false);
    let base = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let reviewed = Sha256::digest(b"works")
        .iter()
        .fold(String::new(), |mut hex, byte| {
            use std::fmt::Write;
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    edit_goal(&fixture, |goal| {
        goal["execution"]["attempts"]
            .as_array_mut()
            .unwrap_or_else(|| panic!("expected attempts"))
            .push(json!({"task_id":"task","attempt":2,"session_id":"retry","base_sha":base,
                "launch_started":false,"result_sha":null,"integrated":false,"integration_intent":null}));
        goal["execution"]["corrective_approvals"] = json!([{"id":"amendment","revision":1,
            "goal_id":"g","integrated_tip":base,"amendment":[{"criterion":"c",
            "old_sha256":reviewed,"replace":"works","with":"works on every platform"}]}]);
    });
    fixture.start().await;
    wait_for_single_launch(&fixture).await;
    let goal = fixture.goal();
    assert_eq!(goal["execution"]["attempts"][1]["contract_revision"], 1);
    assert!(
        goal["execution"]["attempts"][0]
            .get("contract_revision")
            .is_none()
    );
    let plan = fs::read_to_string(fixture.sessions.sessions_dir().join("retry/plan.md"))
        .unwrap_or_else(|error| panic!("read reserved child plan: {error}"));
    assert!(plan.contains("\n- c: works on every platform\n"), "{plan}");
}
