use reqwest::StatusCode;
use serde_json::Value;
use std::fs;
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;

struct StopDaemon<'a>(&'a std::path::Path);

impl Drop for StopDaemon<'_> {
    fn drop(&mut self) {
        let _ = Command::new(env!("CARGO_BIN_EXE_cruise"))
            .current_dir(self.0)
            .env("HOME", self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_DATA_HOME", self.0.join("data"))
            .env("XDG_STATE_HOME", self.0.join("state"))
            .env_remove("CRUISE_DAEMON_PORT")
            .args(["daemon", "stop", "--force"])
            .output();
    }
}

#[tokio::test]
async fn manager_routes_require_bearer_authentication() {
    let home = TempDir::new().unwrap();
    let command = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_cruise"))
            .current_dir(home.path())
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("XDG_STATE_HOME", home.path().join("state"))
            .env_remove("CRUISE_CONFIG")
            .env_remove("CRUISE_DAEMON_PORT")
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap()
    };
    let started = command(&["daemon", "start", "--port", "0"]);
    assert!(
        started.status.success(),
        "daemon start: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    let _stop = StopDaemon(home.path());
    let record: Value = serde_json::from_slice(
        &fs::read(home.path().join("state/cruise/daemon/instance.json")).unwrap(),
    )
    .unwrap();
    let origin = record["origin"].as_str().unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let mut statuses = Vec::new();
    for path in ["api/manager/snapshot", "api/manager/action"] {
        for token in [None, Some("invalid-token")] {
            let mut request = if path.ends_with("snapshot") {
                client.get(format!("{origin}{path}"))
            } else {
                client.post(format!("{origin}{path}"))
            };
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            statuses.push(request.send().await.unwrap().status());
        }
    }
    assert_eq!(statuses, vec![StatusCode::UNAUTHORIZED; 4]);
}
