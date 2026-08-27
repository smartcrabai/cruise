use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use console::Term;

const FRAMES: &[char] = &['-', '/', '|', '\\', '-', '/', '|', '\\', '-', '/'];

/// Process-wide lock serializing all spinner output across instances.
///
/// Concurrent batch workers (`run --all --parallelism N`) each run their own
/// `Spinner`; without a shared lock the independent animation threads interleave
/// `\r`-prefixed frame rewrites and `clear_line`s with each other's (and other
/// workers') stderr lines, garbling the terminal. Every spinner frame write,
/// suspend, and teardown clears goes through this one lock.
static TERMINAL_LOCK: Mutex<()> = Mutex::new(());

fn terminal_lock() -> MutexGuard<'static, ()> {
    TERMINAL_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// An animated terminal spinner that cleans up on drop.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    pub fn start(msg: &str) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        if crate::console_mode::is_quiet() {
            return Spinner { stop, handle: None };
        }
        let stop_clone = stop.clone();
        let msg = msg.to_string();

        let handle = std::thread::spawn(move || {
            let term = Term::stderr();
            let mut i = 0usize;
            while !stop_clone.load(Ordering::Relaxed) {
                {
                    let _guard = terminal_lock();
                    let _ = term.write_str(&format!("\r  {} {}", FRAMES[i % FRAMES.len()], msg));
                }
                std::thread::sleep(Duration::from_millis(80));
                i += 1;
            }
            let _guard = terminal_lock();
            let _ = term.clear_line();
        });

        Spinner {
            stop,
            handle: Some(handle),
        }
    }

    /// Pause animation, run `f` (e.g. print a message), then resume.
    #[expect(clippy::unused_self)]
    pub fn suspend<F: FnOnce()>(&self, f: F) {
        if crate::console_mode::is_quiet() {
            f();
            return;
        }
        let _guard = terminal_lock();
        let _ = Term::stderr().clear_line();
        f();
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Spinner;

    struct QuietModeGuard;

    impl Drop for QuietModeGuard {
        fn drop(&mut self) {
            crate::console_mode::set_quiet(false);
        }
    }

    #[test]
    fn quiet_start_does_not_spawn_a_render_thread() {
        // Given: quiet console mode is enabled for a dashboard run
        let _lock = crate::test_support::lock_process();
        crate::console_mode::set_quiet(true);
        let _quiet = QuietModeGuard;

        // When: a spinner is started
        let spinner = Spinner::start("working");

        // Then: no animation thread is spawned
        assert!(spinner.handle.is_none());
    }

    #[test]
    fn quiet_suspend_still_executes_the_callback() {
        // Given: quiet console mode is enabled and a spinner has no render thread
        let _lock = crate::test_support::lock_process();
        crate::console_mode::set_quiet(true);
        let _quiet = QuietModeGuard;
        let spinner = Spinner::start("working");
        let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // When: output is suspended around a callback
        let called_by_callback = std::sync::Arc::clone(&called);
        spinner.suspend(|| {
            called_by_callback.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        // Then: the callback runs even though terminal clearing is skipped
        assert!(called.load(std::sync::atomic::Ordering::Relaxed));
    }
}
