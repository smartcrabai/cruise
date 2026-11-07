//! End-to-end lifecycle checks against the real CLI, with an isolated state directory.
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use cruise::session::{SessionManager, SessionPhase, SessionState};
use cruise::session_config::SessionConfigRef;
use serde_json::Value;
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Harness {
    home: TempDir,
}

impl Harness {
    fn new() -> TestResult<Self> {
        Ok(Self {
            home: TempDir::new()?,
        })
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

    fn run(&self, args: &[&str]) -> std::io::Result<Output> {
        let mut stdout = tempfile::tempfile()?;
        let mut stderr = tempfile::tempfile()?;
        let mut child = self
            .command()
            .args(args)
            .stdout(Stdio::from(stdout.try_clone()?))
            .stderr(Stdio::from(stderr.try_clone()?))
            .spawn()?;
        let status = wait_for_child(&mut child, &format!("cruise {args:?}"))?;
        stdout.seek(SeekFrom::Start(0))?;
        stderr.seek(SeekFrom::Start(0))?;
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        stdout.read_to_end(&mut stdout_bytes)?;
        stderr.read_to_end(&mut stderr_bytes)?;
        Ok(Output {
            status,
            stdout: stdout_bytes,
            stderr: stderr_bytes,
        })
    }

    fn success(&self, args: &[&str]) -> TestResult<Output> {
        let output = self.run(args)?;
        assert!(
            output.status.success(),
            "{args:?}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(output)
    }

    fn record_path(&self) -> PathBuf {
        self.home.path().join("state/cruise/webui/instance.json")
    }

    fn record(&self) -> TestResult<Value> {
        Ok(serde_json::from_slice(&std::fs::read(self.record_path())?)?)
    }

    fn start(&self) -> TestResult<Value> {
        self.success(&["webui", "start", "--port", "0"])?;
        self.record()
    }

    fn status(&self) -> TestResult<Value> {
        let output = self.success(&["webui", "status", "--json"])?;
        Ok(serde_json::from_slice(&output.stdout)?)
    }
}

fn wait_for_child(
    child: &mut Child,
    description: &str,
) -> std::io::Result<std::process::ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let timeout = std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{description} did not exit within 30 seconds"),
            );
            if let Err(kill_error) = child.kill()
                && child.try_wait()?.is_none()
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{timeout}; failed to kill child: {kill_error}"),
                ));
            }
            let kill_deadline = Instant::now() + Duration::from_secs(5);
            while child.try_wait()?.is_none() {
                if Instant::now() >= kill_deadline {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!("{timeout}; child did not exit after being killed"),
                    ));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            return Err(timeout);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // Stop even when an assertion panics, so a detached daemon does not leak.
        let _ = self.run(&["webui", "stop", "--force"]);
    }
}

fn response(port: u16) -> TestResult<String> {
    let mut socket = TcpStream::connect(("127.0.0.1", port))?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    socket.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    socket.read_to_string(&mut response)?;
    Ok(response)
}

#[test]
fn start_status_stop_round_trip() -> TestResult {
    let h = Harness::new()?;
    let record = h.start()?;
    let port = u16::try_from(record["port"].as_u64().ok_or("missing port")?)?;
    let response = response(port)?;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response
            .to_ascii_lowercase()
            .contains("cache-control: no-store"),
        "{response}"
    );
    let status = h.status()?;
    assert_eq!(status["running"], true);
    assert_eq!(status["mode"], "daemon");
    assert_eq!(status["pid"], record["pid"]);
    assert!(h.record_path().exists());
    h.success(&["webui", "stop"])?;
    assert!(!h.record_path().exists());
    assert_eq!(h.run(&["webui", "status"])?.status.code(), Some(1));
    assert!(
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(1)).is_err()
    );
    Ok(())
}

#[cfg(windows)]
#[test]
fn windows_detached_daemon_survives_start_command_exit() -> TestResult {
    let h = Harness::new()?;
    let started = h.start()?;
    let status = h.status()?;
    assert_eq!(status["running"], true);
    assert_eq!(status["pid"], started["pid"]);
    let port = u16::try_from(started["port"].as_u64().ok_or("missing port")?)?;
    assert!(response(port)?.starts_with("HTTP/1.1 200"));
    h.success(&["webui", "stop"])?;
    Ok(())
}

#[cfg(windows)]
#[test]
fn windows_start_closes_captured_output_while_daemon_remains_running() -> TestResult {
    let h = Harness::new()?;
    let mut command = h.command();
    command.args(["webui", "start", "--port", "0"]);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(command.output());
    });
    let output = match rx.recv_timeout(Duration::from_secs(30)) {
        Ok(output) => output?,
        Err(_) => {
            let _ = h.run(&["webui", "stop", "--force"]);
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "webui start did not close captured output within 30 seconds",
            )
            .into());
        }
    };
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("started in background"));
    assert_eq!(h.status()?["running"], true);
    h.success(&["webui", "stop"])?;
    Ok(())
}

