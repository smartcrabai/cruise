//! `cruise daemon serve` must exit promptly on SIGINT, even with an SSE client open.
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

fn start(home: &std::path::Path) -> (ChildGuard, u16, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    for key in cruise::config_env::all_override_names() {
        command.env_remove(key);
    }
    let child = command
        .args(["daemon", "serve", "--port", "0"])
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("GIT_CONFIG_COUNT", "0")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env("CRUISE_UI_LANGUAGE", "en")
        .env_remove("HERDR_ENV")
        .env_remove("CRUISE_CONFIG")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to start cruise daemon serve: {e}"));
    let mut guard = ChildGuard(child);
    let stdout = guard
        .0
        .stdout
        .take()
        .unwrap_or_else(|| panic!("stdout should be piped"));
    let mut stdout = BufReader::new(stdout);
    let mut listening_line = String::new();
    stdout
        .read_line(&mut listening_line)
        .unwrap_or_else(|e| panic!("failed to read listening line: {e}"));
    let port = listening_line
        .trim()
        .rsplit(':')
        .next()
        .and_then(|tail| tail.trim_end_matches('/').parse::<u16>().ok())
        .unwrap_or_else(|| panic!("could not parse port from {listening_line:?}"));
    let mut login_line = String::new();
    stdout
        .read_line(&mut login_line)
        .unwrap_or_else(|e| panic!("failed to read WebUI login line: {e}"));
    let login_url = login_line
        .trim()
        .strip_prefix("cruise daemon WebUI login: ")
        .unwrap_or_else(|| panic!("could not parse WebUI login URL from {login_line:?}"))
        .to_owned();
    (guard, port, login_url)
}

fn response_headers(stream: &mut TcpStream) -> String {
    let mut received = Vec::new();
    let mut buf = [0u8; 512];
    while !String::from_utf8_lossy(&received).contains("\r\n\r\n") {
        let n = stream
            .read(&mut buf)
            .unwrap_or_else(|e| panic!("failed to read response headers: {e}"));
        assert!(n > 0, "connection closed before response headers");
        received.extend_from_slice(&buf[..n]);
    }
    String::from_utf8_lossy(&received).into_owned()
}

fn open_sse(port: u16, login_url: &str) -> TcpStream {
    let login_url =
        url::Url::parse(login_url).unwrap_or_else(|error| panic!("invalid login URL: {error}"));
    let login_path = format!(
        "{}?{}",
        login_url.path(),
        login_url.query().unwrap_or_default()
    );
    let host = format!("127.0.0.1:{port}");
    let mut login = TcpStream::connect(("127.0.0.1", port))
        .unwrap_or_else(|error| panic!("failed to connect for login: {error}"));
    login
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|error| panic!("{error}"));
    write!(
        login,
        "GET {login_path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap_or_else(|error| panic!("failed to write login request: {error}"));
    let login_response = response_headers(&mut login);
    assert!(
        login_response.starts_with("HTTP/1.1 303"),
        "unexpected WebUI login response: {login_response}"
    );
    let cookie = login_response
        .lines()
        .find(|line| line.to_ascii_lowercase().starts_with("set-cookie:"))
        .and_then(|line| line.split_once(':').map(|(_, value)| value.trim()))
        .and_then(|value| value.split(';').next())
        .unwrap_or_else(|| panic!("WebUI login response had no session cookie: {login_response}"));

    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .unwrap_or_else(|error| panic!("failed to connect for SSE: {error}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|error| panic!("{error}"));
    write!(
        stream,
        "GET /webui/events HTTP/1.1\r\nHost: {host}\r\nCookie: {cookie}\r\n\r\n"
    )
    .unwrap_or_else(|error| panic!("failed to write SSE request: {error}"));
    let response = response_headers(&mut stream);
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "unexpected SSE response: {response}"
    );
    stream
}
#[test]
fn sigint_exits_even_with_open_sse_client() {
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (mut guard, port, login_url) = start(tmp.path());
    let _sse = open_sse(port, &login_url);

    sigint_and_expect_exit(&mut guard);
}

#[test]
fn sigint_exits_without_clients() {
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let (mut guard, _port, _login_url) = start(tmp.path());

    sigint_and_expect_exit(&mut guard);
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
                panic!("cruise daemon did not exit within 8s of SIGINT")
            }
            None => thread::sleep(Duration::from_millis(50)),
        }
    }
}
