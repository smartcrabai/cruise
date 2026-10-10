//! `cruise webui` must exit promptly on SIGINT, even with an SSE client open.
#![cfg(unix)]

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(home: &std::path::Path) -> (ChildGuard, u16) {
    let child = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .args(["webui", "--no-open", "--port", "0"])
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
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to start cruise webui: {e}"));
    let mut guard = ChildGuard(child);
    let stdout = guard
        .0
        .stdout
        .take()
        .unwrap_or_else(|| panic!("stdout should be piped"));
    let mut line = String::new();
    BufReader::new(stdout)
        .read_line(&mut line)
        .unwrap_or_else(|e| panic!("failed to read listening line: {e}"));
    let port = line
        .trim()
        .rsplit(':')
        .next()
        .and_then(|tail| tail.trim_end_matches('/').parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse port from {line:?}"));
    (guard, port)
}

fn open_sse(port: u16) -> TcpStream {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .unwrap_or_else(|e| panic!("failed to connect: {e}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|e| panic!("{e}"));
    stream
        .write_all(b"GET /webui/events HTTP/1.1\r\nHost: x\r\n\r\n")
        .unwrap_or_else(|e| panic!("{e}"));
    let mut received = Vec::new();
    let mut buf = [0u8; 512];
    while !String::from_utf8_lossy(&received).contains("\r\n\r\n") {
        let n = stream
            .read(&mut buf)
            .unwrap_or_else(|e| panic!("failed to read SSE headers: {e}"));
        assert!(n > 0, "connection closed before SSE headers");
        received.extend_from_slice(&buf[..n]);
    }
    assert!(
        String::from_utf8_lossy(&received).starts_with("HTTP/1.1 200"),
        "unexpected SSE response"
    );
    stream
}

fn sigint_and_expect_exit(guard: &mut ChildGuard) {
    let status = Command::new("kill")
        .args(["-INT", &guard.0.id().to_string()])
        .status()
        .unwrap_or_else(|e| panic!("failed to run kill: {e}"));
    assert!(status.success());
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match guard
            .0
            .try_wait()
            .unwrap_or_else(|e| panic!("failed to poll child: {e}"))
        {
            Some(status) => {
                assert!(status.success(), "unexpected exit status: {status:?}");
                return;
            }
            None if Instant::now() >= deadline => {
                panic!("cruise webui did not exit within 8s of SIGINT")
            }
            None => thread::sleep(Duration::from_millis(50)),
        }
    }
}

#[test]
fn sigint_exits_even_with_open_sse_client() {
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (mut guard, port) = start(tmp.path());
    let _sse = open_sse(port);

    sigint_and_expect_exit(&mut guard);
}

#[test]
fn sigint_exits_without_clients() {
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (mut guard, _port) = start(tmp.path());

    sigint_and_expect_exit(&mut guard);
}