#[test]
fn repeated_start_and_foreground_exclusivity() -> TestResult {
    let h = Harness::new()?;
    let original = h.start()?;
    let again = h.success(&["webui", "start"])?;
    assert!(String::from_utf8_lossy(&again.stdout).contains(&original["pid"].to_string()));
    assert_eq!(h.record()?["pid"], original["pid"]);
    let foreground = h.run(&["webui", "--port", "0"])?;
    assert!(!foreground.status.success());
    assert!(
        String::from_utf8_lossy(&foreground.stderr)
            .to_ascii_lowercase()
            .contains("already running")
    );
    let conflict = h.run(&["webui", "start", "--port", "1"])?;
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("restart"));
    assert_eq!(h.record()?["pid"], original["pid"]);
    Ok(())
}

#[test]
fn invalid_live_record_blocks_new_servers() -> TestResult {
    let h = Harness::new()?;
    let original = h.start()?;
    let original_bytes = std::fs::read(h.record_path())?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let mut unsupported = original.clone();
    unsupported["schema_version"] = 999.into();
    for invalid in [b"{broken".to_vec(), unsupported.to_string().into_bytes()] {
        std::fs::write(h.record_path(), &invalid)?;
        let daemon = h.run(&["webui", "start", "--port", &port.to_string()])?;
        let after_daemon = std::fs::read(h.record_path())?;
        let foreground = h.run(&["webui", "--port", &port.to_string()])?;
        let after_foreground = std::fs::read(h.record_path())?;
        let new_server = TcpStream::connect(("127.0.0.1", port)).is_ok();
        std::fs::write(h.record_path(), &original_bytes)?;
        for output in [daemon, foreground] {
            assert!(!output.status.success());
            assert!(String::from_utf8_lossy(&output.stderr).contains("verify no WebUI is running"));
        }
        assert_eq!(after_daemon, invalid);
        assert_eq!(after_foreground, invalid);
        assert!(!new_server);
        assert_eq!(h.status()?["pid"], original["pid"]);
    }
    Ok(())
}

#[test]
fn restart_replaces_process_and_reuses_port() -> TestResult {
    let h = Harness::new()?;
    let before = h.start()?;
    h.success(&["webui", "restart"])?;
    let after = h.record()?;
    assert_ne!(before["pid"], after["pid"]);
    assert_eq!(before["port"], after["port"]);
    assert_eq!(h.status()?["pid"], after["pid"]);
    Ok(())
}

#[test]
fn occupied_port_fails_without_publishing_record() -> TestResult {
    let h = Harness::new()?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port().to_string();
    let output = h.run(&["webui", "start", "--port", &port])?;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed to bind"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!h.record_path().exists());
    Ok(())
}

#[test]
fn stop_when_not_running_is_successful() -> TestResult {
    let h = Harness::new()?;
    let output = h.success(&["webui", "stop"])?;
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .to_ascii_lowercase()
            .contains("not running")
    );
    assert_eq!(h.run(&["webui", "status"])?.status.code(), Some(1));
    Ok(())
}

#[cfg(unix)]
#[test]
fn killed_daemon_is_reported_stale_and_replaced() -> TestResult {
    let h = Harness::new()?;
    let original = h.start()?;
    let pid = original["pid"].as_u64().ok_or("missing pid")?.to_string();
    assert!(
        Command::new("kill")
            .args(["-KILL", &pid])
            .status()?
            .success()
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let output = h.run(&["webui", "status", "--json"])?;
        if !output.status.success() {
            let status: Value = serde_json::from_slice(&output.stdout)?;
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
    h.success(&["webui", "start", "--port", "0"])?;
    assert_ne!(h.record()?["instance_id"], original["instance_id"]);
    Ok(())
}

#[test]
fn stale_record_is_replaced_by_start() -> TestResult {
    let h = Harness::new()?;
    let mut exited = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .arg("--version")
        .stdout(Stdio::null())
        .spawn()?;
    let dead_pid = exited.id();
    wait_for_child(&mut exited, "cruise --version")?;
    let dir = h
        .record_path()
        .parent()
        .ok_or("record path has no parent")?
        .to_path_buf();
    std::fs::create_dir_all(&dir)?;
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
    std::fs::write(h.record_path(), stale.to_string())?;
    let status = h.run(&["webui", "status"])?;
    assert_eq!(status.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("stale"),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );
    let record = h.start()?;
    assert_ne!(record["instance_id"], "stale-instance");
    assert_ne!(
        record["pid"].as_u64().ok_or("missing pid")?,
        u64::from(dead_pid)
    );
    assert_eq!(h.status()?["running"], true);
    // On Windows the process object stays openable while this handle is held,
    // even though wait() has confirmed that the process exited.
    drop(exited);
    Ok(())
}

#[test]
fn start_and_stop_leave_planned_sessions_untouched() -> TestResult {
    let h = Harness::new()?;
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
    manager.create(&session)?;
    h.start()?;
    h.success(&["webui", "restart"])?;
    let restarted = manager.load("20261105000000")?;
    assert_eq!(restarted.phase, SessionPhase::Planned);
    assert_eq!(restarted.runner_pid, None);
    assert_eq!(restarted.runner_started_at, None);
    h.success(&["webui", "stop"])?;
    let loaded = manager.load("20261105000000")?;
    assert_eq!(loaded.phase, SessionPhase::Planned);
    assert_eq!(loaded.runner_pid, None);
    assert_eq!(loaded.runner_started_at, None);
    Ok(())
}
