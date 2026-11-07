//! Persistent, private registry for the single local WebUI instance.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt as _;
use serde::{Deserialize, Serialize};

use crate::error::{CruiseError, Result};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceRecord {
    pub schema_version: u32,
    pub instance_id: String,
    pub mode: String,
    pub pid: u32,
    pub process_started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_identity: Option<String>,
    pub host: String,
    pub port: u16,
    pub origin: String,
    pub control_token: String,
    pub version: String,
    pub executable: PathBuf,
    pub working_dir: PathBuf,
    pub log_path: Option<PathBuf>,
    pub started_at: u64,
}

impl InstanceRecord {
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.verify_identity().unwrap_or(false)
    }

    pub fn verify_identity(&self) -> Result<bool> {
        if process_definitely_absent(self.pid) {
            return Ok(false);
        }
        let expected = self.process_identity.as_ref().ok_or_else(|| CruiseError::Other(
            "legacy WebUI record has no precise process identity; verify the process manually before removing the record".into()
        ))?;
        let actual = match precise_identity(self.pid) {
            Some(identity) => identity,
            None if process_definitely_absent(self.pid) => return Ok(false),
            None => {
                return Err(CruiseError::Other(format!(
                    "cannot verify WebUI process identity for PID {}; record retained",
                    self.pid
                )));
            }
        };
        if *expected != actual {
            return Ok(false);
        }
        match process_start_time(self.pid) {
            Some(started_at) => Ok(self.process_started_at == started_at),
            None if process_definitely_absent(self.pid) => Ok(false),
            None => Err(CruiseError::Other(format!(
                "cannot verify WebUI process start time for PID {}; record retained",
                self.pid
            ))),
        }
    }
}

/// Return a process's OS start time, in Unix seconds, if it can be inspected.
#[must_use]
pub fn process_start_time(pid: u32) -> Option<u64> {
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

#[cfg(unix)]
fn process_definitely_absent(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    unsafe {
        libc::kill(pid, 0) != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}

#[cfg(windows)]
fn process_definitely_absent(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if !handle.is_null() {
            CloseHandle(handle);
            return false;
        }
        io::Error::last_os_error().raw_os_error() == Some(87)
    }
}

#[cfg(not(any(unix, windows)))]
fn process_definitely_absent(_pid: u32) -> bool {
    false
}

#[cfg(target_os = "linux")]
pub fn precise_identity(pid: u32) -> Option<String> {
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
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
pub fn precise_identity(pid: u32) -> Option<String> {
    let pid = i32::try_from(pid).ok()?;
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    let result = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut _,
            size as i32,
        )
    };
    if result != size as i32 || info.pbi_pid != pid as u32 {
        return None;
    }
    Some(format!(
        "macos:{}:{}",
        info.pbi_start_tvsec, info.pbi_start_tvusec
    ))
}

#[cfg(windows)]
pub fn precise_identity(pid: u32) -> Option<String> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        Some(format!(
            "windows:{}:{}",
            creation.dwHighDateTime, creation.dwLowDateTime
        ))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn precise_identity(_pid: u32) -> Option<String> {
    None
}

#[must_use]
pub fn new_control_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Compare control tokens without short-circuiting on a byte mismatch.
#[must_use]
pub fn tokens_match(expected: &str, actual: &str) -> bool {
    let a = expected.as_bytes();
    let b = actual.as_bytes();
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    !a.is_empty() && diff == 0
}

#[derive(Clone, Debug)]
pub struct Registry {
    dir: PathBuf,
}

/// The lock is released when this value is dropped.
pub struct RegistryLock(File);

impl Drop for RegistryLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl Registry {
    pub fn new() -> Result<Self> {
        Ok(Self::at(crate::paths::state_dir()?.join("webui")))
    }

