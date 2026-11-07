//! Detached `WebUI` lifecycle management.
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::instance::{InstanceRecord, Registry};
use crate::cli::{ServeArgs, StatusArgs, StopArgs};
use crate::error::{CruiseError, Result};

const WAIT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(50);

fn error(message: impl Into<String>) -> CruiseError {
    CruiseError::Other(message.into())
}

pub(crate) fn start(args: &ServeArgs) -> Result<()> {
    let registry = Registry::new()?;
    let _lock = registry.lock()?;
    start_locked(args)
}

fn start_locked(args: &ServeArgs) -> Result<()> {
    if let Some(record) = Registry::new()?.read_for_start()?
        && record.verify_identity()?
    {
        if args.host.as_ref().is_some_and(|host| host != &record.host)
            || args.port.is_some_and(|port| port != record.port)
        {
            return Err(error(format!(
                "WebUI is already running at {}; use cruise webui restart to change the address",
                record.origin
            )));
        }
        println!(
            "WebUI already running at {} (PID {}, {}, version {}, executable {})",
            record.origin,
            record.pid,
            record.mode,
            record.version,
            record.executable.display()
        );
        if is_outdated(&record) {
            println!("WebUI is outdated; run cruise webui restart");
        }
        if args.open
            && let Err(e) = crate::platform::open_url(&record.origin)
        {
            eprintln!("warning: could not open browser: {e}");
        }
        return Ok(());
    }
    let root = Registry::new()?.dir().to_path_buf();
    fs::create_dir_all(&root)?;
    let log_path = root.join("webui.log");
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
    let port = args.port.unwrap_or(8484);
    let mut child = spawn_daemon(&id, host, port, args.webui_dir.as_deref(), log)?;
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(exit) = child.try_wait()? {
            return Err(error(format!(
                "WebUI daemon exited ({exit}):\n{}",
                log_tail(&log_path)
            )));
        }
        if let Ok(Some(record)) = Registry::new()?.read()
            && record.instance_id == id
            && record.pid == child.id()
            && record.is_live()
        {
            println!(
                "cruise webui started in background (PID {}): {}",
                record.pid, record.origin
            );
            println!("log: {}", log_path.display());
            if args.open
                && let Err(e) = crate::platform::open_url(&record.origin)
            {
                eprintln!("warning: could not open browser: {e}");
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error(format!(
                "timed out starting WebUI daemon:\n{}",
                log_tail(&log_path)
            )));
        }
        thread::sleep(POLL);
    }
}

