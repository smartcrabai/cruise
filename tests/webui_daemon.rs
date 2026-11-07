//! End-to-end lifecycle checks against the real CLI, with an isolated state directory.
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use serde_json::Value;
use tempfile::TempDir;

struct Harness {
    home: TempDir,
}

impl Harness {
    fn new() -> Self {
        Self {
            home: TempDir::new().unwrap(),
        }
    }

    fn command(&self) -> Command {
        let home = self.home.path();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        cmd.current_dir(home)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("HERDR_ENV")
            .env_remove("CRUISE_CONFIG")
            .env_remove("CRUISE_MODEL")
            .env_remove("CRUISE_PLAN_MODEL")
            .env_remove("CRUISE_SDK")
            .env_remove("CRUISE_LANGUAGE_PR")
            .env_remove("CRUISE_LANGUAGE_PLAN")
            .env_remove("CRUISE_CLEANUP_AFTER_PR")
            .env_remove("CRUISE_INTERACTIVE_PLANNING")
            .env_remove("CRUISE_FORCE_EXEC")
            .stdin(Stdio::null());
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn success(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn record_path(&self) -> PathBuf {
        self.home.path().join("state/cruise/webui/instance.json")
    }

    fn record(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.record_path()).unwrap()).unwrap()
    }

    fn start(&self) -> Value {
        self.success(&["webui", "start", "--port", "0"]);
        self.record()
    }

    fn status(&self) -> Value {
        let output = self.success(&["webui", "status", "--json"]);
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // Stop even when an assertion panics, so a detached daemon does not leak.
        let _ = self.run(&["webui", "stop", "--force"]);
    }
}

fn response(port: u16) -> String {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn start_status_stop_round_trip() {
    let h = Harness::new();
    let record = h.start();
    let port = record["port"].as_u64().unwrap() as u16;
    let response = response(port);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response
            .to_ascii_lowercase()
            .contains("cache-control: no-store"),
        "{response}"
    );
    let status = h.status();
    assert_eq!(status["running"], true);
    assert_eq!(status["mode"], "daemon");
    assert_eq!(status["pid"], record["pid"]);
    assert!(h.record_path().exists());
    h.success(&["webui", "stop"]);
    assert!(!h.record_path().exists());
    assert_eq!(h.run(&["webui", "status"]).status.code(), Some(1));
    assert!(
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(1)).is_err()
    );
}

#[cfg(windows)]
#[test]
fn windows_detached_daemon_survives_start_command_exit() {
    let h = Harness::new();
    let started = h.start();
    let status = h.status();
    assert_eq!(status["running"], true);
    assert_eq!(status["pid"], started["pid"]);
    let port = started["port"].as_u64().unwrap() as u16;
    assert!(response(port).starts_with("HTTP/1.1 200"));
    h.success(&["webui", "stop"]);
}

#[test]
fn repeated_start_and_foreground_exclusivity() {
    let h = Harness::new();
    let original = h.start();
    let again = h.success(&["webui", "start"]);
    assert!(String::from_utf8_lossy(&again.stdout).contains(&original["pid"].to_string()));
    assert_eq!(h.record()["pid"], original["pid"]);
    let foreground = h.run(&["webui", "--port", "0"]);
    assert!(!foreground.status.success());
    assert!(
        String::from_utf8_lossy(&foreground.stderr)
            .to_ascii_lowercase()
            .contains("already running")
    );
    let conflict = h.run(&["webui", "start", "--port", "1"]);
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("restart"));
    assert_eq!(h.record()["pid"], original["pid"]);
}

#[test]
fn invalid_live_record_blocks_new_servers() {
    let h = Harness::new();
    let original = h.start();
    let original_bytes = std::fs::read(h.record_path()).unwrap();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut unsupported = original.clone();
    unsupported["schema_version"] = 999.into();
    for invalid in [b"{broken".to_vec(), unsupported.to_string().into_bytes()] {
        std::fs::write(h.record_path(), &invalid).unwrap();
        let daemon = h.run(&["webui", "start", "--port", &port.to_string()]);
        let after_daemon = std::fs::read(h.record_path()).unwrap();
        let foreground = h.run(&["webui", "--port", &port.to_string()]);
        let after_foreground = std::fs::read(h.record_path()).unwrap();
        let new_server = TcpStream::connect(("127.0.0.1", port)).is_ok();
        std::fs::write(h.record_path(), &original_bytes).unwrap();
        for output in [daemon, foreground] {
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("verify no WebUI is running"));
        }
        assert_eq!(after_daemon, invalid);
        assert_eq!(after_foreground, invalid);
        assert!(!new_server);
        assert_eq!(h.status()["pid"], original["pid"]);
    }
}

