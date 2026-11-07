//! Real CLI and HTTP coverage for the WebUI control endpoint.

use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

struct Daemon {
    home: TempDir,
}

impl Daemon {
    fn new() -> Self {
        Self {
            home: TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}")),
        }
    }

    fn command(&self) -> Command {
        let home = self.home.path();
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        cmd.env("HOME", home)
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
        self.command()
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("run {args:?}: {e}"))
    }

    fn record_path(&self) -> PathBuf {
        self.home.path().join("state/cruise/webui/instance.json")
    }

    fn start(&self) -> Value {
        let output = self.run(&["webui", "start", "--port", "0"]);
        assert!(
            output.status.success(),
            "start failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let bytes =
            fs::read(self.record_path()).unwrap_or_else(|e| panic!("read instance record: {e}"));
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse instance record: {e}"))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Stop even after an assertion fails, so detached processes do not leak.
        if self.record_path().exists() {
            let _ = self.run(&["webui", "stop", "--force"]);
        }
    }
}

fn post_stop(port: u16, token: Option<&str>) -> (u16, String, String) {
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).unwrap_or_else(|e| panic!("connect: {e}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|e| panic!("read timeout: {e}"));
    let header = token.map_or_else(String::new, |t| format!("X-Cruise-Webui-Control: {t}\r\n"));
    write!(stream, "POST /webui/control/stop HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{header}Content-Length: 0\r\nConnection: close\r\n\r\n").unwrap_or_else(|e| panic!("send request: {e}"));
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .unwrap_or_else(|e| panic!("read response: {e}"));
    let (headers, body) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("invalid HTTP response: {response:?}"));
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("invalid HTTP status: {headers:?}"));
    (status, headers.to_ascii_lowercase(), body.to_owned())
}

fn wait_for_removal(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while path.exists() {
        assert!(
            Instant::now() < deadline,
            "instance record remained after control stop"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn control_stop_requires_token_and_returns_shutdown_response() {
    let daemon = Daemon::new();
    let record = daemon.start();
    let port = record["port"]
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or_else(|| panic!("invalid port in record"));
    let token = record["control_token"]
        .as_str()
        .unwrap_or_else(|| panic!("missing control token"));
    assert_eq!(token.len(), 64);
    let wrong = format!("{}0", &token[..63]);
    let wrong = if wrong == token {
        format!("{}1", &token[..63])
    } else {
        wrong
    };

    for candidate in [None, Some(wrong.as_str()), Some(&token[..63])] {
        let (status, headers, _) = post_stop(port, candidate);
        assert_eq!(status, 401, "unexpected response: {headers}");
        assert!(headers.contains("cache-control: no-store"), "{headers}");
        let output = daemon.run(&["webui", "status", "--json"]);
        assert!(
            output.status.success(),
            "unauthorized request stopped server: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let status: Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("parse status: {e}"));
        assert_eq!(status["running"], true);
    }

    let (status, headers, body) = post_stop(port, Some(token));
    assert_eq!(status, 202, "unexpected response: {headers} {body}");
    assert!(headers.contains("connection: close"), "{headers}");
    assert!(headers.contains("cache-control: no-store"), "{headers}");
    let body: Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("parse control response: {e}, body: {body:?}"));
    assert_eq!(body["active_operations"], 0);
    wait_for_removal(&daemon.record_path());
    let output = daemon.run(&["webui", "status", "--json"]);
    assert!(
        !output.status.success(),
        "server still running after authorized stop"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "server still accepts connections"
    );
}
