use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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
    TERMINAL_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Default)]
struct SpinnerState {
    stopped: bool,
    pause_count: usize,
    rendered: bool,
    deferred_notices: Vec<String>,
}

struct SpinnerShared {
    state: Mutex<SpinnerState>,
}

impl SpinnerShared {
    fn lock(&self) -> MutexGuard<'_, SpinnerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn clear_rendered_line(state: &mut SpinnerState, term: &Term) {
    if state.rendered {
        let _ = term.clear_line();
        state.rendered = false;
    }
}

/// Cloneable control state for a running [`Spinner`].
///
/// The control handle never owns the spinner thread or its join handle. It can
/// therefore be captured by a tool callback without extending the spinner's
/// lifetime.
#[derive(Clone)]
pub(crate) struct SpinnerControl {
    shared: Arc<SpinnerShared>,
}

impl SpinnerControl {
    /// Pause rendering and return a guard that resumes it when dropped.
    ///
    /// Only the short terminal-clearing operation takes the terminal lock. The
    /// caller can hold the returned guard while waiting for user input.
    pub(crate) fn pause(&self) -> SpinnerPauseGuard {
        let _terminal = terminal_lock();
        let mut state = self.shared.lock();
        if !state.stopped {
            state.pause_count = state.pause_count.saturating_add(1);
            if state.pause_count == 1 {
                clear_rendered_line(&mut state, &Term::stderr());
            }
        }
        SpinnerPauseGuard {
            shared: Some(Arc::clone(&self.shared)),
        }
    }

    /// Run a synchronized status callback while the spinner is suspended.
    pub(crate) fn suspend<F>(&self, f: F)
    where
        F: FnOnce(),
    {
        let _pause = self.pause();
        let _terminal = terminal_lock();
        f();
    }

    /// Print a planning notice immediately, or defer it while input owns the
    /// terminal. The state check and line clearing are one terminal-locked
    /// operation so a notice cannot race a pause and overwrite the question.
    pub(crate) fn notify_or_defer<F>(&self, message: String, f: F)
    where
        F: FnOnce(&str),
    {
        let _terminal = terminal_lock();
        let mut state = self.shared.lock();
        if state.stopped {
            drop(state);
            f(&message);
            return;
        }
        if state.pause_count > 0 {
            state.deferred_notices.push(message);
            return;
        }
        clear_rendered_line(&mut state, &Term::stderr());
        f(&message);
    }

    /// Flush notices deferred during an input pause, if all nested pauses have
    /// ended. The callback runs while the terminal lock is held, matching the
    /// existing `Spinner::suspend` output contract.
    pub(crate) fn flush_deferred<F>(&self, mut f: F)
    where
        F: FnMut(&str),
    {
        let _terminal = terminal_lock();
        let mut state = self.shared.lock();
        if state.stopped || state.pause_count > 0 || state.deferred_notices.is_empty() {
            return;
        }
        clear_rendered_line(&mut state, &Term::stderr());
        let notices = std::mem::take(&mut state.deferred_notices);
        for notice in notices {
            f(&notice);
        }
    }
}

/// RAII guard for a [`SpinnerControl::pause`] operation.
pub(crate) struct SpinnerPauseGuard {
    shared: Option<Arc<SpinnerShared>>,
}

impl Drop for SpinnerPauseGuard {
    fn drop(&mut self) {
        let Some(shared) = self.shared.take() else {
            return;
        };
        let _terminal = terminal_lock();
        let mut state = shared.lock();
        if !state.stopped && state.pause_count > 0 {
            state.pause_count -= 1;
        }
    }
}

/// An animated terminal spinner that cleans up on drop.
pub struct Spinner {
    shared: Arc<SpinnerShared>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    pub fn start(msg: &str) -> Self {
        let shared = Arc::new(SpinnerShared {
            state: Mutex::new(SpinnerState::default()),
        });
        if crate::console_mode::is_quiet() {
            return Spinner {
                shared,
                handle: None,
            };
        }
        let shared_clone = Arc::clone(&shared);
        let msg = msg.to_string();

        let handle = std::thread::spawn(move || {
            let term = Term::stderr();
            let mut i = 0usize;
            loop {
                let stopped = {
                    let _terminal = terminal_lock();
                    let mut state = shared_clone.lock();
                    if state.stopped {
                        clear_rendered_line(&mut state, &term);
                        true
                    } else if state.pause_count == 0 {
                        let _ =
                            term.write_str(&format!("\r  {} {}", FRAMES[i % FRAMES.len()], msg));
                        state.rendered = true;
                        false
                    } else {
                        clear_rendered_line(&mut state, &term);
                        false
                    }
                };
                if stopped {
                    break;
                }
                std::thread::sleep(Duration::from_millis(80));
                i += 1;
            }
        });

        Spinner {
            shared,
            handle: Some(handle),
        }
    }

    /// Return a control handle that does not own this spinner's thread.
    pub(crate) fn control(&self) -> SpinnerControl {
        SpinnerControl {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Whether this spinner owns a render thread.
    pub(crate) fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// Pause animation, run `f` (e.g. print a message), then resume.
    pub fn suspend<F: FnOnce()>(&self, f: F) {
        if crate::console_mode::is_quiet() {
            f();
            return;
        }
        self.control().suspend(f);
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        {
            let _terminal = terminal_lock();
            let mut state = self.shared.lock();
            state.stopped = true;
            state.deferred_notices.clear();
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Spinner, SpinnerControl, SpinnerShared, SpinnerState};
    use std::sync::{Arc, Mutex};

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

    fn test_control() -> SpinnerControl {
        SpinnerControl {
            shared: Arc::new(SpinnerShared {
                state: Mutex::new(SpinnerState::default()),
            }),
        }
    }

    #[test]
    fn nested_pause_resumes_only_after_the_last_guard_drops() {
        let _lock = crate::test_support::lock_process();
        let control = test_control();
        let first = control.pause();
        let second = control.pause();

        assert_eq!(control.shared.lock().pause_count, 2);
        drop(first);
        assert_eq!(control.shared.lock().pause_count, 1);
        drop(second);
        assert_eq!(control.shared.lock().pause_count, 0);
    }

    #[test]
    fn notices_are_deferred_during_pause_and_flushed_after_resume() {
        let _lock = crate::test_support::lock_process();
        let control = test_control();
        let pause = control.pause();
        let delivered = Arc::new(Mutex::new(Vec::new()));
        let queued = Arc::clone(&delivered);

        control.notify_or_defer("retrying".to_string(), |notice| {
            queued
                .lock()
                .unwrap_or_else(|error| panic!("{error:?}"))
                .push(notice.to_string());
        });
        assert!(
            delivered
                .lock()
                .unwrap_or_else(|error| panic!("{error:?}"))
                .is_empty()
        );
        assert_eq!(control.shared.lock().deferred_notices, ["retrying"]);

        drop(pause);
        control.flush_deferred(|notice| {
            delivered
                .lock()
                .unwrap_or_else(|error| panic!("{error:?}"))
                .push(notice.to_string());
        });
        assert_eq!(
            delivered
                .lock()
                .unwrap_or_else(|error| panic!("{error:?}"))
                .as_slice(),
            ["retrying"]
        );
    }

    #[test]
    fn pause_guard_does_not_restart_a_stopped_spinner() {
        let _lock = crate::test_support::lock_process();
        let control = test_control();
        control.shared.lock().stopped = true;

        let pause = control.pause();
        drop(pause);

        let state = control.shared.lock();
        assert!(state.stopped);
        assert_eq!(state.pause_count, 0);
    }
}
