//! Legacy detached-WebUI migration checks against the real CLI.
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
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
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        command
            .current_dir(home)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("HERDR_ENV")
            .env_remove("CRUISE_CONFIG")
            .env_remove("CRUISE_MODEL")
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> std::io::Result<Output> {
        self.command().args(args).output()
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

    fn state_dir(&self) -> PathBuf {
        self.home.path().join("state/cruise")
    }

    fn legacy_dir(&self) -> PathBuf {
        self.state_dir().join("webui")
    }

    fn daemon_record_path(&self) -> PathBuf {
        self.state_dir().join("daemon/instance.json")
    }

    fn write_legacy_record(
        &self,
        pid: u32,
        identity: &str,
        started_at: u64,
        origin: &str,
    ) -> TestResult {
        let parsed = url::Url::parse(origin)?;
        let value = json!({
            "schema_version": 1,
            "instance_id": "legacy-webui-instance",
            "mode": "daemon",
            "pid": pid,
            "process_started_at": started_at,
            "process_identity": identity,
            "host": "127.0.0.1",
            "port": parsed.port().ok_or("missing server port")?,
            "origin": origin,
            "control_token": "legacy-control-secret",
            "version": "0.3.0",
            "executable": "cruise",
            "working_dir": self.home.path(),
            "log_path": self.legacy_dir().join("webui.log"),
            "started_at": 1_700_000_000u64
        });
        std::fs::create_dir_all(self.legacy_dir())?;
        std::fs::write(self.legacy_dir().join("instance.json"), value.to_string())?;
        Ok(())
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let home = self.home.path();
        let _ = Command::new(env!("CARGO_BIN_EXE_cruise"))
            .args(["daemon", "stop", "--force"])
            .current_dir(home)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_STATE_HOME", home.join("state"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

struct ChildGuard {
    pid: u32,
    stop: mpsc::Sender<()>,
    result: Arc<Mutex<Option<ExitStatus>>>,
    reaper: Option<JoinHandle<()>>,
}

impl ChildGuard {
    fn id(&self) -> u32 {
        self.pid
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(reaper) = self.reaper.take() {
            reaper
                .join()
                .map_err(|_| io::Error::other("legacy helper reaper panicked"))?;
        }
        self.result
            .lock()
            .map_err(|_| io::Error::other("legacy helper status lock poisoned"))?
            .take()
            .ok_or_else(|| io::Error::other("legacy helper exited without a status"))
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(reaper) = self.reaper.take() {
            let _ = reaper.join();
        }
    }
}

struct LegacyServer {
    child: ChildGuard,
    origin: String,
    identity: String,
    started_at: u64,
}

fn start_legacy_server(token_path: &Path, record_path: &Path) -> TestResult<LegacyServer> {
    let ready_path = token_path.with_extension("ready");
    let mut child = Command::new(std::env::current_exe()?)
        .args(["--ignored", "--exact", "legacy_server_helper"])
        .env("CRUISE_LEGACY_HELPER", "1")
        .env("CRUISE_LEGACY_TOKEN_PATH", token_path)
        .env("CRUISE_LEGACY_READY_PATH", &ready_path)
        .env("CRUISE_LEGACY_RECORD_PATH", record_path)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let origin = loop {
        if let Ok(origin) = std::fs::read_to_string(&ready_path) {
            break origin;
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!("legacy helper exited before ready: {status}").into());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("legacy helper did not become ready".into());
        }
        thread::sleep(Duration::from_millis(10));
    };
    let origin = origin.trim().to_owned();
    let pid = child.id();
    let identity = process_identity(pid).ok_or("cannot inspect helper process identity")?;
    let started_at = process_start_time(pid).ok_or("cannot inspect helper start time")?;
    let result = Arc::new(Mutex::new(None));
    let (stop, stop_receiver) = mpsc::channel();
    let reaper_result = Arc::clone(&result);
    let reaper = thread::spawn(move || {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    *reaper_result
                        .lock()
                        .unwrap_or_else(|error| panic!("{error}")) = Some(status);
                    return;
                }
                Err(_) => {
                    let _ = child.kill();
                    let status = child.wait().ok();
                    *reaper_result
                        .lock()
                        .unwrap_or_else(|error| panic!("{error}")) = status;
                    return;
                }
                Ok(None) => {}
            }
            if stop_receiver.try_recv().is_ok() {
                let _ = child.kill();
                let status = child.wait().ok();
                *reaper_result
                    .lock()
                    .unwrap_or_else(|error| panic!("{error}")) = status;
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    Ok(LegacyServer {
        child: ChildGuard {
            pid,
            stop,
            result,
            reaper: Some(reaper),
        },
        origin,
        identity,
        started_at,
    })
}

#[test]
fn live_legacy_server_blocks_autostart_and_is_stopped_with_its_token() -> TestResult {
    let harness = Harness::new()?;
    let token_path = harness.home.path().join("received-control-token");
    let mut server = start_legacy_server(&token_path, &harness.legacy_dir().join("instance.json"))?;
    harness.write_legacy_record(
        server.child.id(),
        &server.identity,
        server.started_at,
        &server.origin,
    )?;
    std::fs::write(harness.legacy_dir().join("instance.lock"), b"")?;
    std::fs::write(harness.legacy_dir().join("webui.log"), b"legacy log")?;

    let status = harness.run(&["daemon", "status", "--json"])?;
    let value: Value = serde_json::from_slice(&status.stdout)?;
    assert_eq!(value["legacy_webui"]["running"], true);
    assert_eq!(value["legacy_webui"]["pid"], server.child.id());
    let blocked = harness.run(&["list", "--json"])?;
    assert!(!blocked.status.success());
    let diagnostic = String::from_utf8_lossy(&blocked.stderr);
    assert!(
        diagnostic.contains(&format!("PID {}", server.child.id())),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("version 0.3.0"), "{diagnostic}");
    assert!(diagnostic.contains(&server.origin), "{diagnostic}");
    assert!(diagnostic.contains("cruise daemon stop"), "{diagnostic}");
    assert!(diagnostic.contains("Suspended"), "{diagnostic}");
    assert!(diagnostic.contains("cruise run <id>"), "{diagnostic}");
    assert!(!harness.daemon_record_path().exists());

    let stopped = harness.success(&["daemon", "stop"])?;
    assert!(String::from_utf8_lossy(&stopped.stdout).contains("legacy WebUI server stopped"));
    assert_eq!(
        std::fs::read_to_string(token_path)?,
        "legacy-control-secret"
    );
    assert!(server.child.wait()?.success());
    assert!(!harness.legacy_dir().join("instance.json").exists());
    assert!(!harness.legacy_dir().join("instance.lock").exists());
    assert!(!harness.legacy_dir().join("webui.log").exists());

    harness.success(&["list", "--json"])?;
    assert!(harness.daemon_record_path().exists());

    let both_token_path = harness.home.path().join("received-control-token-both");
    let mut both_server = start_legacy_server(
        &both_token_path,
        &harness.legacy_dir().join("instance.json"),
    )?;
    harness.write_legacy_record(
        both_server.child.id(),
        &both_server.identity,
        both_server.started_at,
        &both_server.origin,
    )?;
    let both_stopped = harness.success(&["daemon", "stop"])?;
    let both_output = String::from_utf8_lossy(&both_stopped.stdout);
    assert!(both_output.contains("daemon stopped"), "{both_output}");
    assert!(
        both_output.contains("legacy WebUI server stopped"),
        "{both_output}"
    );
    assert_eq!(
        std::fs::read_to_string(both_token_path)?,
        "legacy-control-secret"
    );
    assert!(both_server.child.wait()?.success());
    assert!(!harness.daemon_record_path().exists());
    Ok(())
}

#[test]
fn stale_legacy_record_is_cleaned_before_autostart() -> TestResult {
    let harness = Harness::new()?;
    let mut dead = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let dead_pid = dead.id();
    dead.wait()?;
    harness.write_legacy_record(dead_pid, "stale-identity", 1, "http://127.0.0.1:8484/")?;
    std::fs::write(harness.legacy_dir().join("instance.lock"), b"")?;
    std::fs::write(harness.legacy_dir().join("webui.log"), b"stale log")?;

    harness.success(&["list", "--json"])?;
    assert!(harness.daemon_record_path().exists());
    assert!(!harness.legacy_dir().join("instance.json").exists());
    assert!(!harness.legacy_dir().join("instance.lock").exists());
    assert!(!harness.legacy_dir().join("webui.log").exists());
    Ok(())
}

#[test]
#[ignore = "spawned as a real legacy WebUI server process by the integration test"]
fn legacy_server_helper() -> TestResult {
    assert_eq!(
        std::env::var("CRUISE_LEGACY_HELPER").unwrap_or_default(),
        "1"
    );
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let origin = format!("http://{}/", listener.local_addr()?);
    let ready_path = PathBuf::from(
        std::env::var_os("CRUISE_LEGACY_READY_PATH")
            .ok_or_else(|| io::Error::other("ready path is required"))?,
    );
    // The parent polls for this file; rename so it never observes it empty.
    let staging_path = ready_path.with_extension("ready.tmp");
    std::fs::write(&staging_path, origin)?;
    std::fs::rename(&staging_path, &ready_path)?;
    let expected_token = "legacy-control-secret";
    let token_path = std::env::var_os("CRUISE_LEGACY_TOKEN_PATH")
        .ok_or_else(|| io::Error::other("token path is required"))?;

    for connection in listener.incoming() {
        let mut stream = connection?;
        let request = read_request(&mut stream)?;
        if request.path != "/webui/control/stop" {
            respond(&mut stream, 404, "not found")?;
            continue;
        }
        let received = request.control_token.unwrap_or_default();
        std::fs::write(&token_path, &received)?;
        if received != expected_token {
            respond(&mut stream, 403, "invalid control token")?;
            continue;
        }
        respond(&mut stream, 202, "{\"active_operations\":2}")?;
        thread::sleep(Duration::from_millis(250));
        // Like the real v0.3.0 server, remove the owned record before exiting.
        if let Some(record_path) = std::env::var_os("CRUISE_LEGACY_RECORD_PATH") {
            match std::fs::remove_file(record_path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error.into()),
                _ => {}
            }
        }
        return Ok(());
    }
    Ok(())
}

struct Request {
    path: String,
    control_token: Option<String>,
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Request> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let path = line
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let mut control_token = None;
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("X-Cruise-Webui-Control")
        {
            control_token = Some(value.trim().to_owned());
        }
    }
    Ok(Request {
        path,
        control_token,
    })
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = if status == 202 { "Accepted" } else { "Error" };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes())
}

