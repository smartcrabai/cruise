use std::path::{Path, PathBuf};
use std::process::Command;

use crate::session::{SessionPhase, SessionState};

pub static GLOBAL_PROCESS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
