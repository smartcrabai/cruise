use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A token that can be used to signal cancellation to an in-progress workflow execution.
///
/// Clones share the same underlying state via `Arc`; cancelling any clone cancels all.
/// Designed to be cheap to clone and thread-safe.
/// A cancellation token. Clones share local state; child tokens additionally
/// observe cancellation of their parent (and every ancestor) without being able
/// to cancel siblings or the parent.
#[derive(Clone, Debug)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    ancestors: Arc<Vec<Arc<AtomicBool>>>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationToken {
    /// Create a new, uncancelled token.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            ancestors: Arc::new(Vec::new()),
        }
    }

    /// Create an independently cancellable token linked to this token.
    /// Cancelling the child does not affect this token or any sibling.
    #[must_use]
    pub fn child_token(&self) -> Self {
        let mut ancestors = (*self.ancestors).clone();
        ancestors.push(Arc::clone(&self.cancelled));
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            ancestors: Arc::new(ancestors),
        }
    }

    /// Signal cancellation. After this call, `is_cancelled()` returns `true` on all clones.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self
                .ancestors
                .iter()
                .any(|parent| parent.load(Ordering::Acquire))
    }

    /// Returns a future that resolves once the token has been cancelled.
    ///
    /// Polls every 50 ms. Acceptable latency for interactive cancellation of long-running LLM calls.
    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_token_is_not_cancelled() {
        // Given: a freshly created CancellationToken
        let token = CancellationToken::new();
        // Then: it is not cancelled
        assert!(!token.is_cancelled());
    }

    #[test]
    fn test_cancel_marks_token_as_cancelled() {
        // Given: a new, uncancelled token
        let token = CancellationToken::new();
        // When: cancel() is called
        token.cancel();
        // Then: is_cancelled() returns true
        assert!(token.is_cancelled());
    }

    #[test]
    fn test_clone_cancel_affects_original() {
        // Given: a token and its clone
        let token = CancellationToken::new();
        let clone = token.clone();
        // When: the clone is cancelled
        clone.cancel();
        // Then: the original also sees the cancellation (Arc-shared state)
        assert!(token.is_cancelled());
    }

    #[test]
    fn test_cancel_original_affects_clone() {
        // Given: a token and its clone
        let token = CancellationToken::new();
        let clone = token.clone();
        // When: the original is cancelled
        token.cancel();
        // Then: the clone also reports cancelled
        assert!(clone.is_cancelled());
    }

    #[test]
    fn test_multiple_cancel_calls_are_idempotent() {
        // Given: a token
        let token = CancellationToken::new();
        // When: cancel() is called multiple times
        token.cancel();
        token.cancel();
        token.cancel();
        // Then: still simply cancelled, no panic
        assert!(token.is_cancelled());
    }

    #[test]
    fn test_cancel_from_another_thread() {
        // Given: a token shared with another thread
        let token = CancellationToken::new();
        let token_in_thread = token.clone();
        // When: a separate thread calls cancel()
        let handle = std::thread::spawn(move || {
            token_in_thread.cancel();
        });
        handle.join().unwrap_or_else(|_| panic!("thread panicked"));
        // Then: the main thread sees the cancellation
        assert!(token.is_cancelled());
    }

    #[test]
    fn test_default_is_not_cancelled() {
        // Given: a token created via Default trait
        let token = CancellationToken::default();
        // Then: it is not cancelled
        assert!(!token.is_cancelled());
    }
    #[test]
    fn child_cancellation_is_isolated() {
        let parent = CancellationToken::new();
        let child = parent.child_token();
        let sibling = parent.child_token();
        child.cancel();
        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());
        assert!(!sibling.is_cancelled());
    }

    #[test]
    fn parent_cancellation_reaches_nested_children() {
        let parent = CancellationToken::new();
        let child = parent.child_token();
        let grandchild = child.child_token();
        parent.cancel();
        assert!(child.is_cancelled());
        assert!(grandchild.is_cancelled());
    }
}
