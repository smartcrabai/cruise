use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use serde_json::{Value, json};
use tempfile::TempDir;

struct Daemon {
    home: TempDir,
}

impl Daemon {
    fn new() -> Self {
        Self {
            home: TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}")),
        }
    }

    fn command(&self, args: &[&str]) -> Output {
        let home = self.home.path();
        Command::new(env!("CARGO_BIN_EXE_cruise"))
            .current_dir(home)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("CRUISE_CONFIG")
            .env_remove("CRUISE_DAEMON_PORT")
            .env_remove("HERDR_ENV")
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("run {args:?}: {error}"))
    }

    fn start(&self) -> Value {
        let output = self.command(&["daemon", "start", "--port", "0"]);
        assert!(
            output.status.success(),
            "daemon start failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(
            &fs::read(self.record_path())
                .unwrap_or_else(|error| panic!("read instance record: {error}")),
        )
        .unwrap_or_else(|error| panic!("parse instance record: {error}"))
    }

    fn record_path(&self) -> PathBuf {
        self.home.path().join("state/cruise/daemon/instance.json")
    }

    fn success(&self, args: &[&str]) -> Output {
        let output = self.command(args);
        assert!(
            output.status.success(),
            "{args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.record_path().exists() {
            let _ = self.command(&["daemon", "stop", "--force"]);
        }
    }
}

fn write_cli_session(home: &Path) {
    let project = home.join("project");
    fs::create_dir_all(&project).unwrap_or_else(|error| panic!("create project: {error}"));
    let config = project.join("cruise.yml");
    fs::write(
        &config,
        "command: [echo]\nsteps:\n  build:\n    prompt: make a change\n",
    )
    .unwrap_or_else(|error| panic!("write config: {error}"));
    let manager = SessionManager::new(home.join("data/cruise"));
    let mut state = SessionState::new(
        "list-json-session".to_owned(),
        project,
        SessionConfigRef::File { path: config },
        "compare API and CLI list JSON".to_owned(),
    );
    state.phase = SessionPhase::Planned;
    state.title = Some("List JSON shape".to_owned());
    state.current_step = Some("build".to_owned());
    state.auto_requeue_count = 2;
    manager
        .create(&state)
        .unwrap_or_else(|error| panic!("create list session: {error}"));
    fs::write(
        state.plan_path(&manager.sessions_dir()),
        "# Plan\n\nVerify visible JSON fields.\n",
    )
    .unwrap_or_else(|error| panic!("write plan: {error}"));
    state.plan_error = None;
    manager
        .save(&state)
        .unwrap_or_else(|error| panic!("save list session: {error}"));
}

#[tokio::test]
async fn sessions_api_json_matches_cruise_list_json() {
    let daemon = Daemon::new();
    let instance = daemon.start();
    write_cli_session(daemon.home.path());
    let origin = instance["origin"]
        .as_str()
        .unwrap_or_else(|| panic!("instance origin missing"));
    let token = instance["token"]
        .as_str()
        .unwrap_or_else(|| panic!("instance token missing"));
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_else(|error| panic!("HTTP client: {error}"))
        .get(format!("{origin}api/v1/sessions"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap_or_else(|error| panic!("GET sessions: {error}"));
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let api_json = response
        .json::<Value>()
        .await
        .unwrap_or_else(|error| panic!("parse sessions API: {error}"));

    let cli = daemon.success(&["list", "--json"]);
    let cli_json: Value = serde_json::from_slice(&cli.stdout)
        .unwrap_or_else(|error| panic!("parse cruise list --json: {error}"));
    assert_eq!(api_json, cli_json);
    let item = &cli_json[0];
    assert_eq!(item["id"], "list-json-session");
    assert_eq!(item["phase"], "Planned");
    assert_eq!(
        item["base_dir"],
        daemon
            .home
            .path()
            .join("project")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        item["config_source"],
        daemon
            .home
            .path()
            .join("project")
            .join("cruise.yml")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(item["title"], "List JSON shape");
    assert_eq!(item["current_step"], "build");
    assert!(item["plan_available"].as_bool().unwrap_or(false));
    assert_eq!(item["pending_instructions"], 0);
    assert_eq!(item["auto_requeue_count"], 2);
    assert!(item.get("baseDir").is_none());
    assert!(item.get("configSource").is_none());
    assert_eq!(item["workspace_mode"], json!("Worktree"));
}
