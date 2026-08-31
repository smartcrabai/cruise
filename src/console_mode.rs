use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static QUIET: AtomicBool = AtomicBool::new(false);
static QUIET_GUARDS: AtomicUsize = AtomicUsize::new(0);
static QUIET_ROOT_RESTORE: AtomicBool = AtomicBool::new(false);

/// Enable or disable suppression of transient CLI status output.
pub fn set_quiet(on: bool) {
    QUIET.store(on, Ordering::SeqCst);
}

/// Return whether transient CLI status output is currently suppressed.
#[must_use]
pub fn is_quiet() -> bool {
    QUIET.load(Ordering::SeqCst)
}

/// Guard that enables quiet mode until the final overlapping guard is dropped.
/// Restoration uses the mode captured by the first guard, regardless of drop order.
pub struct QuietModeGuard;

#[must_use]
pub fn quiet_guard() -> QuietModeGuard {
    if QUIET_GUARDS.fetch_add(1, Ordering::SeqCst) == 0 {
        QUIET_ROOT_RESTORE.store(is_quiet(), Ordering::SeqCst);
    }
    set_quiet(true);
    QuietModeGuard
}

impl Drop for QuietModeGuard {
    fn drop(&mut self) {
        if QUIET_GUARDS.fetch_sub(1, Ordering::SeqCst) == 1 {
            set_quiet(QUIET_ROOT_RESTORE.load(Ordering::SeqCst));
        }
    }
}
/// Print status output unless the live batch dashboard owns stderr.
#[macro_export]
macro_rules! status_eprintln {
    ($($arg:tt)*) => {
        if !$crate::console_mode::is_quiet() {
            eprintln!($($arg)*);
        }
    };
}

#[cfg(test)]
mod tests {
    use crate::test_support::lock_process;

    struct QuietModeGuard;

    impl Drop for QuietModeGuard {
        fn drop(&mut self) {
            super::set_quiet(false);
        }
    }

    #[test]
    fn status_output_is_enabled_by_default() {
        let _lock = lock_process();
        let _quiet = QuietModeGuard;
        super::set_quiet(false);
        assert!(!super::is_quiet());
    }

    #[test]
    fn quiet_mode_suppresses_status_output() {
        let _lock = lock_process();
        let _quiet = QuietModeGuard;
        super::set_quiet(true);
        crate::status_eprintln!("this must not appear in quiet mode");
        assert!(super::is_quiet());
    }
}
