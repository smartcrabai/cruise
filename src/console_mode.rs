use std::cell::RefCell;
use std::fmt;
use std::fmt::Write as _;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(crate) type StatusOutputSink = Arc<dyn Fn(&str) + Send + Sync>;

tokio::task_local! {
    static OPERATION_STATUS_SINK: StatusOutputSink;
}

thread_local! {
    static THREAD_STATUS_SINK: RefCell<Option<StatusOutputSink>> = const { RefCell::new(None) };
}

struct ThreadStatusSinkGuard(Option<StatusOutputSink>);

impl Drop for ThreadStatusSinkGuard {
    fn drop(&mut self) {
        THREAD_STATUS_SINK.with(|sink| *sink.borrow_mut() = self.0.take());
    }
}

pub(crate) fn install_thread_status_sink(sink: StatusOutputSink) -> impl Drop {
    let previous = THREAD_STATUS_SINK.with(|slot| slot.replace(Some(sink)));
    ThreadStatusSinkGuard(previous)
}

pub(crate) fn capture_status_sink() -> Option<StatusOutputSink> {
    OPERATION_STATUS_SINK
        .try_with(Arc::clone)
        .ok()
        .or_else(|| THREAD_STATUS_SINK.with(|sink| sink.borrow().clone()))
}
/// Whether status output is routed to an operation sink instead of the terminal.
pub(crate) fn has_status_sink() -> bool {
    OPERATION_STATUS_SINK
        .try_with(|_| true)
        .unwrap_or_else(|_| THREAD_STATUS_SINK.with(|sink| sink.borrow().is_some()))
}

pub(crate) async fn scope_status_sink<F: Future>(sink: StatusOutputSink, future: F) -> F::Output {
    OPERATION_STATUS_SINK.scope(sink, future).await
}

pub fn status_output(arguments: fmt::Arguments<'_>) {
    if let Some(sink) = capture_status_sink() {
        let mut output = String::new();
        let _ = output.write_fmt(arguments);
        sink(&output);
    } else if !is_quiet() {
        eprintln!("{arguments}");
    }
}

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
/// Route status output to the owning operation when one is active.
#[macro_export]
macro_rules! status_eprintln {
    ($($arg:tt)*) => {
        $crate::console_mode::status_output(format_args!($($arg)*))
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

    #[tokio::test]
    async fn operation_status_output_bypasses_quiet_mode() {
        use std::sync::{Arc, Mutex};

        let _lock = lock_process();
        let _quiet = QuietModeGuard;
        super::set_quiet(true);
        let output = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&output);
        let sink: super::StatusOutputSink = Arc::new(move |line| {
            captured
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(line.to_owned());
        });
        super::scope_status_sink(sink, async {
            crate::status_eprintln!("step {name}", name = "build");
        })
        .await;
        let output = output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(output.len(), 1);
        assert_eq!(output[0], "step build");
    }
}
