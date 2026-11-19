//! Detects and stops the detached `WebUI` service released before the daemon.
use std::fs;
use std::io;
use std::net::IpAddr;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::daemon::instance::{ProcessTarget, verify_process_identity};
use crate::error::{CruiseError, Result};

const WAIT: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(50);

fn error(message: impl Into<String>) -> CruiseError {
    CruiseError::Other(message.into())
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyRecord {
    pub(crate) schema_version: u32,
    pub(crate) instance_id: String,
    pub(crate) mode: String,
    pub(crate) pid: u32,
    pub(crate) process_started_at: u64,
    #[serde(default)]
    pub(crate) process_identity: Option<String>,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) origin: String,
    pub(crate) control_token: String,
    pub(crate) version: String,
    pub(crate) executable: PathBuf,
    pub(crate) working_dir: PathBuf,
    #[expect(
        dead_code,
        reason = "Retained to parse the released registry shape without deleting arbitrary paths."
    )]
    pub(crate) log_path: Option<PathBuf>,
    pub(crate) started_at: u64,
}

impl LegacyRecord {
    pub(crate) fn verify_identity(&self) -> Result<bool> {
        verify_process_identity(
            self.pid,
            self.process_started_at,
            self.process_identity.as_deref(),
            "legacy WebUI record",
            "legacy WebUI",
        )
    }
}

impl ProcessTarget for LegacyRecord {
    fn pid(&self) -> u32 {
        self.pid
    }

    #[cfg(windows)]
    fn expected_process_identity(&self) -> Option<&str> {
        self.process_identity.as_deref()
    }

    fn verify_identity(&self) -> Result<bool> {
        LegacyRecord::verify_identity(self)
    }

    fn process_label(&self) -> &'static str {
        "legacy WebUI"
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Registry {
    dir: PathBuf,
}

impl Registry {
    pub(crate) fn new() -> Result<Self> {
        Ok(Self::at(crate::paths::state_dir()?.join("webui")))
    }

    #[must_use]
    pub(crate) fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    #[must_use]
    pub(crate) fn record_path(&self) -> PathBuf {
        self.dir.join("instance.json")
    }

    pub(crate) fn read(&self) -> Result<Option<LegacyRecord>> {
        let path = self.record_path();
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(io_error) if io_error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(io_error) => {
                return Err(error(format!(
                    "cannot inspect legacy WebUI instance record: {io_error}"
                )));
            }
        };
        if !metadata.is_file() {
            return Err(error("legacy WebUI instance record is not a regular file"));
        }
        let bytes = fs::read(path).map_err(|io_error| {
            error(format!(
                "cannot read legacy WebUI instance record: {io_error}"
            ))
        })?;
        let record: LegacyRecord = serde_json::from_slice(&bytes).map_err(|parse_error| {
            error(format!(
                "invalid legacy WebUI instance record: {parse_error}"
            ))
        })?;
        if record.schema_version != 1 {
            return Err(error(format!(
                "unsupported legacy WebUI instance schema version: {}",
                record.schema_version
            )));
        }
        if !matches!(record.mode.as_str(), "foreground" | "daemon")
            || record.pid == 0
            || record.instance_id.is_empty()
            || record.control_token.is_empty()
            || record.host.is_empty()
            || record.port == 0
            || record.origin.is_empty()
            || record.version.is_empty()
            || record.executable.as_os_str().is_empty()
            || record.working_dir.as_os_str().is_empty()
            || record.started_at == 0
        {
            return Err(error("invalid legacy WebUI instance record"));
        }
        Ok(Some(record))
    }

    pub(crate) fn read_for_start(&self) -> Result<Option<LegacyRecord>> {
        self.read().map_err(|error| {
            CruiseError::Other(format!(
                "cannot safely start daemon: {error}; inspect the legacy WebUI record at {}",
                self.record_path().display()
            ))
        })
    }

    pub(crate) fn remove_stale_if_owned(&self, record: &LegacyRecord) -> Result<()> {
        let Some(current) = self
            .read()?
            .filter(|current| current.instance_id == record.instance_id)
        else {
            return Ok(());
        };
        if current.verify_identity()? {
            return Ok(());
        }
        let _ = fs::remove_file(self.record_path());
        let _ = fs::remove_file(self.dir.join("instance.lock"));
        let _ = fs::remove_file(self.dir.join("webui.log"));
        Ok(())
    }