fn process_start_time(pid: u32) -> Option<u64> {
    let mut system = sysinfo::System::new().ok()?;
    let pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
    system.process(pid).and_then(|process| {
        if matches!(
            process.status(),
            sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
        ) {
            None
        } else {
            Some(process.start_time())
        }
    })
}

#[cfg(target_os = "linux")]
fn process_identity(pid: u32) -> Option<String> {
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let ticks: u64 = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()?;
    Some(format!("linux:{}:{ticks}", boot.trim()))
}

#[cfg(target_os = "macos")]
fn process_identity(pid: u32) -> Option<String> {
    let native_pid = i32::try_from(pid).ok()?;
    // SAFETY: proc_pidinfo fills this initialized output struct for the requested PID.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
    // SAFETY: the output pointer is valid for `size` bytes and is properly aligned.
    let result = unsafe {
        libc::proc_pidinfo(
            native_pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    if result != size || info.pbi_pid != pid {
        return None;
    }
    Some(format!(
        "macos:{}:{}",
        info.pbi_start_tvsec, info.pbi_start_tvusec
    ))
}

#[cfg(windows)]
fn process_identity(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: the process handle is closed after its creation time is read.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let success = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        if success == 0 {
            return None;
        }
        Some(format!(
            "windows:{}:{}",
            creation.dwHighDateTime, creation.dwLowDateTime
        ))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn process_identity(_pid: u32) -> Option<String> {
    None
}
