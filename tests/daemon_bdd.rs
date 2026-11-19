//! Gherkin scenarios drive the shipped CLI and its real HTTP listener.
use cucumber::{World as _, given, then, when};
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::{NamedTempFile, TempDir};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Default, cucumber::World)]
struct DaemonWorld {
    home: Option<TempDir>,
    original: Option<Value>,
    response: Option<String>,
    authenticated_response: Option<String>,
    reserved_listener: Option<TcpListener>,
}

impl DaemonWorld {
    fn command(&self, args: &[&str]) -> TestResult<Output> {
        let home = self
            .home
            .as_ref()
            .ok_or("missing isolated environment")?
            .path();
        Ok(Command::new(env!("CARGO_BIN_EXE_cruise"))
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
            .output()?)
    }

    fn success(&self, args: &[&str]) -> TestResult<Output> {
        let output = self.command(args)?;
        assert!(
            output.status.success(),
            "{args:?}: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(output)
    }

    fn record_path(&self) -> TestResult<PathBuf> {
        Ok(self
            .home
            .as_ref()
            .ok_or("missing home")?
            .path()
            .join("state/cruise/daemon/instance.json"))
    }

    fn record(&self) -> TestResult<Value> {
        Ok(serde_json::from_slice(&fs::read(self.record_path()?)?)?)
    }

    fn status(&self) -> TestResult<Value> {
        Ok(serde_json::from_slice(
            &self.success(&["daemon", "status", "--json"])?.stdout,
        )?)
    }

    fn port(&self) -> TestResult<u16> {
        Ok(u16::try_from(
            self.record()?["port"]
                .as_u64()
                .ok_or("missing daemon port")?,
        )?)
    }

    fn original_port(&self) -> TestResult<u16> {
        Ok(u16::try_from(
            self.original
                .as_ref()
                .ok_or("missing original daemon record")?["port"]
                .as_u64()
                .ok_or("missing original port")?,
        )?)
    }

    fn request(&self, method: &str, path: &str, token: Option<&str>) -> TestResult<String> {
        self.request_with_cookie(method, path, token, None)
    }

    fn request_with_cookie(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        cookie: Option<&str>,
    ) -> TestResult<String> {
        let port = self.port()?;
        let mut stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let authorization = token.map_or_else(String::new, |token| {
            format!("Authorization: Bearer {token}\r\n")
        });
        let cookie = cookie.map_or_else(String::new, |cookie| format!("Cookie: {cookie}\r\n"));
        write!(
            stream,
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{authorization}{cookie}Content-Length: 0\r\nConnection: close\r\n\r\n"
        )?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response)
    }

    fn webui_request(&self, path: &str) -> TestResult<String> {
        // Each login URL is a single-use nonce, so request a fresh one for every login.
        let output = self.success(&["webui"])?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let login_url = stdout
            .lines()
            .next()
            .ok_or("cruise webui did not print a login URL")?;
        let login_path = login_url
            .split_once("://")
            .and_then(|(_, authority_and_path)| {
                authority_and_path
                    .find('/')
                    .map(|index| &authority_and_path[index..])
            })
            .ok_or("invalid daemon WebUI login URL")?;
        let login_response = self.request("GET", login_path, None)?;
        assert_eq!(response_status(&login_response)?, 303, "{login_response}");
        let set_cookie = login_response
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("set-cookie:"))
            .ok_or("WebUI login did not set a cookie")?;
        let cookie = set_cookie
            .split_once(':')
            .ok_or("invalid Set-Cookie header")?
            .1
            .trim()
            .split(';')
            .next()
            .ok_or("empty Set-Cookie header")?;
        assert!(cookie.starts_with("cruise_webui="), "{set_cookie}");
        self.request_with_cookie("GET", path, None, Some(cookie))
    }
}

impl Drop for DaemonWorld {
    fn drop(&mut self) {
        if self.home.is_some() {
            let _ = self.command(&["daemon", "stop", "--force"]);
        }
    }
}

fn response_status(response: &str) -> TestResult<u16> {
    Ok(response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or("missing HTTP status")?
        .parse()?)
}

