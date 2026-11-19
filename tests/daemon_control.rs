//! Real CLI and HTTP coverage for authenticated daemon shutdown.

use std::fs;
use std::io::{BufRead as _, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
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
        self.home.path().join("state/cruise/daemon/instance.json")
    }

    fn start(&self) -> Value {
        let output = self.run(&["daemon", "start", "--port", "0"]);
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
            let _ = self.run(&["daemon", "stop", "--force"]);
        }
    }
}

struct RestoreRecord {
    path: PathBuf,
    contents: Vec<u8>,
}

impl Drop for RestoreRecord {
    fn drop(&mut self) {
        let _ = fs::write(&self.path, &self.contents);
    }
}

fn post_stop(port: u16, token: Option<&str>) -> (u16, String, String) {
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).unwrap_or_else(|e| panic!("connect: {e}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|e| panic!("read timeout: {e}"));
    let header = token.map_or_else(String::new, |token| {
        format!("Authorization: Bearer {token}\r\n")
    });
    write!(stream, "POST /api/v1/shutdown HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{header}Content-Length: 0\r\nConnection: close\r\n\r\n").unwrap_or_else(|e| panic!("send request: {e}"));
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
fn get_api_status(port: u16, token: Option<&str>) -> u16 {
    get_api_status_response(port, token).0
}

fn get_api_status_response(port: u16, token: Option<&str>) -> (u16, String) {
    let mut stream =
        TcpStream::connect(("127.0.0.1", port)).unwrap_or_else(|e| panic!("connect: {e}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|e| panic!("read timeout: {e}"));
    let header = token.map_or_else(String::new, |token| {
        format!("Authorization: Bearer {token}\r\n")
    });
    write!(
        stream,
        "GET /api/v1/status HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{header}Connection: close\r\n\r\n"
    )
    .unwrap_or_else(|e| panic!("send request: {e}"));
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
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("invalid HTTP response: {response:?}"));
    (status, body.to_owned())
}

fn set_blackhole_proxy(command: &mut Command) {
    command
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("http_proxy", "http://127.0.0.1:1")
        .env_remove("HTTPS_PROXY")
        .env_remove("https_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy");
}

fn wait_for_removal(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while path.exists() {
        assert!(
            Instant::now() < deadline,
            "daemon instance record remained after shutdown"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn api_shutdown_requires_token_and_returns_response() {
    let daemon = Daemon::new();
    let record = daemon.start();
    let port = record["port"]
        .as_u64()
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or_else(|| panic!("invalid port in record"));
    let token = record["token"]
        .as_str()
        .unwrap_or_else(|| panic!("missing daemon token"));
    assert_eq!(token.len(), 64);
    let original_record = fs::read(daemon.record_path())
        .unwrap_or_else(|error| panic!("read original daemon record: {error}"));
    let restore_record = RestoreRecord {
        path: daemon.record_path(),
        contents: original_record,
    };
    let mut stale_record = record.clone();
    stale_record["version"] = Value::String("0.0.0".to_owned());
    stale_record["pid"] = Value::from(0);
    fs::write(
        daemon.record_path(),
        serde_json::to_vec(&stale_record)
            .unwrap_or_else(|error| panic!("encode stale daemon record: {error}")),
    )
    .unwrap_or_else(|error| panic!("write stale daemon record: {error}"));
    let (status_code, status_body) = get_api_status_response(port, Some(token));
    drop(restore_record);
    assert_eq!(status_code, 200);
    let status: Value = serde_json::from_str(&status_body)
        .unwrap_or_else(|error| panic!("parse API status: {error}"));
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["pid"], record["pid"]);

    let wrong = format!("{}0", &token[..63]);
    let wrong = if wrong == token {
        format!("{}1", &token[..63])
    } else {
        wrong
    };

    for candidate in [None, Some(wrong.as_str()), Some(&token[..63])] {
        let (status, headers, _) = post_stop(port, candidate);
        assert_eq!(status, 401, "unexpected response: {headers}");
        assert_eq!(get_api_status(port, candidate), 401);
        let output = daemon.run(&["daemon", "status", "--json"]);
        assert!(
            output.status.success(),
            "unauthorized request stopped server: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let status: Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|e| panic!("parse status: {e}"));
        assert_eq!(status["running"], true);
    }
    assert_eq!(get_api_status(port, Some(token)), 200);

    let (status, headers, body) = post_stop(port, Some(token));
    assert_eq!(status, 202, "unexpected response: {headers} {body}");
    assert!(headers.contains("connection: close"), "{headers}");
    let body: Value = serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("parse shutdown response: {error}, body: {body:?}"));
    assert_eq!(body["activeOperations"], 0);
    wait_for_removal(&daemon.record_path());
    let output = daemon.run(&["daemon", "status", "--json"]);
    assert!(
        !output.status.success(),
        "server still running after authorized stop"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", port)).is_err(),
        "server still accepts connections"
    );
}

#[test]
fn daemon_http_clients_bypass_proxy_environment() {
    let daemon = Daemon::new();
    let mut start = daemon.command();
    set_blackhole_proxy(&mut start);
    let output = start
        .args(["daemon", "start", "--port", "0"])
        .output()
        .unwrap_or_else(|error| panic!("start daemon with proxy set: {error}"));
    assert!(
        output.status.success(),
        "daemon start failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let login_line = stdout
        .lines()
        .find(|line| line.starts_with("cruise daemon WebUI login: "))
        .unwrap_or_else(|| panic!("missing login URL: {stdout}"));
    assert!(login_line.contains("/login?nonce="), "{login_line}");
    assert!(!login_line.contains("token="), "{login_line}");

    let mut webui = daemon.command();
    set_blackhole_proxy(&mut webui);
    let output = webui
        .args(["webui"])
        .output()
        .unwrap_or_else(|error| panic!("request WebUI URL with proxy set: {error}"));
    assert!(
        output.status.success(),
        "cruise webui failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let url = String::from_utf8_lossy(&output.stdout);
    assert!(url.contains("/login?nonce="), "{url}");
    assert!(!url.contains("token="), "{url}");

    let mut stop = daemon.command();
    set_blackhole_proxy(&mut stop);
    let output = stop
        .args(["daemon", "stop", "--force"])
        .output()
        .unwrap_or_else(|error| panic!("stop daemon with proxy set: {error}"));
    assert!(
        output.status.success(),
        "daemon stop failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn mock_outdated_status_server(
    listener: TcpListener,
    daemon_port: u16,
    token: String,
) -> thread::JoinHandle<()> {
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("set mock listener nonblocking: {error}"));
    thread::spawn(move || {
        for _ in 0..3 {
            let deadline = Instant::now() + Duration::from_secs(30);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept mock request: {error}"),
                }
            };
            // Windows sockets inherit non-blocking mode from the listener.
            stream
                .set_nonblocking(false)
                .unwrap_or_else(|error| panic!("set mock stream blocking: {error}"));
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap_or_else(|error| panic!("mock request timeout: {error}"));
            let mut reader = std::io::BufReader::new(
                stream
                    .try_clone()
                    .unwrap_or_else(|error| panic!("clone mock stream: {error}")),
            );
            let mut request_line = String::new();
            reader
                .read_line(&mut request_line)
                .unwrap_or_else(|error| panic!("read mock request line: {error}"));
            loop {
                let mut header = String::new();
                reader
                    .read_line(&mut header)
                    .unwrap_or_else(|error| panic!("read mock request header: {error}"));
                if header == "\r\n" || header.is_empty() {
                    break;
                }
            }
            drop(reader);

            let (status, body) = if request_line.starts_with("GET /api/v1/status ") {
                (
                    "200 OK",
                    r#"{"version":"0.0.0-incompatible","activeOperations":0,"batchActive":false}"#,
                )
            } else if request_line.starts_with("POST /api/v1/shutdown ") {
                let (status, _, _) = post_stop(daemon_port, Some(&token));
                assert_eq!(status, 202, "mock could not stop old daemon");
                ("202 Accepted", r#"{"activeOperations":0}"#)
            } else {
                panic!("unexpected mock request: {request_line}");
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap_or_else(|error| panic!("write mock response: {error}"));
        }
    })
}

#[test]
fn outdated_daemon_restart_keeps_list_json_clean() {
    let daemon = Daemon::new();
    let record = daemon.start();
    let port = record["port"]
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .unwrap_or_else(|| panic!("invalid port in record"));
    let token = record["token"]
        .as_str()
        .unwrap_or_else(|| panic!("missing daemon token"))
        .to_owned();
    let original_id = record["instance_id"].clone();

    let listener = TcpListener::bind(("127.0.0.1", 0))
        .unwrap_or_else(|error| panic!("mock listener: {error}"));
    let mock_addr = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("mock listener address: {error}"));
    let mock = mock_outdated_status_server(listener, port, token.clone());
    let record_path = daemon.record_path();
    let mut stale = record.clone();
    stale["version"] = Value::String("0.0.0-incompatible".to_owned());
    stale["origin"] = Value::String(format!("http://{mock_addr}/"));
    fs::write(
        &record_path,
        serde_json::to_vec(&stale)
            .unwrap_or_else(|error| panic!("encode old daemon record: {error}")),
    )
    .unwrap_or_else(|error| panic!("write old daemon record: {error}"));

    let mut command = daemon.command();
    command.env("CRUISE_MODEL", "version-restart-only-model");
    let output = command
        .args(["list", "--json"])
        .output()
        .unwrap_or_else(|error| panic!("run client against old daemon: {error}"));
    mock.join()
        .unwrap_or_else(|error| panic!("mock server thread: {error:?}"));
    assert!(
        output.status.success(),
        "list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("list output was not JSON: {error}; stdout={stdout}"));
    assert!(
        !stdout.contains(&token),
        "list output contained API token: {stdout}"
    );
    assert!(
        !stdout.contains("daemon stopped"),
        "lifecycle chatter: {stdout}"
    );
    assert!(
        !stdout.contains("cruise daemon WebUI login:"),
        "restart printed a login URL: {stdout}"
    );

    let bytes = fs::read(record_path)
        .unwrap_or_else(|error| panic!("read restarted daemon record: {error}"));
    let restarted: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("parse restarted daemon record: {error}"));
    assert_ne!(restarted["instance_id"], original_id);
    assert_ne!(restarted["pid"], record["pid"]);
    assert_eq!(restarted["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(restarted["port"], record["port"]);
    assert_ne!(restarted["token"], token);
    assert!(!stdout.contains(restarted["token"].as_str().unwrap_or_default()));
}