fn rotate_log(path: &Path) -> Result<()> {
    if fs::metadata(path).is_ok_and(|m| m.len() > 5 * 1024 * 1024) {
        let rotated = path.with_file_name("webui.log.1");
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
    port: u16,
    webui_dir: Option<&Path>,
    log: fs::File,
) -> Result<Child> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args([
        "internal",
        "webui-serve",
        "--instance-id",
        id,
        "--host",
        host,
        "--port",
        &port.to_string(),
    ]);
    if let Some(dir) = webui_dir {
        cmd.arg("--webui-dir").arg(dir);
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
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn()
        .map_err(|e| error(format!("failed to spawn WebUI daemon: {e}")))
}

fn log_tail(path: &Path) -> String {
    let Ok(contents) = fs::read_to_string(path) else {
        return String::new();
    };
    let mut lines: Vec<_> = contents.lines().rev().take(20).collect();
    lines.reverse();
    lines.join("\n")
}

pub(crate) fn stop(args: &StopArgs) -> Result<()> {
    let registry = Registry::new()?;
    let _lock = registry.lock()?;
    stop_locked(&registry, args.force).map(|_| ())
}

fn stop_locked(registry: &Registry, force: bool) -> Result<Option<InstanceRecord>> {
    let Some(record) = registry.read()? else {
        println!("WebUI is not running");
        return Ok(None);
    };
    if !record.verify_identity()? {
        registry.remove_if_owned(&record.instance_id)?;
        println!("WebUI is not running (removed stale record)");
        return Ok(None);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .timeout_connect(Some(Duration::from_secs(5)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    // Unspecified bind addresses are not portable connection destinations.
    // Keep the advertised origin unchanged, but reach a local wildcard listener
    // over its corresponding loopback address.
    let mut origin =
        url::Url::parse(&record.origin).map_err(|e| error(format!("invalid WebUI origin: {e}")))?;
    match origin.host_str() {
        Some("0.0.0.0") => origin.set_host(Some("127.0.0.1")),
        Some("[::]" | "::") => origin.set_host(Some("[::1]")),
        _ => Ok(()),
    }
    .map_err(|e| error(format!("invalid WebUI control address: {e}")))?;
    let url = origin
        .join("webui/control/stop")
        .map_err(|e| error(format!("invalid WebUI control URL: {e}")))?;
    let response = agent
        .post(url.as_str())
        .header("X-Cruise-Webui-Control", &record.control_token)
        .send_empty();
    let mut cancelled = 0u64;
    match response {
        Ok(mut response) if response.status().as_u16() == 202 => {
            let body = response
                .body_mut()
                .read_to_string()
                .map_err(|e| error(format!("invalid WebUI stop response: {e}")))?;
            cancelled = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["active_operations"].as_u64())
                .unwrap_or(0);
        }
        Ok(response) => {
            return Err(error(format!(
                "WebUI stop rejected with HTTP {}",
                response.status()
            )));
        }
        Err(e) => {
            #[cfg(unix)]
            {
                if record.verify_identity()? {
                    signal(&record, libc::SIGTERM)?;
                }
            }
            #[cfg(not(unix))]
            if !force {
                return Err(error(format!(
                    "WebUI stop request failed: {e}; use cruise webui stop --force"
                )));
            }
            #[cfg(unix)]
            eprintln!("warning: WebUI control request failed ({e}); sent SIGTERM");
        }
    }
    let deadline = Instant::now() + WAIT;
    while Instant::now() < deadline {
        if !verify_or_confirm_cleanup(registry, &record, || record.verify_identity())? {
            registry.remove_if_owned(&record.instance_id)?;
            println!("WebUI stopped (cancelled {cancelled} active operation(s))");
            return Ok(Some(record));
        }
        if !matches!(registry.read(), Ok(Some(ref current)) if current.instance_id == record.instance_id)
        {
            println!("WebUI stopped (cancelled {cancelled} active operation(s))");
            return Ok(Some(record));
        }
        thread::sleep(POLL);
    }
    if !force {
        return Err(error("WebUI did not stop; use cruise webui stop --force"));
    }
    if record.verify_identity()? {
        force_kill(&record)?;
    }
    registry.remove_if_owned(&record.instance_id)?;
    println!("WebUI forcibly stopped");
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
fn signal(record: &InstanceRecord, sig: i32) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let pid = i32::try_from(record.pid).map_err(|_| error("invalid WebUI PID"))?;
    if pid <= 0 {
        return Err(error("invalid WebUI PID"));
    }
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let handle =
        unsafe { OwnedFd::from_raw_fd(i32::try_from(fd).map_err(|_| error("invalid pidfd"))?) };
    if !record.verify_identity()? {
        return Err(error("WebUI process identity changed before signaling"));
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
fn signal(record: &InstanceRecord, _sig: i32) -> Result<()> {
    if !record.verify_identity()? {
        return Err(error("WebUI process identity changed before signaling"));
    }
    Err(error(
        "safe process-bound signaling is unavailable on this platform",
    ))
}
#[cfg(unix)]
fn force_kill(record: &InstanceRecord) -> Result<()> {
    signal(record, libc::SIGKILL)
}
#[cfg(windows)]
fn force_kill(record: &InstanceRecord) -> Result<()> {
    if !record.verify_identity()? {
        return Err(error("WebUI process identity changed before termination"));
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
            record.pid,
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
        if record.process_identity.as_deref() != Some(identity.as_str()) {
            CloseHandle(handle);
            return Err(error("WebUI process identity changed before termination"));
        }
        let result = TerminateProcess(handle, 1);
        let error = if result == 0 {
            Some(std::io::Error::last_os_error())
        } else {
            None
        };
        CloseHandle(handle);
        if let Some(error) = error {
            return Err(error.into());
        }
    }
    Ok(())
}

pub(crate) fn restart(mut args: ServeArgs) -> Result<()> {
    let registry = Registry::new()?;
    let _lock = registry.lock()?;
    let previous = registry.read()?;
    stop_locked(&registry, false)?;
    if let Some(previous) = previous {
        if args.host.is_none() {
            args.host = Some(previous.host);
        }
        if args.port.is_none() {
            args.port = Some(previous.port);
        }
    }
    start_locked(&args)
}

pub(crate) fn status(args: &StatusArgs) -> Result<()> {
    let record = Registry::new()?.read()?;
    let running = match record.as_ref() {
        Some(record) => record.verify_identity()?,
        None => false,
    };
    let stale = record.is_some() && !running;
    let outdated = running && record.as_ref().is_some_and(is_outdated);
    if args.json {
        let value = serde_json::json!({"running":running,"stale":stale,"mode":record.as_ref().map(|r| &r.mode),"origin":record.as_ref().map(|r| &r.origin),"pid":record.as_ref().map(|r| r.pid),"version":record.as_ref().map(|r| &r.version),"executable":record.as_ref().map(|r| &r.executable),"started_at":record.as_ref().map(|r| r.started_at),"log_path":record.as_ref().and_then(|r| r.log_path.as_ref()),"outdated":outdated});
        println!("{}", serde_json::to_string(&value)?);
    } else if let Some(r) = record.filter(|_| running) {
        println!(
            "WebUI running at {} (PID {}, {}, version {}, executable {}, started {}, log {})",
            r.origin,
            r.pid,
            r.mode,
            r.version,
            r.executable.display(),
            r.started_at,
            r.log_path
                .as_ref()
                .map_or_else(|| "none".to_owned(), |p| p.display().to_string())
        );
        if outdated {
            println!("WebUI is outdated; run cruise webui restart");
        }
    } else {
        println!(
            "WebUI is not running{}",
            if stale { " (stale record)" } else { "" }
        );
    }
    if running {
        Ok(())
    } else {
        Err(error("WebUI is not running"))
    }
}

pub(crate) fn restart_hint() {
    if let Ok(Some(r)) = Registry::new().and_then(|registry| registry.read())
        && r.is_live()
    {
        let _ = writeln!(
            std::io::stderr(),
            "warning: the WebUI (PID {}) is still running the previous version; run cruise webui restart to load the updated runtime",
            r.pid
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        child.wait()?;
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
        let path = temp.path().join("webui.log");
        fs::write(&path, "")?;
        assert_eq!(log_tail(&path), "");
        fs::write(&path, "first\nsecond\n")?;
        assert_eq!(log_tail(&path), "first\nsecond");
        let lines: Vec<_> = (0..25).map(|n| format!("line {n}")).collect();
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
            control_token: "0".repeat(64),
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
        assert!(stop_locked(&registry, true)?.is_none());
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
        assert!(stop_locked(&registry, true).is_err());
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
                let _ = self.0.wait();
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
        assert!(!child.0.wait()?.success());
        Ok(())
    }

    #[test]
    fn rotation_keeps_one_generation_above_threshold() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let log = temp.path().join("webui.log");
        let rotated = temp.path().join("webui.log.1");
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
