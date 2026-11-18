//! Detached daemon lifecycle management.
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::instance::{InstanceRecord, Registry};
use crate::daemon::{ServeOptions, StatusOptions, StopOptions};
use crate::error::{CruiseError, Result};

const WAIT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(50);

fn error(message: impl Into<String>) -> CruiseError {
    CruiseError::Other(message.into())
}

pub(crate) fn start(args: &ServeOptions) -> Result<()> {
    start_with_env(args, false, true)
}

pub(crate) fn start_autostart(args: &ServeOptions) -> Result<()> {
    start_with_env(args, true, false)
}

fn start_with_env(args: &ServeOptions, strip_workflow_env: bool, announce: bool) -> Result<()> {
    crate::daemon::legacy::ensure_stopped_before_start_at_state_dir()?;
    let registry = Registry::new()?;
    let _lock = registry.lock()?;
    let requested_port = requested_port(args)?;
    let port_explicit = args.port_explicit.unwrap_or(requested_port.is_some());
    start_locked(
        args,
        &registry,
        requested_port,
        port_explicit,
        strip_workflow_env,
        announce,
    )
}

fn requested_port(args: &ServeOptions) -> Result<Option<u16>> {
    if let Some(port) = args.port {
        return Ok(Some(port));
    }
    std::env::var("CRUISE_DAEMON_PORT")
        .ok()
        .map(|value| {
            value.parse::<u16>().map_err(|error| {
                CruiseError::Other(format!("invalid CRUISE_DAEMON_PORT '{value}': {error}"))
            })
        })
        .transpose()
}

fn start_locked(
    args: &ServeOptions,
    registry: &Registry,
    requested_port: Option<u16>,
    port_explicit: bool,
    strip_workflow_env: bool,
    announce: bool,
) -> Result<()> {
    if let Some(record) = registry.read_for_start()?
        && record.verify_identity()?
    {
        if args.host.as_ref().is_some_and(|host| host != &record.host)
            || requested_port.is_some_and(|port| port != record.port)
        {
            return Err(error(format!(
                "daemon is already running at {}; use cruise daemon restart to change the address",
                record.origin
            )));
        }
        if announce {
            println!(
                "daemon already running at {} (PID {}, version {}, executable {})",
                record.origin,
                record.pid,
                record.version,
                record.executable.display()
            );
            if is_outdated(&record) {
                println!("daemon is outdated; run cruise daemon restart");
            }
        }
        announce_webui_login(&record, announce, args.open)?;
        return Ok(());
    }
    let root = registry.dir().to_path_buf();
    fs::create_dir_all(&root)?;
    let log_path = registry.log_path();
    rotate_log(&log_path)?;
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let log = options.open(&log_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        log.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    crate::platform::set_owner_only_acl(&log_path)?;
    let id = uuid::Uuid::new_v4().to_string();
    let host = args.host.as_deref().unwrap_or("127.0.0.1");
    let mut child = spawn_daemon(
        &id,
        host,
        requested_port,
        port_explicit,
        args.webui_dir.as_deref(),
        log,
        strip_workflow_env,
    )?;
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(exit) = child.try_wait()? {
            return Err(error(format!(
                "daemon exited ({exit}):\n{}",
                log_tail(&log_path)
            )));
        }
        if let Ok(Some(record)) = registry.read()
            && record.instance_id == id
            && record.pid == child.id()
            && record.is_live()
        {
            if requested_port.is_none() && record.port != 8484 {
                eprintln!(
                    "warning: default daemon port 8484 is busy; using ephemeral port. Set --port or CRUISE_DAEMON_PORT explicitly on shared hosts."
                );
            }
            if announce {
                println!(
                    "cruise daemon started in background (PID {}): {}",
                    record.pid, record.origin
                );
                println!("log: {}", log_path.display());
            }
            announce_webui_login(&record, announce, args.open)?;
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error(format!(
                "timed out starting daemon:\n{}",
                log_tail(&log_path)
            )));
        }
        thread::sleep(POLL);
    }
}

