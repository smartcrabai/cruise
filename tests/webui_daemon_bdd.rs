//! Gherkin scenarios drive the shipped CLI and its real HTTP listener.
use cucumber::{World as _, given, then, when};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use tempfile::TempDir;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Default, cucumber::World)]
struct WebuiWorld {
    home: Option<TempDir>,
    original: Option<Value>,
    response: Option<String>,
}

impl WebuiWorld {
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
    fn record(&self) -> TestResult<Value> {
        let path = self
            .home
            .as_ref()
            .ok_or("missing home")?
            .path()
            .join("state/cruise/webui/instance.json");
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }
    fn status(&self) -> TestResult<Value> {
        Ok(serde_json::from_slice(
            &self.success(&["webui", "status", "--json"])?.stdout,
        )?)
    }
    fn request(&self, request: &str) -> TestResult<String> {
        let port = u16::try_from(self.record()?["port"].as_u64().ok_or("missing port")?)?;
        let mut stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.write_all(request.as_bytes())?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Ok(response)
    }
}
impl Drop for WebuiWorld {
    fn drop(&mut self) {
        if self.home.is_some() {
            let _ = self.command(&["webui", "stop", "--force"]);
        }
    }
}

#[given("an isolated Cruise environment")]
fn isolated(w: &mut WebuiWorld) -> TestResult {
    w.home = Some(TempDir::new()?);
    Ok(())
}
#[when("I start the WebUI on an ephemeral port")]
fn start(w: &mut WebuiWorld) -> TestResult {
    w.success(&["webui", "start", "--port", "0"])?;
    w.original = Some(w.record()?);
    Ok(())
}
#[then("status reports the running daemon and its endpoint responds")]
fn running(w: &mut WebuiWorld) -> TestResult {
    let status = w.status()?;
    assert_eq!(status["running"], true);
    assert_eq!(status["mode"], "daemon");
    assert_eq!(status["pid"], w.record()?["pid"]);
    assert!(
        w.request("GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?
            .starts_with("HTTP/1.1 200")
    );
    Ok(())
}
#[when("I stop the WebUI")]
fn stop(w: &mut WebuiWorld) -> TestResult {
    w.success(&["webui", "stop"])?;
    Ok(())
}
#[then("status reports that the WebUI is not running")]
fn stopped(w: &mut WebuiWorld) -> TestResult {
    let output = w.command(&["webui", "status", "--json"])?;
    assert_eq!(output.status.code(), Some(1));
    let status: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(status["running"], false);
    Ok(())
}
#[when("I start the WebUI again without options")]
fn start_again(w: &mut WebuiWorld) -> TestResult {
    w.success(&["webui", "start"])?;
    Ok(())
}
#[then("the existing instance remains registered")]
fn same_instance(w: &mut WebuiWorld) -> TestResult {
    assert_eq!(
        w.record()?["instance_id"],
        w.original.as_ref().ok_or("missing original")?["instance_id"]
    );
    Ok(())
}
#[then("a foreground WebUI cannot start concurrently")]
fn foreground_conflict(w: &mut WebuiWorld) -> TestResult {
    let output = w.command(&["webui", "--port", "0"])?;
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .to_lowercase()
            .contains("already running")
    );
    Ok(())
}
#[when("I restart the WebUI")]
fn restart(w: &mut WebuiWorld) -> TestResult {
    w.success(&["webui", "restart"])?;
    Ok(())
}
#[then("a new daemon is running on the original port")]
fn restarted(w: &mut WebuiWorld) -> TestResult {
    let old = w.original.as_ref().ok_or("missing original")?;
    let new = w.record()?;
    assert_ne!(new["instance_id"], old["instance_id"]);
    assert_ne!(new["pid"], old["pid"]);
    assert_eq!(new["port"], old["port"]);
    assert_eq!(w.status()?["running"], true);
    Ok(())
}
#[when("I send an unauthenticated control stop request")]
fn unauthorized(w: &mut WebuiWorld) -> TestResult {
    w.response = Some(w.request("POST /webui/control/stop HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?);
    Ok(())
}
#[then("the control request is rejected and the daemon remains running")]
fn rejected(w: &mut WebuiWorld) -> TestResult {
    assert!(
        w.response
            .as_ref()
            .ok_or("missing response")?
            .starts_with("HTTP/1.1 401")
    );
    assert_eq!(w.status()?["running"], true);
    Ok(())
}

#[tokio::main]
async fn main() {
    WebuiWorld::cucumber()
        .max_concurrent_scenarios(1)
        .fail_on_skipped()
        .run_and_exit(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/webui_daemon.feature"
        ))
        .await;
}
