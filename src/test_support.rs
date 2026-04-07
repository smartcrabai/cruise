use std::path::{Path, PathBuf};
use std::process::Command;

use crate::session::{SessionPhase, SessionState};

pub static GLOBAL_PROCESS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that restores a single environment variable on drop.
/// Caller must hold `GLOBAL_PROCESS_LOCK` to ensure no concurrent env access.
pub struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}
impl EnvGuard {
    #[must_use]
    pub fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: caller holds GLOBAL_PROCESS_LOCK, so no concurrent env access.
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }
    #[must_use]
    pub fn remove(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: caller holds GLOBAL_PROCESS_LOCK, so no concurrent env access.
        unsafe { std::env::remove_var(key) };
        Self { key, prev }
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: callers must hold GLOBAL_PROCESS_LOCK for the full lifetime of this guard.
        // This is a trusted-caller contract, not a compile-time guarantee.
        // By convention, declare the ProcessLock (from lock_process()) before EnvGuard in
        // the same scope; Rust's reverse-drop order then ensures the lock outlives the guard.
        unsafe {
            if let Some(ref v) = self.prev {
                std::env::set_var(self.key, v);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

pub struct ProcessLock {
    _guard: std::sync::MutexGuard<'static, ()>,
}

pub fn lock_process() -> ProcessLock {
    let guard = GLOBAL_PROCESS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if std::env::current_dir().is_err() {
        #[cfg(unix)]
        let _ = std::env::set_current_dir("/");
        #[cfg(windows)]
        let _ = std::env::set_current_dir(
            std::env::var("SYSTEMDRIVE").unwrap_or_else(|_| "C:".into()) + "\\",
        );
    }
    ProcessLock { _guard: guard }
}

/// Run a git command in the given directory, panicking if it fails.
///
/// # Panics
///
/// Panics if the git command fails to start or exits with a non-zero status.
pub fn run_git_ok(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("git command failed to start: {e}"));
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr).trim()
    );
}

/// Initialise a regular git repository with an initial commit in the given directory.
///
/// # Panics
///
/// Panics if any git command or file-system operation fails.
pub fn init_git_repo(dir: &Path) {
    run_git_ok(dir, &["init"]);
    run_git_ok(dir, &["config", "user.email", "test@example.com"]);
    run_git_ok(dir, &["config", "user.name", "Test"]);
    std::fs::write(dir.join("README.md"), "init").unwrap_or_else(|e| panic!("{e:?}"));
    run_git_ok(dir, &["add", "."]);
    run_git_ok(dir, &["commit", "-m", "init"]);
    run_git_ok(dir, &["branch", "-M", "main"]);
}

/// Prepend `dir` to the current `PATH` and return a guard that restores the
/// original value on drop.  Caller must hold `GLOBAL_PROCESS_LOCK`.
///
/// # Panics
///
/// Panics if the resulting path cannot be joined into a valid `OsString`.
#[must_use]
pub fn prepend_to_path(dir: &Path) -> EnvGuard {
    let prev_path = std::env::var_os("PATH");
    let mut paths = vec![dir.to_path_buf()];
    if let Some(ref existing) = prev_path {
        paths.extend(std::env::split_paths(existing));
    }
    let joined = std::env::join_paths(&paths).unwrap_or_else(|e| panic!("{e}"));
    EnvGuard::set("PATH", joined)
}

/// Install a minimal `gh` stub that exits 0 for `--version` and 1 for
/// everything else.  Used to simulate an installed `gh` CLI that passes the
/// preflight check.
///
/// # Panics
///
/// Panics if writing the script file, reading its metadata, or setting
/// permissions fails.
#[cfg(unix)]
pub fn install_version_only_gh(bin_dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let script_path = bin_dir.join("gh");
    let script = concat!(
        "#!/bin/sh\n",
        "if [ \"$1\" = \"--version\" ]; then\n",
        "  printf 'gh version test\\n'\n",
        "  exit 0\n",
        "fi\n",
        "exit 1\n",
    );
    std::fs::write(&script_path, script).unwrap_or_else(|e| panic!("{e:?}"));
    let mut perms = std::fs::metadata(&script_path)
        .unwrap_or_else(|e| panic!("{e:?}"))
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms).unwrap_or_else(|e| panic!("{e:?}"));
}

/// Create a minimal `Planned` session for use in tests.
#[must_use]
pub fn make_session(id: &str, base_dir: &Path) -> SessionState {
    let mut session = SessionState::new(
        id.to_string(),
        PathBuf::from(base_dir),
        "cruise.yaml".to_string(),
        "test task".to_string(),
    );
    session.phase = SessionPhase::Planned;
    session
}