fn announce_webui_login(record: &InstanceRecord, announce: bool, open: bool) -> Result<()> {
    if !announce && !open {
        return Ok(());
    }
    let login_url = login_url_from_api(record)?;
    if announce {
        println!("cruise daemon WebUI login: {login_url}");
    }
    if open && let Err(error) = crate::platform::open_login_redirect(&login_url) {
        eprintln!("warning: could not open browser: {error}");
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct LoginNonceResponse {
    url: String,
}

fn login_url_from_api(record: &InstanceRecord) -> Result<String> {
    let origin = url::Url::parse(&record.origin)
        .map_err(|parse_error| error(format!("invalid daemon origin: {parse_error}")))?;
    let url = origin
        .join("api/v1/login-nonce")
        .map_err(|url_error| error(format!("invalid daemon login nonce URL: {url_error}")))?;
    let agent = control_agent();
    let deadline = Instant::now() + WAIT;
    loop {
        let response = agent
            .post(url.as_str())
            .header("Authorization", &format!("Bearer {}", record.token))
            .send_empty();
        match response {
            Ok(mut response) => {
                if response.status().as_u16() != 200 {
                    return Err(error(format!(
                        "daemon login nonce request rejected with HTTP {}",
                        response.status()
                    )));
                }
                let body = response.body_mut().read_to_string().map_err(|read_error| {
                    error(format!("invalid daemon login nonce response: {read_error}"))
                })?;
                return serde_json::from_str::<LoginNonceResponse>(&body)
                    .map(|response| response.url)
                    .map_err(|parse_error| {
                        error(format!(
                            "invalid daemon login nonce response: {parse_error}"
                        ))
                    });
            }
            Err(_) if Instant::now() < deadline => {
                thread::sleep(POLL);
            }
            Err(request_error) => {
                return Err(error(format!(
                    "daemon login nonce request failed: {request_error}"
                )));
            }
        }
    }
}

fn control_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(5)))
        .timeout_connect(Some(Duration::from_secs(5)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into()
}

fn rotate_log(path: &Path) -> Result<()> {
    if fs::metadata(path).is_ok_and(|metadata| metadata.len() > 5 * 1024 * 1024) {
        let rotated = path.with_file_name("daemon.log.1");
        if rotated.exists() {
            fs::remove_file(&rotated)?;
        }
        fs::rename(path, rotated)?;
    }
    Ok(())
}

fn is_outdated(record: &InstanceRecord) -> bool {
    record.version != env!("CARGO_PKG_VERSION")
}

fn spawn_daemon(
    id: &str,
    host: &str,
    port: Option<u16>,
    port_explicit: bool,
    webui_dir: Option<&Path>,
    log: fs::File,
    strip_workflow_env: bool,
) -> Result<Child> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args([
        "internal",
        "daemon-serve",
        "--instance-id",
        id,
        "--host",
        host,
    ]);
    if let Some(port) = port {
        cmd.arg("--port").arg(port.to_string());
        if !port_explicit {
            cmd.arg("--port-implicit");
        }
    }
    if let Some(dir) = webui_dir {
        cmd.arg("--webui-dir").arg(dir);
    }
    if strip_workflow_env {
        for key in crate::config_env::WORKFLOW_ENV_KEYS {
            cmd.env_remove(key.var);
        }
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // SAFETY: setsid is async-signal-safe and does not access shared Rust state.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::Foundation::{
            HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation,
        };
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        // The launcher clears its original captured-pipe handles before
        // launching this runtime. Clear the std-handle copies inherited here;
        // Command duplicates the configured log handles for the daemon.
        for handle in [
            std::io::stdin().as_raw_handle(),
            std::io::stdout().as_raw_handle(),
            std::io::stderr().as_raw_handle(),
        ] {
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                // SAFETY: these are borrowed standard handles, and changing the
                // inheritance flag does not close or invalidate them.
                if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
        }
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn()
        .map_err(|spawn_error| error(format!("failed to spawn daemon: {spawn_error}")))
}

