#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

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
        .unwrap()
}

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
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
    home: TempDir,
    record: PathBuf,
    session: PathBuf,
    sha: String,
}
impl Fixture {
    fn new() -> Self {
        let home = TempDir::new().unwrap();
        let root = home.path();
        let repo = root.join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "user.email", "test@example.test"]);
        fs::write(repo.join("file"), "base\n").unwrap();
        git(&repo, &["add", "file"]);
        git(&repo, &["commit", "-qm", "base"]);
        let sha = git(&repo, &["rev-parse", "HEAD"]);
        git(&repo, &["update-ref", "refs/heads/cruise/goal/g", &sha]);
        let origin = "https://github.com/example/project.git";
        git(&repo, &["remote", "add", "origin", origin]);
        let anchor = repo.canonicalize().unwrap();
        let common = repo.join(".git").canonicalize().unwrap();
        let workflow: cruise::config::WorkflowConfig = serde_json::from_value(json!({
            "create_pr":false, "cleanup_after_pr":false, "force_exec":false,
            "steps":{"run":{"command":"sleep 30"}}
        }))
        .unwrap();
        let workflow = serde_json::to_value(workflow).unwrap();
        let proposal = json!({"id":"p","revision":1,"objective":"goal","target_branch":"main","target_sha":sha,
            "exclusions":[],"criteria":[{"id":"c","description":"works"}],
            "checks":[{"id":"check","criterion_ids":["c"],"argv":["true"]}],
            "tasks":[{"id":"task","instructions":"work","dependencies":[]}],"workflow":"builtin:tdd",
            "contract":{"anchor":anchor,"origin":origin,"forge_kind":"GitHub","forge_host":"github.com","forge_repository":"example/project","workflow":workflow}});
        let attempt = json!({"task_id":"task","attempt":1,"session_id":"child","base_sha":sha,
            "launch_started":false,"result_sha":null,"integrated":false,"integration_intent":null});
        let goal = json!({"id":"g","proposal":proposal,"status":"active","reason":null,
            "execution":{"anchor":anchor,"origin":origin,"workflow":workflow,"integrated_tip":sha,"attempts":[attempt],
            "paused_from":null,"paused_reason":null,"publication":null,"publication_history":[]}});
        let key: String = Sha256::digest(common.to_string_lossy().as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let record = root.join("data/cruise/manager").join(format!("{key}.json"));
        fs::create_dir_all(record.parent().unwrap()).unwrap();
        fs::write(
            &record,
            serde_json::to_vec(
                &json!({"repository":common,"transcript":[],"proposal":null,"goals":[goal]}),
            )
            .unwrap(),
        )
        .unwrap();
        let session = root.join("data/cruise/sessions/child");
        Self {
            home,
            record,
            session,
            sha,
        }
    }
    fn owner(&self) -> Value {
        json!({"goal_id":"g","task_id":"task","attempt":1,"revision":1,"base_sha":self.sha})
    }
    fn start(&self) {
        let out = cli(self.home.path(), &["daemon", "start", "--port", "0"]);
        assert!(
            out.status.success(),
            "start: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn goal(&self) -> Value {
        serde_json::from_slice::<Value>(&fs::read(&self.record).unwrap()).unwrap()["goals"][0]
            .clone()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = cli(self.home.path(), &["daemon", "stop", "--force"]);
    }
}
async fn eventually(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out waiting for daemon");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn persisted_reservation_recovers_from_prepublication_staging() {
    let f = Fixture::new();
    let abandoned = f
        .home
        .path()
        .join("data/cruise/manager-reservation-staging/abandoned");
    fs::create_dir_all(&abandoned).unwrap();
    fs::write(
        abandoned.join("manager-owner.json"),
        serde_json::to_vec(&f.owner()).unwrap(),
    )
    .unwrap();
    f.start();
    eventually(|| f.session.join("state.json").exists()).await;
    let actual: Value =
        serde_json::from_slice(&fs::read(f.session.join("manager-owner.json")).unwrap()).unwrap();
    assert_eq!(actual, f.owner());
    assert_eq!(
        f.goal()["execution"]["attempts"].as_array().unwrap().len(),
        1
    );
    assert!(abandoned.exists());
}

#[tokio::test]
async fn existing_matching_owner_marker_is_recovered_without_new_attempt() {
    let f = Fixture::new();
    fs::create_dir_all(&f.session).unwrap();
    fs::write(
        f.session.join("manager-owner.json"),
        serde_json::to_vec(&f.owner()).unwrap(),
    )
    .unwrap();
    f.start();
    eventually(|| f.session.join("state.json").exists()).await;
    let actual: Value =
        serde_json::from_slice(&fs::read(f.session.join("manager-owner.json")).unwrap()).unwrap();
    assert_eq!(actual, f.owner());
    assert_eq!(
        f.goal()["execution"]["attempts"].as_array().unwrap().len(),
        1
    );
}

#[tokio::test]
async fn foreign_markerless_directory_is_not_adopted_or_modified() {
    let f = Fixture::new();
    fs::create_dir_all(&f.session).unwrap();
    fs::write(f.session.join("foreign"), b"do not touch").unwrap();
    f.start();
    eventually(|| f.goal()["status"] == "blocked").await;
    assert_eq!(
        fs::read(f.session.join("foreign")).unwrap(),
        b"do not touch"
    );
    assert!(!f.session.join("manager-owner.json").exists());
    assert!(!f.session.join("state.json").exists());
    assert_eq!(fs::read_dir(&f.session).unwrap().count(), 1);
}
