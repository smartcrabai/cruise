use std::sync::{
    Arc, Mutex, MutexGuard, PoisonError,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use console::Term;

const FRAMES: &[char] = &['-', '/', '|', '\\', '-', '/', '|', '\\', '-', '/'];

/// Process-wide lock serializing all spinner output across instances.
static TERMINAL_LOCK: Mutex<()> = Mutex::new(());

fn terminal_lock() -> MutexGuard<'static, ()> {
    TERMINAL_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An animated terminal spinner that cleans up on drop.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    pub fn start(msg: &str) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
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
    pub fn suspend<F: FnOnce()>(&self, f: F) {
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
