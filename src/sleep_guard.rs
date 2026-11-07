//! Best-effort macOS sleep prevention for workflow execution.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Default)]
struct State {
    active: usize,
    child: Option<Child>,
}

pub(crate) struct SleepGuard(Option<Arc<Mutex<State>>>);

impl Drop for SleepGuard {
    fn drop(&mut self) {
        if let Some(state) = &self.0 {
            let mut state = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.active -= 1;
            if state.active == 0
                && let Some(mut child) = state.child.take()
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn acquire(
    state: &Arc<Mutex<State>>,
    spawn: impl FnOnce() -> std::io::Result<Child>,
) -> std::io::Result<SleepGuard> {
    let mut current = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if current.active == 0 {
        current.child = Some(spawn()?);
    }
    current.active += 1;
    Ok(SleepGuard(Some(Arc::clone(state))))
}

fn caffeinate_args(pid: u32) -> [String; 3] {
    ["-dis".into(), "-w".into(), pid.to_string()]
}

fn should_prevent(config: &crate::app_config::AppConfig, is_macos: bool) -> bool {
    is_macos && config.prevent_sleep
}

fn spawn_caffeinate(program: &Path, pid: u32) -> std::io::Result<Child> {
    Command::new(program)
        .args(caffeinate_args(pid))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

pub(crate) fn prevent_sleep_for_run() -> SleepGuard {
    static STATE: OnceLock<Arc<Mutex<State>>> = OnceLock::new();
    if !cfg!(target_os = "macos") {
        return SleepGuard(None);
    }
    let config = match crate::app_config::AppConfig::load() {
        Ok(config) => config,
        Err(error) => {
            crate::status_eprintln!("warning: could not load sleep prevention config: {error}");
            return SleepGuard(None);
        }
    };
    if !should_prevent(&config, true) {
        return SleepGuard(None);
    }
    let state = STATE.get_or_init(|| Arc::new(Mutex::new(State::default())));
    match acquire(state, || {
        spawn_caffeinate(Path::new("/usr/bin/caffeinate"), std::process::id())
    }) {
        Ok(guard) => guard,
        Err(error) => {
            crate::status_eprintln!("warning: could not prevent sleep: {error}");
            SleepGuard(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_and_platform_config() {
        assert_eq!(caffeinate_args(4242), ["-dis", "-w", "4242"]);
        let mut config = crate::app_config::AppConfig::default();
        assert!(!should_prevent(&config, true));
        config.prevent_sleep = true;
        assert!(should_prevent(&config, true));
        assert!(!should_prevent(&config, false));
    }

    #[test]
    fn missing_caffeinate_is_an_io_error() {
        assert_eq!(
            spawn_caffeinate(Path::new("/nonexistent/caffeinate"), 1)
                .err()
                .unwrap_or_else(|| panic!("missing executable should fail"))
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }

    #[cfg(unix)]
    fn sleeper() -> std::io::Result<Child> {
        Command::new("/bin/sleep").arg("60").spawn()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_caffeinate_is_stopped_after_last_guard() {
        let state = Arc::new(Mutex::new(State::default()));
        let guard = acquire(&state, || {
            spawn_caffeinate(Path::new("/usr/bin/caffeinate"), std::process::id())
        })
        .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        let pid = {
            let mut current = state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
            let child = current
                .child
                .as_mut()
                .unwrap_or_else(|| panic!("child must be running"));
            assert!(
                child
                    .try_wait()
                    .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                    .is_none()
            );
            child.id()
        };
        drop(guard);
        assert!(
            state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                .child
                .is_none()
        );
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", &pid.to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                .success()
        );
    }

    #[cfg(unix)]
    #[test]
    fn overlapping_and_sequential_runs_share_then_restart_child() {
        let state = Arc::new(Mutex::new(State::default()));
        let first = acquire(&state, sleeper)
            .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        let pid = state
            .lock()
            .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
            .child
            .as_ref()
            .unwrap_or_else(|| panic!("child must be running"))
            .id();
        let second = acquire(&state, || panic!("must reuse child"))
            .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        assert_eq!(
            state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                .active,
            2
        );
        drop(first);
        {
            let mut current = state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
            assert_eq!(current.active, 1);
            assert_eq!(
                current
                    .child
                    .as_ref()
                    .unwrap_or_else(|| panic!("child must be running"))
                    .id(),
                pid
            );
            assert!(
                current
                    .child
                    .as_mut()
                    .unwrap_or_else(|| panic!("child must be running"))
                    .try_wait()
                    .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                    .is_none()
            );
        }
        drop(second);
        assert_eq!(
            state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                .active,
            0
        );
        assert!(
            state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                .child
                .is_none()
        );
        let status = Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        assert!(!status.success());
        let next = acquire(&state, sleeper)
            .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        assert_eq!(
            state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                .active,
            1
        );
        drop(next);
        assert!(
            state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                .child
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_spawn_can_retry_and_error_unwinds_guard() {
        let state = Arc::new(Mutex::new(State::default()));
        assert!(
            acquire(&state, || spawn_caffeinate(
                Path::new("/nonexistent/caffeinate"),
                1
            ))
            .is_err()
        );
        assert_eq!(
            state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                .active,
            0
        );
        let run = || -> std::io::Result<()> {
            let _guard = acquire(&state, sleeper)?;
            Err(std::io::Error::other("run failed"))
        };
        assert!(run().is_err());
        assert!(
            state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                .child
                .is_none()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_future_releases_guard() {
        let state = Arc::new(Mutex::new(State::default()));
        let task_state = Arc::clone(&state);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _guard = acquire(&task_state, sleeper)
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
            let _ = ready_tx.send(());
            std::future::pending::<()>().await;
        });
        ready_rx
            .await
            .unwrap_or_else(|error| panic!("test operation failed: {error:?}"));
        task.abort();
        let _ = task.await;
        assert_eq!(
            state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                .active,
            0
        );
        assert!(
            state
                .lock()
                .unwrap_or_else(|error| panic!("test operation failed: {error:?}"))
                .child
                .is_none()
        );
    }
}