#[test]
fn restart_replaces_process_and_reuses_port() {
    let h = Harness::new();
    let before = h.start();
    h.success(&["webui", "restart"]);
    let after = h.record();
    assert_ne!(before["pid"], after["pid"]);
    assert_eq!(before["port"], after["port"]);
    assert_eq!(h.status()["pid"], after["pid"]);
}

#[test]
fn occupied_port_fails_without_publishing_record() {
    let h = Harness::new();
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    let output = h.run(&["webui", "start", "--port", &port]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed to bind"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!h.record_path().exists());
}

#[test]
fn stop_when_not_running_is_successful() {
    let h = Harness::new();
    let output = h.success(&["webui", "stop"]);
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .to_ascii_lowercase()
            .contains("not running")
    );
    assert_eq!(h.run(&["webui", "status"]).status.code(), Some(1));
}

#[cfg(unix)]
#[test]
fn killed_daemon_is_reported_stale_and_replaced() {
    let h = Harness::new();
    let original = h.start();
    let pid = original["pid"].as_u64().unwrap().to_string();
    assert!(
        Command::new("kill")
            .args(["-KILL", &pid])
            .status()
            .unwrap()
            .success()
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let output = h.run(&["webui", "status", "--json"]);
        if !output.status.success() {
            let status: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(status["running"], false);
            assert_eq!(status["stale"], true);
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "killed daemon remained live"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        h.record_path().exists(),
        "status must not remove stale records"
    );
    h.success(&["webui", "start", "--port", "0"]);
    assert_ne!(h.record()["instance_id"], original["instance_id"]);
}

#[test]
fn stale_record_is_replaced_by_start() {
    let h = Harness::new();
    let mut exited = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .arg("--version")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let dead_pid = exited.id();
    exited.wait().unwrap();
    let dir = h.record_path().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    let stale = serde_json::json!({
        "schema_version": 1,
        "instance_id": "stale-instance",
        "mode": "daemon",
        "pid": dead_pid,
        "process_started_at": 1_767_000_000u64,
        "host": "127.0.0.1",
        "port": 8484,
        "origin": "http://127.0.0.1:8484/",
        "control_token": "0".repeat(64),
        "version": "0.0.0",
        "executable": "/nonexistent/cruise",
        "working_dir": "/",
        "log_path": null,
        "started_at": 1_767_000_000u64,
    });
    std::fs::write(h.record_path(), stale.to_string()).unwrap();
    let status = h.run(&["webui", "status"]);
    assert_eq!(status.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("stale"),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );
    let record = h.start();
    assert_ne!(record["instance_id"], "stale-instance");
    assert_ne!(record["pid"].as_u64().unwrap(), u64::from(dead_pid));
    assert_eq!(h.status()["running"], true);
}

#[test]
fn start_and_stop_leave_planned_sessions_untouched() {
    let h = Harness::new();
    let manager = SessionManager::new(h.home.path().join("data/cruise"));
    let mut session = SessionState::new(
        "20261105000000".into(),
        h.home.path().to_path_buf(),
        SessionConfigRef::File {
            path: h.home.path().join("workflow.yaml"),
        },
        "planned task".into(),
    );
    session.phase = SessionPhase::Planned;
    manager.create(&session).unwrap();
    h.start();
    h.success(&["webui", "restart"]);
    let restarted = manager.load("20261105000000").unwrap();
    assert_eq!(restarted.phase, SessionPhase::Planned);
    assert_eq!(restarted.runner_pid, None);
    assert_eq!(restarted.runner_started_at, None);
    h.success(&["webui", "stop"]);
    let loaded = manager.load("20261105000000").unwrap();
    assert_eq!(loaded.phase, SessionPhase::Planned);
    assert_eq!(loaded.runner_pid, None);
    assert_eq!(loaded.runner_started_at, None);
}