fn log_tail(path: &Path) -> String {
    let Ok(contents) = fs::read_to_string(path) else {
        return String::new();
    };
    let mut lines: Vec<_> = contents.lines().rev().take(20).collect();
    lines.reverse();
    lines.join("\n")
}

pub(crate) fn stop(args: StopOptions) -> Result<()> {
    let registry = Registry::new()?;
    let _lock = registry.lock()?;
    let daemon_result = stop_locked(&registry, args.force, true).map(|_| ());
    let legacy_result = crate::daemon::legacy::stop(args.force).map(|_| ());
    daemon_result.and(legacy_result)
}

fn stop_locked(registry: &Registry, force: bool, announce: bool) -> Result<Option<InstanceRecord>> {
    let Some(record) = registry.read()? else {
        if announce {
            println!("daemon is not running");
        }
        return Ok(None);
    };
    if !record.verify_identity()? {
        registry.remove_if_owned(&record.instance_id)?;
        if announce {
            println!("daemon is not running (removed stale record)");
        }
        return Ok(None);
    }
    let agent = control_agent();
    let mut origin = url::Url::parse(&record.origin)
        .map_err(|parse_error| error(format!("invalid daemon origin: {parse_error}")))?;
    match origin.host_str() {
        Some("0.0.0.0") => origin.set_host(Some("127.0.0.1")),
        Some("[::]" | "::") => origin.set_host(Some("[::1]")),
        _ => Ok(()),
    }
    .map_err(|address_error| error(format!("invalid daemon control address: {address_error}")))?;
    let url = origin
        .join("api/v1/shutdown")
        .map_err(|url_error| error(format!("invalid daemon shutdown URL: {url_error}")))?;
    let response = agent
        .post(url.as_str())
        .header("Authorization", &format!("Bearer {}", record.token))
        .send_empty();
    let mut cancelled = 0u64;
    match response {
        Ok(mut response) if response.status().as_u16() == 202 => {
            let body = response.body_mut().read_to_string().map_err(|read_error| {
                error(format!("invalid daemon shutdown response: {read_error}"))
            })?;
            cancelled = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| value["activeOperations"].as_u64())
                .unwrap_or(0);
        }
        Ok(response) => {
            return Err(error(format!(
                "daemon shutdown rejected with HTTP {}",
                response.status()
            )));
        }
        Err(request_error) => {
            #[cfg(unix)]
            {
                if record.verify_identity()? {
                    signal(&record, libc::SIGTERM)?;
                }
            }
            #[cfg(not(unix))]
            if !force {
                return Err(error(format!(
                    "daemon shutdown request failed: {request_error}; use cruise daemon stop --force"
                )));
            }
            if announce {
                eprintln!("warning: daemon control request failed ({request_error}); sent SIGTERM");
            }
        }
    }
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if !verify_or_confirm_cleanup(registry, &record, || record.verify_identity())? {
            registry.remove_if_owned(&record.instance_id)?;
            if announce {
                println!("daemon stopped (cancelled {cancelled} active operation(s))");
            }
            return Ok(Some(record));
        }
        if !matches!(registry.read(), Ok(Some(ref current)) if current.instance_id == record.instance_id)
        {
            if announce {
                println!("daemon stopped (cancelled {cancelled} active operation(s))");
            }
            return Ok(Some(record));
        }
        thread::sleep(POLL);
    }
    if !force {
        return Err(error("daemon did not stop; use cruise daemon stop --force"));
    }
    if record.verify_identity()? {
        force_kill(&record)?;
    }
    registry.remove_if_owned(&record.instance_id)?;
    if announce {
        println!("daemon forcibly stopped");
    }
    Ok(Some(record))
}

