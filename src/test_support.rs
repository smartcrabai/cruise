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
