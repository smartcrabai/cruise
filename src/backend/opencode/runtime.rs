use super::OpenCodeRunnerConfig;
use crate::cancellation::CancellationToken;
use command_group::{AsyncCommandGroup, AsyncGroupChild};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::{ChildStdin, Command},
};

#[derive(Serialize, Deserialize)]
struct Manifest {
    owner: String,
    id: String,
    cwd: PathBuf,
    clean: bool,
}
pub(crate) struct Storage {
    pub root: PathBuf,
    id: String,
    cwd: PathBuf,
    _lock: File,
    keep: bool,
    pub initial_clean: bool,
}
fn storage_path(id: &str) -> std::result::Result<PathBuf, String> {
    let mut key = String::with_capacity(64);
    for byte in Sha256::digest(id.as_bytes()) {
        let _ = write!(&mut key, "{byte:02x}");
    }
    Ok(crate::paths::data_dir()
        .map_err(|e| e.to_string())?
        .join("opencode")
        .join(key))
}
fn reject_symlink(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(std::io::Error::other(
            "OpenCode private path must not be a symlink",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
pub(crate) fn private_dir(path: &Path) -> std::io::Result<()> {
    reject_symlink(path)?;
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    crate::platform::set_owner_only_acl(path)?;
    Ok(())
}
pub(crate) fn private_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    reject_symlink(path)?;
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    crate::platform::set_owner_only_acl(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}
fn open_lock(path: &Path, create: bool) -> std::io::Result<File> {
    reject_symlink(path)?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    crate::platform::set_owner_only_acl(path)?;
    Ok(file)
}
impl Storage {
    pub fn prepare(
        config: &OpenCodeRunnerConfig,
        id: String,
        cwd: PathBuf,
    ) -> std::result::Result<Self, String> {
        let root = storage_path(&id)?;
        if let Some(parent) = root.parent() {
            private_dir(parent).map_err(|e| e.to_string())?;
        }
        reject_symlink(&root).map_err(|e| e.to_string())?;
        for name in [
            "manifest.json",
            "lock",
            "opencode.db",
            "opencode.db-wal",
            "opencode.db-shm",
        ] {
            reject_symlink(&root.join(name)).map_err(|e| e.to_string())?;
        }
        if config.resume_session_id.is_some() && !root.join("manifest.json").is_file() {
            return Err(format!("OpenCode resume conversation {id} is missing"));
        }
        private_dir(&root).map_err(|e| e.to_string())?;
        let lock = open_lock(&root.join("lock"), true).map_err(|e| e.to_string())?;
        lock.try_lock_exclusive()
            .map_err(|_| "OpenCode conversation is already active".to_owned())?;
        if config.resume_session_id.is_some() {
            let manifest: Manifest = serde_json::from_slice(
                &fs::read(root.join("manifest.json")).map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            if manifest.owner != "cruise-opencode-v2" || manifest.id != id || manifest.cwd != cwd {
                return Err(
                    "OpenCode resume storage ownership or working directory mismatch".into(),
                );
            }
            if !manifest.clean {
                return Err(
                    "OpenCode resume refused: previous execution outcome is uncertain".into(),
                );
            }
        }
        let storage = Self {
            root,
            id,
            cwd,
            _lock: lock,
            keep: config.keep_session_home,
            initial_clean: config.resume_session_id.is_some(),
        };
        storage.mark(storage.initial_clean)?;
        Ok(storage)
    }
    pub fn mark(&self, clean: bool) -> std::result::Result<(), String> {
        let value = Manifest {
            owner: "cruise-opencode-v2".into(),
            id: self.id.clone(),
            cwd: self.cwd.clone(),
            clean,
        };
        private_write(
            &self.root.join("manifest.json"),
            &serde_json::to_vec(&value).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }
    pub fn finish(&self, clean: bool, checkpointed: bool) -> std::result::Result<(), String> {
        if self.keep && checkpointed {
            self.mark(clean)
        } else {
            fs::remove_dir_all(&self.root).map_err(|e| e.to_string())
        }
    }
}
pub(crate) fn cleanup_conversation(
    id: &str,
    _working_dir: Option<&Path>,
) -> crate::error::Result<()> {
    let run = || -> std::result::Result<(), String> {
        let root = storage_path(id)?;
        if let Some(parent) = root.parent() {
            reject_symlink(parent).map_err(|e| e.to_string())?;
        }
        if !root.exists() {
            return Ok(());
        }
        if fs::symlink_metadata(&root)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("OpenCode storage must not be a symlink".into());
        }
        for name in ["manifest.json", "lock"] {
            reject_symlink(&root.join(name)).map_err(|e| e.to_string())?;
        }
        let manifest: Manifest = serde_json::from_slice(
            &fs::read(root.join("manifest.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if manifest.owner != "cruise-opencode-v2" || manifest.id != id {
            return Err("OpenCode storage ownership mismatch".into());
        }
        let lock = open_lock(&root.join("lock"), false).map_err(|e| e.to_string())?;
        lock.try_lock_exclusive()
            .map_err(|_| "OpenCode conversation is active".to_owned())?;
        fs::remove_dir_all(root).map_err(|e| e.to_string())
    };
    run().map_err(crate::error::CruiseError::Other)
}

pub(crate) struct Runtime {
    child: AsyncGroupChild,
    #[cfg(unix)]
    group: Option<i32>,
    stdin: Option<ChildStdin>,
    readers: Vec<tokio::task::JoinHandle<()>>,
    pub password: String,
    stderr: Arc<Mutex<Vec<u8>>>,
}
impl Runtime {
    pub fn spawn(
        config: &OpenCodeRunnerConfig,
        storage: &Storage,
        overlay: &str,
    ) -> std::result::Result<Self, String> {
        let password = uuid::Uuid::new_v4().simple().to_string();
        let mut command = Command::new(
            config
                .cli_path
                .as_deref()
                .unwrap_or_else(|| Path::new("opencode")),
        );
        command
            .args(["serve", "--stdio", "--hostname", "127.0.0.1", "--port", "0"])
            .envs(&config.env)
            .env("OPENCODE_SERVER_PASSWORD", &password)
            .env("OPENCODE_DB", storage.root.join("opencode.db"))
            .env("OPENCODE_CONFIG_CONTENT", overlay)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &config.cwd {
            command.current_dir(cwd);
        }
        let mut child = command
            .group_spawn()
            .map_err(|e| format!("OpenCode CLI startup failed: {e}"))?;
        let stdin = child.inner().stdin.take();
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let mut readers = vec![];
        if let Some(mut pipe) = child.inner().stderr.take() {
            let output = stderr.clone();
            readers.push(tokio::spawn(async move {
                let mut bytes = [0; 4096];
                while let Ok(count) = pipe.read(&mut bytes).await {
                    if count == 0 {
                        break;
                    }
                    let mut target = output
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    target.extend_from_slice(&bytes[..count]);
                    if target.len() > 32768 {
                        let excess = target.len() - 32768;
                        target.drain(..excess);
                    }
                }
            }));
        }
        #[cfg(unix)]
        let group = child.id().and_then(|pid| i32::try_from(pid).ok());
        Ok(Self {
            child,
            #[cfg(unix)]
            group,
            stdin,
            readers,
            password,
            stderr,
        })
    }
    pub async fn startup(
        &mut self,
        cancel: &CancellationToken,
    ) -> std::result::Result<url::Url, String> {
        let stdout = self
            .child
            .inner()
            .stdout
            .take()
            .ok_or("OpenCode missing startup stdout")?;
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        let read = tokio::select! {
            ()=cancel.cancelled()=>return Err("OpenCode startup interrupted".into()),
            value=tokio::time::timeout(Duration::from_secs(7),async {
                loop {
                    let byte=reader.read_u8().await;
                    match byte {
                        Ok(byte)=>{
                            line.push(byte);
                            if byte==b'\n' {break Ok(line.len());}
                            if line.len()>16384 {break Err(std::io::Error::other("OpenCode startup JSON exceeds limit"));}
                        }
                        Err(error) if error.kind()==std::io::ErrorKind::UnexpectedEof=>break Ok(line.len()),
                        Err(error)=>break Err(error)
                    }
                }
            })=>value.map_err(|_|"OpenCode startup timed out")?.map_err(|e|e.to_string())?
        };
        self.readers.push(tokio::spawn(async move {
            let mut buffer = [0; 4096];
            while let Ok(n) = reader.read(&mut buffer).await {
                if n == 0 {
                    break;
                }
            }
        }));
        if read == 0 {
            return Err(format!(
                "OpenCode CLI startup failed: {}",
                self.diagnostic()
            ));
        }
        if line.len() > 16384 {
            return Err("OpenCode startup JSON exceeds limit".into());
        }
        let value: serde_json::Value = serde_json::from_slice(&line)
            .map_err(|e| format!("OpenCode V2 startup JSON malformed: {e}"))?;
        let url = url::Url::parse(
            value["url"]
                .as_str()
                .ok_or("OpenCode startup missing URL")?,
        )
        .map_err(|e| format!("OpenCode startup URL: {e}"))?;
        let local = match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        if url.scheme() != "http"
            || !local
            || url.port().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.path() != "/"
        {
            return Err(
                "OpenCode startup URL must be an authenticated private loopback HTTP address"
                    .into(),
            );
        }
        Ok(url)
    }
    fn diagnostic(&self) -> String {
        String::from_utf8_lossy(
            &self
                .stderr
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .replace(&self.password, "[redacted]")
    }
    pub async fn stop(&mut self) -> std::result::Result<(), String> {
        #[cfg(unix)]
        if let Some(pid) = self.group {
            unsafe {
                libc::kill(-pid, libc::SIGINT);
            }
        }
        self.stdin.take();
        let first_wait = tokio::time::timeout(Duration::from_millis(800), self.child.wait()).await;
        let mut wait_error = match &first_wait {
            Ok(Err(error)) => Some(format!("OpenCode wait failed: {error}")),
            _ => None,
        };
        if !matches!(first_wait, Ok(Ok(_))) {
            #[cfg(unix)]
            if let Some(pid) = self.group {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
            let _ = self.child.start_kill();
            match tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    wait_error.get_or_insert_with(|| format!("OpenCode reap failed: {error}"));
                }
                Err(_) => {
                    wait_error.get_or_insert_with(|| "OpenCode reap timed out".to_owned());
                }
            }
        }
        #[cfg(unix)]
        if let Some(pid) = self.group.take() {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        for reader in self.readers.drain(..) {
            let _ = tokio::time::timeout(Duration::from_secs(1), reader).await;
        }
        wait_error.map_or(Ok(()), Err)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        #[cfg(unix)]
        if let Some(pid) = self.group {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        for reader in &self.readers {
            reader.abort();
        }
    }
}