    #[must_use]
    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    #[must_use]
    pub fn record_path(&self) -> PathBuf {
        self.dir.join("instance.json")
    }
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.dir.join("webui.log")
    }

    pub fn prepare(&self) -> Result<()> {
        fs::create_dir_all(&self.dir)?;
        private_dir(&self.dir)?;
        Ok(())
    }

    pub fn lock(&self) -> Result<RegistryLock> {
        self.prepare()?;
        let file = private_open(&self.dir.join("instance.lock"), false)?;
        file.lock_exclusive()?;
        Ok(RegistryLock(file))
    }

    pub fn read(&self) -> Result<Option<InstanceRecord>> {
        let bytes = match fs::read(self.record_path()) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let record: InstanceRecord = serde_json::from_slice(&bytes).map_err(|error| {
            CruiseError::Other(format!("invalid WebUI instance record: {error}"))
        })?;
        if record.schema_version != 1 {
            return Err(CruiseError::Other(format!(
                "unsupported WebUI instance schema version: {}",
                record.schema_version
            )));
        }
        if !matches!(record.mode.as_str(), "foreground" | "daemon") {
            return Err(CruiseError::Other("invalid WebUI instance mode".to_owned()));
        }
        Ok(Some(record))
    }

    pub fn read_for_start(&self) -> Result<Option<InstanceRecord>> {
        self.read().map_err(|error| CruiseError::Other(format!(
            "cannot safely start WebUI: {error}; verify no WebUI is running, then inspect and remove the instance record at {}",
            self.record_path().display()
        )))
    }

    /// Publish via a private, uniquely named temporary file and atomic rename.
    pub fn write(&self, record: &InstanceRecord) -> Result<()> {
        self.prepare()?;
        let temp = self
            .dir
            .join(format!(".instance-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut file = private_open(&temp, true)?;
            serde_json::to_writer(&mut file, record)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, self.record_path())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }

    pub fn remove(&self) -> Result<()> {
        match fs::remove_file(self.record_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn remove_if_owned(&self, instance_id: &str) -> Result<bool> {
        if self.read()?.is_some_and(|r| r.instance_id == instance_id) {
            self.remove()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

fn private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    crate::platform::set_owner_only_acl(path)?;
    Ok(())
}

fn private_open(path: &Path, create_new: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    if create_new {
        options.create_new(true);
    } else {
        options.create(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    crate::platform::set_owner_only_acl(path)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> InstanceRecord {
        let pid = std::process::id();
        InstanceRecord {
            schema_version: 1,
            instance_id: "test-id".into(),
            mode: "foreground".into(),
            pid,
            process_started_at: process_start_time(pid).expect("current process start time"),
            process_identity: precise_identity(pid),
            host: "127.0.0.1".into(),
            port: 1234,
            origin: "http://127.0.0.1:1234/".into(),
            control_token: new_control_token(),
            version: "1".into(),
            executable: "test".into(),
            working_dir: ".".into(),
            log_path: None,
            started_at: 1,
        }
    }

    #[test]
    fn round_trip_identity_and_owned_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let registry = Registry::at(temp.path().join("webui"));
        let _guard = registry.lock().unwrap();
        let record = record();
        assert!(record.is_live());
        let mut reused = record.clone();
        reused.process_identity = Some("different-process-in-same-second".to_owned());
        assert_eq!(reused.pid, record.pid);
        assert_eq!(reused.process_started_at, record.process_started_at);
        assert!(!reused.verify_identity().unwrap());
        let mut wrong_start = record.clone();
        wrong_start.process_started_at += 1;
        assert!(!wrong_start.verify_identity().unwrap());
        registry.write(&record).unwrap();
        assert_eq!(registry.read().unwrap().unwrap().instance_id, "test-id");
        assert!(!registry.remove_if_owned("someone-else").unwrap());
        assert!(registry.remove_if_owned("test-id").unwrap());
        assert!(registry.read().unwrap().is_none());
    }

    #[test]
    fn registry_lock_blocks_another_handle_until_drop() {
        let temp = tempfile::tempdir().unwrap();
        let registry = Registry::at(temp.path().join("webui"));
        let guard = registry.lock().unwrap();
        let other = private_open(&registry.dir().join("instance.lock"), false).unwrap();
        let err = fs2::FileExt::try_lock_exclusive(&other).unwrap_err();
        assert_eq!(err.kind(), fs2::lock_contended_error().kind());
        drop(guard);
        fs2::FileExt::try_lock_exclusive(&other).unwrap();
        fs2::FileExt::unlock(&other).unwrap();
    }

    #[test]
    fn rejects_unknown_fields_and_versions() {
        let temp = tempfile::tempdir().unwrap();
        let registry = Registry::at(temp.path().join("webui"));
        let mut value = serde_json::to_value(record()).unwrap();
        registry.prepare().unwrap();
        value["unknown"] = true.into();
        fs::write(registry.record_path(), value.to_string()).unwrap();
        assert!(registry.read().is_err());
        value.as_object_mut().unwrap().remove("unknown");
        value["schema_version"] = 2.into();
        fs::write(registry.record_path(), value.to_string()).unwrap();
        assert!(registry.read().is_err());
    }

    #[test]
    fn tokens_have_expected_shape_and_compare() {
        let token = new_control_token();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(tokens_match(&token, &token));
        assert!(!tokens_match(&token, &token[..63]));
        assert!(!tokens_match(&token, &"z".repeat(64)));
    }

    #[cfg(unix)]
    #[test]
    fn registry_permissions_are_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        let registry = Registry::at(temp.path().join("webui"));
        let _guard = registry.lock().unwrap();
        registry.write(&record()).unwrap();
        assert_eq!(
            fs::metadata(registry.dir()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(registry.record_path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