// A shutting-down daemon removes its record before exiting. On macOS it can
// become uninspectable while still visible to kill(pid, 0), so recheck the
// registry after an identity inspection error rather than treating that error
// as evidence that an extant record is safe to remove.
fn verify_or_confirm_cleanup(
    registry: &Registry,
    record: &InstanceRecord,
    verify: impl FnOnce() -> Result<bool>,
) -> Result<bool> {
    match verify() {
        Ok(live) => Ok(live),
        Err(error) => match registry.read()? {
            Some(current) if current.instance_id == record.instance_id => Err(error),
            _ => Ok(false),
        },
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn signal<T: crate::daemon::instance::ProcessTarget>(
    record: &T,
    sig: i32,
) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let pid = i32::try_from(record.pid()).map_err(|_| error("invalid daemon PID"))?;
    if pid <= 0 {
        return Err(error("invalid daemon PID"));
    }
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let handle =
        unsafe { OwnedFd::from_raw_fd(i32::try_from(fd).map_err(|_| error("invalid pidfd"))?) };
    if !record.verify_identity()? {
        return Err(error(format!(
            "{} process identity changed before signaling",
            record.process_label()
        )));
    }
    signal_handle(handle.as_raw_fd(), sig)
}

#[cfg(target_os = "linux")]
fn signal_handle(handle: std::os::fd::RawFd, sig: i32) -> Result<()> {
    if unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            handle,
            sig,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    } != 0
    {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ESRCH) {
            return Err(e.into());
        }
    }
    Ok(())
}
#[cfg(all(unix, not(target_os = "linux")))]
pub(crate) fn signal<T: crate::daemon::instance::ProcessTarget>(
    record: &T,
    _sig: i32,
) -> Result<()> {
    if !record.verify_identity()? {
        return Err(error(format!(
            "{} process identity changed before signaling",
            record.process_label()
        )));
    }
    Err(error(format!(
        "safe process-bound signaling is unavailable for PID {} on this platform",
        record.pid()
    )))
}

#[cfg(unix)]
pub(crate) fn force_kill<T: crate::daemon::instance::ProcessTarget>(record: &T) -> Result<()> {
    signal(record, libc::SIGKILL)
}

#[cfg(windows)]
pub(crate) fn force_kill<T: crate::daemon::instance::ProcessTarget>(record: &T) -> Result<()> {
    if !record.verify_identity()? {
        return Err(error(format!(
            "{} process identity changed before termination",
            record.process_label()
        )));
    }
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        TerminateProcess,
    };
    // SAFETY: the handle returned by OpenProcess is used only for termination and closed once.
    unsafe {
        let handle = OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            record.pid(),
        );
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) == 0 {
            let error = std::io::Error::last_os_error();
            CloseHandle(handle);
            return Err(error.into());
        }
        let identity = format!(
            "windows:{}:{}",
            creation.dwHighDateTime, creation.dwLowDateTime
        );
        if record.expected_process_identity() != Some(identity.as_str()) {
            CloseHandle(handle);
            return Err(error(format!(
                "{} process identity changed before termination",
                record.process_label()
            )));
        }
        let result = TerminateProcess(handle, 1);
        let error = (result == 0).then(std::io::Error::last_os_error);
        CloseHandle(handle);
        if let Some(error) = error {
            return Err(error.into());
        }
    }
    Ok(())
}

