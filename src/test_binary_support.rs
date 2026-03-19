/// RAII guard that prepends a directory to `PATH` and restores the original value on drop.
/// Acquires `GLOBAL_PROCESS_LOCK` internally so callers do not need a separate `lock_process()`.
pub struct PathEnvGuard {
    prev: Option<std::ffi::OsString>,
    _lock: crate::test_support::ProcessLock,
}

impl PathEnvGuard {
    pub fn prepend(dir: &std::path::Path) -> Self {
        let lock = crate::test_support::lock_process();
        let prev = std::env::var_os("PATH");
        let mut paths = vec![dir.to_path_buf()];
        if let Some(ref existing) = prev {
            paths.extend(std::env::split_paths(existing));
        }
        // SAFETY: the test holds GLOBAL_PROCESS_LOCK, so no other test mutates PATH concurrently.
        let joined = std::env::join_paths(&paths)
            .unwrap_or_else(|e| panic!("join_paths failed: {e} (paths: {paths:?})"));
        unsafe { std::env::set_var("PATH", &joined) };
        Self { prev, _lock: lock }
    }
}

impl Drop for PathEnvGuard {
    fn drop(&mut self) {
        // SAFETY: the test holds GLOBAL_PROCESS_LOCK for the lifetime of the guard.
        unsafe {
            if let Some(ref prev) = self.prev {
                std::env::set_var("PATH", prev);
            } else {
                std::env::remove_var("PATH");
            }
        }
    }
}
