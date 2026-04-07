use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use console::Term;

const FRAMES: &[char] = &['-', '/', '|', '\\', '-', '/', '|', '\\', '-', '/'];

/// An animated terminal spinner that cleans up on drop.
pub struct Spinner {
    stop: Arc<AtomicBool>,
    /// Held by the animation thread each frame; grab to pause animation.
    lock: Arc<Mutex<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Spinner {
    pub fn start(msg: &str) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let lock = Arc::new(Mutex::new(()));
        let stop_clone = stop.clone();
        let lock_clone = lock.clone();
        let msg = msg.to_string();

        let handle = std::thread::spawn(move || {
            let term = Term::stderr();
            let mut i = 0usize;
            while !stop_clone.load(Ordering::Relaxed) {
                {
                    let _guard = lock_clone
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let _ = term.write_str(&format!("\r  {} {}", FRAMES[i % FRAMES.len()], msg));
                }
                std::thread::sleep(Duration::from_millis(80));
                i += 1;
            }
            let _ = term.clear_line();
        });

        Spinner {
            stop,
            lock,
            handle: Some(handle),
        }
    }

    /// Pause animation, run `f` (e.g. print a message), then resume.
    pub fn suspend<F: FnOnce()>(&self, f: F) {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