    /// Remove the legacy registry files after `record`'s server exited. The
    /// server deletes its own record on exit, so an absent record still owns
    /// the leftover lock and log; a record from another instance is kept.
    fn remove_if_owned(&self, record: &LegacyRecord) -> Result<()> {
        match self.read()? {
            Some(current) if current.instance_id != record.instance_id => return Ok(()),
            Some(_) => fs::remove_file(self.record_path()).or_else(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    Ok(())
                } else {
                    Err(error)
                }
            })?,
            None => {}
        }
        let _ = fs::remove_file(self.dir.join("instance.lock"));
        let _ = fs::remove_file(self.dir.join("webui.log"));
        Ok(())
    }
}

pub(crate) fn ensure_stopped_before_start(registry: &Registry) -> Result<()> {
    let Some(record) = registry.read_for_start()? else {
        return Ok(());
    };
    if record.verify_identity()? {
        return Err(error(format!(
            "legacy WebUI server is running (PID {}, version {}, origin {}); stop it with 'cruise daemon stop' before starting the daemon. This cancels the legacy server's running work, leaving sessions Suspended; resume them with 'cruise run <id>'.",
            record.pid, record.version, record.origin
        )));
    }
    registry.remove_stale_if_owned(&record)?;
    Ok(())
}

pub(crate) fn ensure_stopped_before_start_at_state_dir() -> Result<()> {
    ensure_stopped_before_start(&Registry::new()?)
}

pub(crate) fn stop(force: bool) -> Result<bool> {
    stop_with_registry(&Registry::new()?, force)
}

fn stop_with_registry(registry: &Registry, force: bool) -> Result<bool> {
    let Some(record) = registry.read()? else {
        return Ok(false);
    };
    if !record.verify_identity()? {
        registry.remove_stale_if_owned(&record)?;
        return Ok(false);
    }

    let cancelled = match request_stop(&record) {
        Ok(active_operations) => active_operations,
        Err(request_error) => {
            #[cfg(target_os = "linux")]
            {
                crate::daemon::lifecycle::signal(&record, libc::SIGTERM)?;
                eprintln!(
                    "warning: legacy WebUI control request failed ({request_error}); attempting SIGTERM"
                );
                0
            }
            #[cfg(all(unix, not(target_os = "linux")))]
            {
                return Err(error(format!(
                    "legacy WebUI control request failed for PID {}: {request_error}; stop the process manually after verifying its identity",
                    record.pid
                )));
            }
            #[cfg(windows)]
            {
                if !force {
                    return Err(error(format!(
                        "legacy WebUI shutdown request failed for PID {}: {request_error}; use cruise daemon stop --force",
                        record.pid
                    )));
                }
                crate::daemon::lifecycle::force_kill(&record)?;
                0
            }
            #[cfg(not(any(unix, windows)))]
            {
                return Err(error(format!(
                    "legacy WebUI shutdown request failed for PID {}: {request_error}",
                    record.pid
                )));
            }
        }
    };

    if wait_for_exit(registry, &record)? {
        registry.remove_if_owned(&record)?;
        println!("legacy WebUI server stopped (cancelled {cancelled} active operation(s))");
        return Ok(true);
    }
    if !force {
        #[cfg(all(unix, not(target_os = "linux")))]
        return Err(error(format!(
            "legacy WebUI server did not stop (PID {}); stop the process manually after verifying its identity",
            record.pid
        )));
        #[cfg(any(target_os = "linux", windows))]
        return Err(error(
            "legacy WebUI server did not stop; use cruise daemon stop --force",
        ));
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    return Err(error(format!(
        "legacy WebUI server is still running (PID {}); stop it manually after verifying its identity",
        record.pid
    )));
    #[cfg(any(target_os = "linux", windows))]
    {
        if record.verify_identity()? {
            crate::daemon::lifecycle::force_kill(&record)?;
        }
        if !wait_for_exit(registry, &record)? {
            return Err(error(
                "legacy WebUI server did not exit after forced termination",
            ));
        }
        registry.remove_if_owned(&record)?;
        println!("legacy WebUI server forcibly stopped");
        Ok(true)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(error("legacy WebUI server did not stop"))
    }
}

fn request_stop(record: &LegacyRecord) -> Result<u64> {
    let url = control_url(record)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(5)))
        .timeout_connect(Some(Duration::from_secs(5)))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .post(url.as_str())
        .header("X-Cruise-Webui-Control", &record.control_token)
        .send_empty()
        .map_err(|request_error| {
            error(format!(
                "legacy WebUI shutdown request failed: {request_error}"
            ))
        })?;
    if response.status().as_u16() != 202 {
        return Err(error(format!(
            "legacy WebUI shutdown rejected with HTTP {}",
            response.status()
        )));
    }
    let body = response.body_mut().read_to_string().map_err(|read_error| {
        error(format!(
            "invalid legacy WebUI shutdown response: {read_error}"
        ))
    })?;
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value["active_operations"]
                .as_u64()
                .or_else(|| value["activeOperations"].as_u64())
        })
        .ok_or_else(|| error("invalid legacy WebUI shutdown response"))
}