pub(crate) fn restart(mut args: ServeOptions) -> Result<()> {
    crate::daemon::legacy::ensure_stopped_before_start_at_state_dir()?;
    let registry = Registry::new()?;
    let _lock = registry.lock()?;
    let previous = registry.read()?;
    let mut requested_port = requested_port(&args)?;
    let mut port_explicit = args.port_explicit.unwrap_or(requested_port.is_some());
    stop_locked(&registry, false, true)?;
    if let Some(previous) = previous {
        if args.host.is_none() {
            args.host = Some(previous.host.clone());
        }
        if requested_port.is_none() && previous.port_explicit {
            requested_port = Some(previous.port);
            port_explicit = true;
        }
    }
    start_locked(&args, &registry, requested_port, port_explicit, false, true)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DaemonStatus {
    version: String,
    active_operations: usize,
    batch_active: bool,
}

fn read_daemon_status(record: &InstanceRecord) -> Result<DaemonStatus> {
    let origin = url::Url::parse(&record.origin)
        .map_err(|parse_error| error(format!("invalid daemon origin: {parse_error}")))?;
    let url = origin
        .join("api/v1/status")
        .map_err(|url_error| error(format!("invalid daemon status URL: {url_error}")))?;
    let mut response = control_agent()
        .get(url.as_str())
        .header("Authorization", &format!("Bearer {}", record.token))
        .call()
        .map_err(|request_error| error(format!("daemon status request failed: {request_error}")))?;
    if response.status().as_u16() != 200 {
        return Err(error(format!(
            "daemon status request rejected with HTTP {}",
            response.status()
        )));
    }
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|read_error| error(format!("invalid daemon status response: {read_error}")))?;
    serde_json::from_str(&body)
        .map_err(|parse_error| error(format!("invalid daemon status response: {parse_error}")))
}

/// Restart only the same live daemon after verifying it is still outdated and idle under lock.
pub(crate) fn restart_outdated_autostart(
    mut args: ServeOptions,
    expected_instance_id: &str,
) -> Result<bool> {
    crate::daemon::legacy::ensure_stopped_before_start_at_state_dir()?;
    let registry = Registry::new()?;
    let _lock = registry.lock()?;
    let Some(previous) = registry.read()? else {
        return Ok(false);
    };
    if previous.instance_id != expected_instance_id || !previous.verify_identity()? {
        return Ok(false);
    }
    let status = read_daemon_status(&previous)?;
    if status.version == env!("CARGO_PKG_VERSION") {
        return Ok(false);
    }
    if status.active_operations != 0 || status.batch_active {
        return Err(error(format!(
            "daemon is running cruise {} with active work; run 'cruise daemon restart' when idle",
            status.version
        )));
    }
    let mut requested_port = requested_port(&args)?;
    let mut port_explicit = args.port_explicit.unwrap_or(requested_port.is_some());
    if args.host.is_none() {
        args.host = Some(previous.host.clone());
    }
    if requested_port.is_none() && previous.port_explicit {
        requested_port = Some(previous.port);
        port_explicit = true;
    }
    stop_locked(&registry, false, false)?;
    start_locked(&args, &registry, requested_port, port_explicit, true, false)?;
    Ok(true)
}

