use std::sync::atomic::{AtomicBool, Ordering};

static QUIET: AtomicBool = AtomicBool::new(false);

/// Enable or disable suppression of transient CLI status output.
pub fn set_quiet(on: bool) {
    QUIET.store(on, Ordering::SeqCst);
}

/// Return whether transient CLI status output is currently suppressed.
#[must_use]
pub fn is_quiet() -> bool {
    QUIET.load(Ordering::SeqCst)
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