fn wait_for_exit(port: u16) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(100))
            .is_err()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("daemon still accepts connections after shutdown".into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[given("an isolated Cruise environment")]
fn isolated(w: &mut DaemonWorld) -> TestResult {
    w.home = Some(TempDir::new()?);
    Ok(())
}

#[when("I start the daemon on an ephemeral port")]
fn start(w: &mut DaemonWorld) -> TestResult {
    let output = w.success(&["daemon", "start", "--port", "0"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("cruise daemon WebUI login: "))
        .ok_or("daemon start did not print the WebUI login URL")?;
    w.original = Some(w.record()?);
    Ok(())
}

#[then("status reports the running daemon and its WebUI responds")]
fn running(w: &mut DaemonWorld) -> TestResult {
    let status = w.status()?;
    assert_eq!(status["running"], true);
    assert_eq!(status["mode"], "daemon");
    assert_eq!(status["pid"], w.record()?["pid"]);
    assert!(
        w.webui_request("/")?.starts_with("HTTP/1.1 200"),
        "WebUI endpoint did not respond successfully"
    );
    Ok(())
}

#[when("I stop the daemon")]
fn stop(w: &mut DaemonWorld) -> TestResult {
    w.success(&["daemon", "stop"])?;
    Ok(())
}

#[then("status reports that the daemon is not running")]
fn stopped(w: &mut DaemonWorld) -> TestResult {
    let output = w.command(&["daemon", "status", "--json"])?;
    assert_eq!(output.status.code(), Some(1));
    let status: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(status["running"], false);
    Ok(())
}

#[when("I start the daemon again without options")]
fn start_again(w: &mut DaemonWorld) -> TestResult {
    w.success(&["daemon", "start"])?;
    Ok(())
}

#[then("the existing instance remains registered")]
fn same_instance(w: &mut DaemonWorld) -> TestResult {
    assert_eq!(
        w.record()?["instance_id"],
        w.original
            .as_ref()
            .ok_or("missing original daemon record")?["instance_id"]
    );
    Ok(())
}

#[then("a foreground daemon cannot start concurrently")]
fn foreground_conflict(w: &mut DaemonWorld) -> TestResult {
    let output = w.command(&["daemon", "serve", "--port", "0"])?;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .to_lowercase()
            .contains("already running")
    );
    Ok(())
}

#[when("I restart the daemon")]
fn restart(w: &mut DaemonWorld) -> TestResult {
    w.success(&["daemon", "restart"])?;
    Ok(())
}

#[then("a new daemon is running on the original port")]
fn restarted(w: &mut DaemonWorld) -> TestResult {
    let old = w
        .original
        .as_ref()
        .ok_or("missing original daemon record")?;
    let new = w.record()?;
    assert_ne!(new["instance_id"], old["instance_id"]);
    assert_ne!(new["pid"], old["pid"]);
    assert_eq!(new["port"], old["port"]);
    assert_eq!(w.status()?["running"], true);
    w.original = Some(new);
    Ok(())
}

#[when("I run the list client without starting the daemon")]
fn list_autostarts(w: &mut DaemonWorld) -> TestResult {
    assert!(!w.record_path()?.exists());
    w.success(&["list", "--json"])?;
    Ok(())
}

#[then("the client has started and registered the daemon")]
fn autostarted(w: &mut DaemonWorld) -> TestResult {
    let record = w.record()?;
    assert_eq!(record["mode"], "daemon");
    assert_eq!(w.status()?["running"], true);
    w.original = Some(record);
    Ok(())
}

#[when("I remove the daemon registry record")]
fn remove_registry(w: &mut DaemonWorld) -> TestResult {
    fs::remove_file(w.record_path()?)?;
    Ok(())
}

#[then("the daemon exits after registry removal")]
fn exits_after_registry_removal(w: &mut DaemonWorld) -> TestResult {
    wait_for_exit(w.original_port()?)
}

#[when("I start the daemon without a port while the default port is busy")]
fn start_with_busy_default(w: &mut DaemonWorld) -> TestResult {
    match TcpListener::bind(("127.0.0.1", 8484)) {
        Ok(listener) => w.reserved_listener = Some(listener),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {}
        Err(error) => return Err(error.into()),
    }
    let output = w.success(&["daemon", "start"])?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(
            "warning: default daemon port 8484 is busy; using ephemeral port. Set --port or CRUISE_DAEMON_PORT explicitly on shared hosts."
        ),
        "{stderr}"
    );
    w.original = Some(w.record()?);
    Ok(())
}

#[then("the daemon is registered on an ephemeral port")]
fn falls_back_to_ephemeral(w: &mut DaemonWorld) -> TestResult {
    let record = w.record()?;
    assert_ne!(record["port"], 8484);
    assert_eq!(w.status()?["running"], true);
    let log_path = record["log_path"]
        .as_str()
        .ok_or("missing daemon log path")?;
    let log = fs::read_to_string(log_path)?;
    assert!(
        log.contains("warning: default daemon port 8484 is busy; using an ephemeral port"),
        "{log}"
    );
    Ok(())
}

#[when("I start the daemon on an occupied explicit port")]
fn start_on_occupied_explicit_port(w: &mut DaemonWorld) -> TestResult {
    w.reserved_listener = Some(TcpListener::bind(("127.0.0.1", 0))?);
    let port = w
        .reserved_listener
        .as_ref()
        .ok_or("missing reserved listener")?
        .local_addr()?
        .port()
        .to_string();
    let output = w.command(&["daemon", "start", "--port", &port])?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to bind"));
    Ok(())
}

