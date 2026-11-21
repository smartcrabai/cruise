#![expect(clippy::unwrap_used, reason = "isolated manager contract fixtures")]

use std::fs;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tempfile::TempDir;

struct Daemon(TempDir);

impl Daemon {
    fn command(&self, args: &[&str]) -> Output {
        let home = self.0.path();
        Command::new(env!("CARGO_BIN_EXE_cruise"))
            .current_dir(home)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("CRUISE_CONFIG")
            .env_remove("CRUISE_DAEMON_PORT")
            .env_remove("HERDR_ENV")
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self
            .0
            .path()
            .join("state/cruise/daemon/instance.json")
            .exists()
        {
            let _ = self.command(&["daemon", "stop", "--force"]);
        }
    }
}

#[tokio::test]
async fn corrupt_manager_record_does_not_disable_ordinary_daemon_service() {
    let daemon = Daemon(TempDir::new().unwrap());
    let home = daemon.0.path();
    let project = home.join("project");
    fs::create_dir_all(&project).unwrap();
    let config = project.join("cruise.yml");
    fs::write(
        &config,
        "command: [echo]\nsteps:\n  build:\n    prompt: make a change\n",
    )
    .unwrap();
    let sessions = SessionManager::new(home.join("data/cruise"));
    let mut session = SessionState::new(
        "ordinary-session".to_owned(),
        project.clone(),
        SessionConfigRef::File { path: config },
        "ordinary service".to_owned(),
    );
    session.phase = SessionPhase::Planned;
    sessions.create(&session).unwrap();

    let record = home.join("data/cruise/manager/corrupt.json");
    fs::create_dir_all(record.parent().unwrap()).unwrap();
    let corrupt = b"{not valid json";
    fs::write(&record, corrupt).unwrap();
    let started = daemon.command(&["daemon", "start", "--port", "0"]);
    assert!(
        started.status.success(),
        "daemon start: {}{}",
        String::from_utf8_lossy(&started.stdout),
        String::from_utf8_lossy(&started.stderr)
    );
    let instance: Value =
        serde_json::from_slice(&fs::read(home.join("state/cruise/daemon/instance.json")).unwrap())
            .unwrap();
    let origin = instance["origin"].as_str().unwrap();
    let token = instance["token"].as_str().unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let response = client
        .get(format!("{origin}api/v1/sessions"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let listed: Value = response.json().await.unwrap();
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "ordinary-session" && s["phase"] == "Planned"),
        "sessions: {listed}"
    );
    let cli = daemon.command(&["list", "--json"]);
    assert!(
        cli.status.success(),
        "list: {}",
        String::from_utf8_lossy(&cli.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&cli.stdout).unwrap(),
        listed
    );

    for action in [
        json!({"type":"message", "text":"hello"}),
        json!({"type":"propose"}),
        json!({"type":"approve", "proposal_id":"p", "revision":1}),
        json!({"type":"pause", "goal_id":"g"}),
        json!({"type":"resume", "goal_id":"g"}),
        json!({"type":"resume_task", "goal_id":"g", "task_id":"t"}),
        json!({"type":"retry", "goal_id":"g", "task_id":"t"}),
        json!({"type":"answer", "goal_id":"g", "prompt_id":"p", "answer":"yes"}),
    ] {
        let response = client
            .post(format!("{origin}api/manager/action"))
            .bearer_auth(token)
            .json(&json!({"cwd":project, "action":action}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::SERVICE_UNAVAILABLE,
            "action: {action}"
        );
    }
    assert_eq!(fs::read(record).unwrap(), corrupt);
}