pub(crate) fn status(args: StatusOptions) -> Result<()> {
    let record = Registry::new()?.read()?;
    let running = match record.as_ref() {
        Some(record) => record.verify_identity()?,
        None => false,
    };
    let stale = record.is_some() && !running;
    let outdated = running && record.as_ref().is_some_and(is_outdated);
    let legacy_record = crate::daemon::legacy::Registry::new()?.read()?;
    let legacy_running = match legacy_record.as_ref() {
        Some(record) => record.verify_identity()?,
        None => false,
    };
    let legacy_stale = legacy_record.is_some() && !legacy_running;
    if args.json {
        let value = serde_json::json!({
            "running": running,
            "stale": stale,
            "mode": record.as_ref().map(|record| &record.mode),
            "origin": record.as_ref().map(|record| &record.origin),
            "host": record.as_ref().map(|record| &record.host),
            "port": record.as_ref().map(|record| record.port),
            "pid": record.as_ref().map(|record| record.pid),
            "version": record.as_ref().map(|record| &record.version),
            "executable": record.as_ref().map(|record| &record.executable),
            "started_at": record.as_ref().map(|record| record.started_at),
            "log_path": record.as_ref().and_then(|record| record.log_path.as_ref()),
            "outdated": outdated,
            "legacy_webui": {
                "running": legacy_running,
                "stale": legacy_stale,
                "origin": legacy_record.as_ref().map(|record| &record.origin),
                "pid": legacy_record.as_ref().map(|record| record.pid),
                "version": legacy_record.as_ref().map(|record| &record.version)
            }
        });
        println!("{}", serde_json::to_string(&value)?);
    } else if let Some(record) = record.filter(|_| running) {
        println!(
            "daemon running at {} (PID {}, version {}, executable {}, started {}, log {})",
            record.origin,
            record.pid,
            record.version,
            record.executable.display(),
            record.started_at,
            record
                .log_path
                .as_ref()
                .map_or_else(|| "none".to_owned(), |path| path.display().to_string())
        );
        if outdated {
            println!("daemon is outdated; run cruise daemon restart");
        }
    } else {
        println!(
            "daemon is not running{}",
            if stale { " (stale record)" } else { "" }
        );
    }
    if !args.json {
        if let Some(record) = legacy_record.as_ref().filter(|_| legacy_running) {
            println!(
                "legacy WebUI server running at {} (PID {}, version {})",
                record.origin, record.pid, record.version
            );
        } else if legacy_stale {
            println!("legacy WebUI server has a stale record");
        }
    }
    if running {
        Ok(())
    } else if legacy_running {
        let record = legacy_record
            .as_ref()
            .ok_or_else(|| error("legacy WebUI record disappeared"))?;
        Err(error(format!(
            "daemon is not running; legacy WebUI server is running at {} (PID {}, version {})",
            record.origin, record.pid, record.version
        )))
    } else {
        Err(error("daemon is not running"))
    }
}