#[then("startup fails without publishing a registry record")]
fn no_failed_start_record(w: &mut DaemonWorld) -> TestResult {
    assert!(!w.record_path()?.exists());
    Ok(())
}

#[when("I change the registered daemon version")]
fn change_registered_version(w: &mut DaemonWorld) -> TestResult {
    let mut record = w.record()?;
    record["version"] = "0.0.0-incompatible".into();
    let path = w.record_path()?;
    let directory = path.parent().ok_or("registry path has no parent")?;
    let mut replacement = NamedTempFile::new_in(directory)?;
    serde_json::to_writer(replacement.as_file_mut(), &record)?;
    replacement.persist(path)?;
    Ok(())
}

#[when("I run the list client")]
fn list_client(w: &mut DaemonWorld) -> TestResult {
    let output = w.success(&["list", "--json"])?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let _: Value = serde_json::from_slice(&output.stdout)?;
    let token = w
        .original
        .as_ref()
        .and_then(|record| record["token"].as_str())
        .ok_or("missing original daemon token")?;
    assert!(
        !stdout.contains(token),
        "client printed API token: {stdout}"
    );
    Ok(())
}

#[then("the client keeps the current daemon and status reports its process version")]
fn stale_registry_version_does_not_restart(w: &mut DaemonWorld) -> TestResult {
    let original = w
        .original
        .as_ref()
        .ok_or("missing original daemon record")?;
    let current = w.record()?;
    assert_eq!(current["instance_id"], original["instance_id"]);
    assert_eq!(current["pid"], original["pid"]);
    assert_eq!(current["version"], "0.0.0-incompatible");

    let token = original["token"].as_str().ok_or("missing daemon token")?;
    let response = w.request("GET", "/api/v1/status", Some(token))?;
    assert_eq!(response_status(&response)?, 200, "{response}");
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .ok_or("API status response has no body")?;
    let status: Value = serde_json::from_str(body)?;
    assert_eq!(status["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(status["pid"], original["pid"]);
    Ok(())
}

#[when("I request API status without a bearer token")]
fn api_status_unauthenticated(w: &mut DaemonWorld) -> TestResult {
    w.response = Some(w.request("GET", "/api/v1/status", None)?);
    Ok(())
}

#[then("API status rejects the request")]
fn api_status_rejected(w: &mut DaemonWorld) -> TestResult {
    assert_eq!(
        response_status(w.response.as_deref().ok_or("missing API response")?)?,
        401
    );
    Ok(())
}

#[when("I request API status with the registered bearer token")]
fn api_status_authenticated(w: &mut DaemonWorld) -> TestResult {
    let record = w.record()?;
    let token = record["token"].as_str().ok_or("missing daemon token")?;
    w.authenticated_response = Some(w.request("GET", "/api/v1/status", Some(token))?);
    Ok(())
}

#[then("API status returns success")]
fn api_status_ok(w: &mut DaemonWorld) -> TestResult {
    assert_eq!(
        response_status(
            w.authenticated_response
                .as_deref()
                .ok_or("missing authenticated API response")?
        )?,
        200
    );
    Ok(())
}

#[when("I send an unauthenticated API shutdown request")]
fn shutdown_unauthenticated(w: &mut DaemonWorld) -> TestResult {
    w.response = Some(w.request("POST", "/api/v1/shutdown", None)?);
    Ok(())
}

#[then("shutdown is rejected and the daemon remains running")]
fn shutdown_rejected(w: &mut DaemonWorld) -> TestResult {
    assert_eq!(
        response_status(w.response.as_deref().ok_or("missing shutdown response")?)?,
        401
    );
    assert_eq!(w.status()?["running"], true);
    Ok(())
}

#[when("I send an authenticated API shutdown request")]
fn shutdown_authenticated(w: &mut DaemonWorld) -> TestResult {
    let record = w.record()?;
    let token = record["token"].as_str().ok_or("missing daemon token")?;
    w.authenticated_response = Some(w.request("POST", "/api/v1/shutdown", Some(token))?);
    Ok(())
}

#[then("shutdown is accepted and the daemon exits")]
fn shutdown_accepted(w: &mut DaemonWorld) -> TestResult {
    assert_eq!(
        response_status(
            w.authenticated_response
                .as_deref()
                .ok_or("missing authenticated shutdown response")?
        )?,
        202
    );
    wait_for_exit(w.original_port()?)
}

#[tokio::main]
async fn main() {
    DaemonWorld::cucumber()
        .max_concurrent_scenarios(1)
        .fail_on_skipped()
        .run_and_exit(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/daemon.feature"
        ))
        .await;
}