fn control_url(record: &LegacyRecord) -> Result<url::Url> {
    let mut origin = url::Url::parse(&record.origin)
        .map_err(|parse_error| error(format!("invalid legacy WebUI origin: {parse_error}")))?;
    if origin.scheme() != "http"
        || !origin.username().is_empty()
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
        || origin.path() != "/"
        || origin.port() != Some(record.port)
    {
        return Err(error("invalid legacy WebUI control origin"));
    }
    match origin.host_str() {
        Some("0.0.0.0") => origin.set_host(Some("127.0.0.1")),
        Some("::" | "[::]") => origin.set_host(Some("[::1]")),
        Some("localhost") => Ok(()),
        Some(host) if host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()) => Ok(()),
        _ => return Err(error("legacy WebUI control origin is not loopback")),
    }
    .map_err(|_| error("invalid legacy WebUI control address"))?;
    origin
        .join("webui/control/stop")
        .map_err(|parse_error| error(format!("invalid legacy WebUI control URL: {parse_error}")))
}

// An exiting process can briefly be uninspectable (macOS) while still visible to
// kill(pid, 0). The legacy server removes its own record on exit, so after an
// inspection error a missing or replaced record confirms the exit; otherwise keep
// polling and surface the error only at the deadline.
fn wait_for_exit(registry: &Registry, record: &LegacyRecord) -> Result<bool> {
    let deadline = Instant::now() + WAIT;
    loop {
        match record.verify_identity() {
            Ok(false) => return Ok(true),
            Ok(true) => {}
            Err(inspection_error) => {
                let owned = registry
                    .read()?
                    .is_some_and(|current| current.instance_id == record.instance_id);
                if !owned {
                    return Ok(true);
                }
                if Instant::now() >= deadline {
                    return Err(inspection_error);
                }
            }
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_json(pid: u32, identity: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "schema_version": 1,
            "instance_id": "webui-instance",
            "mode": "daemon",
            "pid": pid,
            "process_started_at": 1,
            "process_identity": identity,
            "host": "127.0.0.1",
            "port": 8484,
            "origin": "http://127.0.0.1:8484/",
            "control_token": "secret",
            "version": "0.3.0",
            "executable": "cruise",
            "working_dir": "/",
            "log_path": "webui.log",
            "started_at": 1
        })
    }

    #[test]
    fn parses_only_the_released_record_shape() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let registry = Registry::at(temp.path().join("webui"));
        fs::create_dir_all(&registry.dir)?;
        let value = record_json(42, Some("identity"));
        fs::write(registry.record_path(), value.to_string())?;
        assert_eq!(
            registry
                .read()?
                .ok_or_else(|| error("missing legacy record"))?
                .pid,
            42
        );
        let mut value = record_json(42, Some("identity"));
        value["extra"] = true.into();
        fs::write(registry.record_path(), value.to_string())?;
        assert!(registry.read().is_err());
        Ok(())
    }

    #[test]
    fn stale_records_and_files_are_removed_only_after_identity_check() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let registry = Registry::at(temp.path().join("webui"));
        fs::create_dir_all(&registry.dir)?;
        fs::write(
            registry.record_path(),
            record_json(std::process::id(), Some("different-process-identity")).to_string(),
        )?;
        fs::write(registry.dir.join("instance.lock"), b"")?;
        fs::write(registry.dir.join("webui.log"), b"")?;
        ensure_stopped_before_start(&registry)?;
        assert!(!registry.record_path().exists());
        assert!(!registry.dir.join("instance.lock").exists());
        assert!(!registry.dir.join("webui.log").exists());
        Ok(())
    }

    #[test]
    fn live_record_without_process_identity_is_unsafe() -> Result<()> {
        let mut value = record_json(std::process::id(), None);
        value["process_started_at"] =
            crate::daemon::instance::process_start_time(std::process::id())
                .unwrap_or_default()
                .into();
        let record: LegacyRecord =
            serde_json::from_value(value).unwrap_or_else(|error| panic!("{error}"));
        let Err(identity_error) = record.verify_identity() else {
            return Err(error("expected process identity verification to fail"));
        };
        assert!(
            identity_error
                .to_string()
                .contains("no precise process identity")
        );
        Ok(())
    }
}