pub(crate) fn restart_hint() {
    if let Ok(Some(r)) = Registry::new().and_then(|registry| registry.read())
        && r.is_live()
    {
        let _ = writeln!(
            std::io::stderr(),
            "warning: the daemon (PID {}) is still running the previous version; run cruise daemon restart to load the updated runtime",
            r.pid
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", windows))]
    fn wait_for_test_child(
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
                    thread::sleep(POLL);
                }
                return Err(timeout);
            }
            thread::sleep(POLL);
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pidfd_signal_after_process_exit_does_not_target_replacement() -> Result<()> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let mut child = Command::new("sleep").arg("30").spawn()?;
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id(), 0) };
        assert!(fd >= 0, "pidfd_open: {}", std::io::Error::last_os_error());
        let handle =
            unsafe { OwnedFd::from_raw_fd(i32::try_from(fd).map_err(|_| error("invalid pidfd"))?) };
        let mut target = record();
        target.pid = child.id();
        target.process_started_at = super::super::instance::process_start_time(target.pid)
            .ok_or_else(|| error("target process must be inspectable"))?;
        target.process_identity = super::super::instance::precise_identity(target.pid);
        assert!(target.verify_identity()?);
        child.kill()?;
        wait_for_test_child(&mut child, "pidfd test child")?;
        signal_handle(handle.as_raw_fd(), libc::SIGKILL)?;
        Ok(())
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    #[test]
    fn unsupported_platform_refuses_pid_signaling() {
        assert!(signal(&record(), libc::SIGTERM).is_err());
    }

    #[test]
    fn log_tail_handles_empty_short_and_long_logs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("daemon.log");
        fs::write(&path, "")?;
        assert_eq!(log_tail(&path), "");
        fs::write(&path, "first\nsecond\n")?;
        assert_eq!(log_tail(&path), "first\nsecond");
        let lines: Vec<_> = (0..25).map(|number| format!("line {number}")).collect();
        fs::write(&path, lines.join("\n"))?;
        assert_eq!(log_tail(&path), lines[5..].join("\n"));
        Ok(())
    }

    fn record() -> InstanceRecord {
        let pid = std::process::id();
        InstanceRecord {
            schema_version: 1,
            instance_id: "test-instance".to_owned(),
            mode: "daemon".to_owned(),
            pid,
            process_started_at: super::super::instance::process_start_time(pid)
                .unwrap_or_else(|| panic!("current process must be inspectable")),
            process_identity: super::super::instance::precise_identity(pid),
            host: "127.0.0.1".to_owned(),
            port: 8484,
            origin: "http://127.0.0.1:8484/".to_owned(),
            token: "0".repeat(64),
            port_explicit: true,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            executable: "cruise".into(),
            working_dir: ".".into(),
            log_path: None,
            started_at: 0,
        }
    }

    #[test]
    fn shutdown_identity_error_requires_confirmed_registry_cleanup() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let registry = Registry::at(temp.path().to_path_buf());
        let target = record();
        registry.write(&target)?;

        assert!(
            verify_or_confirm_cleanup(&registry, &target, || {
                Err(error("identity unavailable"))
            })
            .is_err()
        );
        assert!(registry.read()?.is_some());

        assert!(!verify_or_confirm_cleanup(&registry, &target, || {
            registry.remove_if_owned(&target.instance_id)?;
            Err(error("identity unavailable"))
        })?);
        assert!(registry.read()?.is_none());
        Ok(())
    }

    #[test]
    fn stop_force_does_not_signal_reused_pid() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let registry = Registry::at(temp.path().to_path_buf());
        let mut stale = record();
        stale.process_identity = Some("different-identity".to_owned());
        registry.write(&stale)?;
        assert!(!stale.is_live());
        assert!(stop_locked(&registry, true, true)?.is_none());
        assert!(registry.read()?.is_none());
        assert!(record().is_live());
        Ok(())
    }

    #[test]
    fn legacy_record_is_retained_without_signaling() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let registry = Registry::at(temp.path().to_path_buf());
        let mut legacy = record();
        legacy.process_identity = None;
        registry.write(&legacy)?;
        assert!(stop_locked(&registry, true, true).is_err());
        assert!(registry.read()?.is_some());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_force_termination_checks_process_identity() -> Result<()> {
        struct ChildGuard(Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = wait_for_test_child(&mut self.0, "Windows test child cleanup");
            }
        }
        let mut child = ChildGuard(
            Command::new("ping")
                .args(["127.0.0.1", "-n", "30"])
                .spawn()?,
        );
        let mut target = record();
        target.pid = child.0.id();
        target.process_started_at = super::super::instance::process_start_time(target.pid)
            .ok_or_else(|| error("target process must be inspectable"))?;
        target.process_identity = super::super::instance::precise_identity(target.pid);
        assert!(target.process_identity.is_some());
        assert!(target.verify_identity()?);
        let mut mismatched = target.clone();
        mismatched.process_identity = Some("windows:wrong".into());
        assert!(force_kill(&mismatched).is_err());
        assert!(child.0.try_wait()?.is_none());
        let mut unverifiable = target.clone();
        unverifiable.process_identity = None;
        assert!(force_kill(&unverifiable).is_err());
        assert!(child.0.try_wait()?.is_none());
        force_kill(&target)?;
        let status = wait_for_test_child(&mut child.0, "Windows force-termination test child")?;
        assert!(!status.success());
        Ok(())
    }

    #[test]
    fn rotation_keeps_one_generation_above_threshold() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let log = temp.path().join("daemon.log");
        let rotated = temp.path().join("daemon.log.1");
        let file = fs::File::create(&log)?;
        file.set_len(5 * 1024 * 1024)?;
        rotate_log(&log)?;
        assert!(log.exists());
        assert!(!rotated.exists());
        file.set_len(5 * 1024 * 1024 + 1)?;
        drop(file);
        fs::write(&rotated, b"old generation")?;
        rotate_log(&log)?;
        assert!(!log.exists());
        assert_eq!(fs::metadata(rotated)?.len(), 5 * 1024 * 1024 + 1);
        Ok(())
    }

    #[test]
    fn version_comparison_uses_running_binary_version() {
        let mut current = record();
        assert!(!is_outdated(&current));
        current.version = "other-version".to_owned();
        assert!(is_outdated(&current));
    }
}
