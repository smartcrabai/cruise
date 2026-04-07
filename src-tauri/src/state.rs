use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use cruise::cancellation::CancellationToken;
use cruise::step::option::OptionResult;
use tokio::sync::oneshot;

/// Per-session runtime state held while a single session is executing.
#[derive(Debug)]
pub struct PerSessionState {
    /// Cancellation token for this session's workflow.
    ///
    /// Cancelling this token also cancels every clone, which the engine polls.
    pub cancel_token: CancellationToken,
    /// Pending oneshot sender waiting for a `respond_to_option` IPC call.
    ///
    /// `None` when the session is not waiting for user input.
    pub option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
}

impl PerSessionState {
    /// Create a new [`PerSessionState`] with a fresh, uncancelled token and no pending response.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancel_token: CancellationToken::new(),
            option_responder: Arc::new(Mutex::new(None)),
        }
    }
}

/// Shared Tauri application state, injected into command handlers via `tauri::State`.
///
/// # Multi-session support
///
/// Previously this held a single `cancel_token` / `option_responder` / `active_session_id`.
/// That singleton design prevented multiple sessions from running concurrently because:
///
/// - Cancelling the token cancelled *every* active session.
/// - `option_responder` could hold only one pending dialog at a time.
/// - `respond_to_option` had no way to route the answer to the correct session.
///
/// This version replaces the singletons with a `HashMap<session_id, PerSessionState>` so that
/// independent sessions can each own their own cancellation token and pending dialog slot.
pub struct AppState {
    /// Active sessions keyed by session ID.
    ///
    /// A session is present in this map while it is in the `Running` phase.
    /// It is removed when the session finishes (Completed / Failed / Cancelled).
    pub sessions: Mutex<HashMap<String, PerSessionState>>,
}

impl AppState {
    /// Create a new [`AppState`] with no active sessions.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Register a new active session, returning its option-responder and cancellation token.
    ///
    /// Returns `(option_responder, cancel_token)` in a single lock acquisition.
    /// If a session with the same `session_id` was already registered, it is replaced.
    ///
    /// # Panics
    ///
    /// Panics if the inner mutex is poisoned.
    pub fn register_session(
        &self,
        session_id: String,
    ) -> (
        Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
        CancellationToken,
    ) {
        let state = PerSessionState::new();
        let responder = Arc::clone(&state.option_responder);
        let token = state.cancel_token.clone();
        self.sessions
            .lock()
            .unwrap_or_else(|e| panic!("AppState mutex poisoned: {e}"))
            .insert(session_id, state);
        (responder, token)
    }

    /// Return a clone of the cancellation token for an active session.
    ///
    /// Returns `None` if the session is not currently registered (not running).
    ///
    /// # Panics
    ///
    /// Panics if the inner mutex is poisoned.
    #[cfg(test)]
    pub(crate) fn cancel_token_for(&self, session_id: &str) -> Option<CancellationToken> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| panic!("AppState mutex poisoned: {e}"))
            .get(session_id)
            .map(|s| s.cancel_token.clone())
    }

    /// Cancel the workflow for a specific session.
    ///
    /// Does nothing if the session is not registered.
    ///
    /// # Panics
    ///
    /// Panics if the inner mutex is poisoned.
    pub fn cancel_session(&self, session_id: &str) {
        if let Some(state) = self
            .sessions
            .lock()
            .unwrap_or_else(|e| panic!("AppState mutex poisoned: {e}"))
            .get(session_id)
        {
            state.cancel_token.cancel();
        }
    }

    /// Cancel **all** active sessions.
    ///
    /// Used when the user clicks "Cancel" during a Run All batch.
    ///
    /// # Panics
    ///
    /// Panics if the inner mutex is poisoned.
    pub fn cancel_all_sessions(&self) {
        for state in self
            .sessions
            .lock()
            .unwrap_or_else(|e| panic!("AppState mutex poisoned: {e}"))
            .values()
        {
            state.cancel_token.cancel();
        }
    }

    /// Route an option-step response to the correct session by its request/session ID.
    ///
    /// Returns `true` if a pending sender was found and the result was sent.
    /// Returns `false` if the session is not registered or has no pending dialog.
    ///
    /// # Panics
    ///
    /// Panics if the inner mutex is poisoned.
    pub fn respond_to_option(&self, session_id: &str, result: OptionResult) -> bool {
        // Clone the Arc while holding the outer lock, then release it before locking the inner
        // mutex so that register_session / unregister_session are not blocked during send.
        let responder = {
            let sessions = self
                .sessions
                .lock()
                .unwrap_or_else(|e| panic!("AppState mutex poisoned: {e}"));
            sessions
                .get(session_id)
                .map(|s| Arc::clone(&s.option_responder))
        };
        let Some(responder) = responder else {
            return false;
        };
        let mut guard = responder
            .lock()
            .unwrap_or_else(|e| panic!("option_responder mutex poisoned: {e}"));
        let Some(sender) = guard.take() else {
            return false;
        };
        let _ = sender.send(result);
        true
    }

    /// Unregister a session that has finished executing.
    ///
    /// # Panics
    ///
    /// Panics if the inner mutex is poisoned.
    pub fn unregister_session(&self, session_id: &str) {
        self.sessions
            .lock()
            .unwrap_or_else(|e| panic!("AppState mutex poisoned: {e}"))
            .remove(session_id);
    }

    /// Return the number of currently active (registered) sessions.
    ///
    /// # Panics
    ///
    /// Panics if the inner mutex is poisoned.
    #[cfg(test)]
    pub(crate) fn active_session_count(&self) -> usize {
        self.sessions
            .lock()
            .unwrap_or_else(|e| panic!("AppState mutex poisoned: {e}"))
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cruise::step::option::OptionResult;
    use tokio::sync::oneshot;

    fn make_option_result(next_step: &str) -> OptionResult {
        OptionResult {
            next_step: Some(next_step.to_string()),
            text_input: None,
        }
    }

    // ── Initial state ─────────────────────────────────────────────────────────

    #[test]
    fn test_new_state_has_no_active_sessions() {
        // Given/When: a fresh AppState
        let state = AppState::new();
        // Then: no active sessions
        assert_eq!(state.active_session_count(), 0);
    }

    // ── Session registration ──────────────────────────────────────────────────

    #[test]
    fn test_register_session_adds_it_to_active_map() {
        // Given: a fresh AppState
        let state = AppState::new();
        // When: a session is registered
        state.register_session("sess-1".to_string());
        // Then: one active session
        assert_eq!(state.active_session_count(), 1);
    }

    #[test]
    fn test_register_two_sessions_are_tracked_independently() {
        // Given: a fresh AppState
        let state = AppState::new();
        // When: two sessions are registered
        state.register_session("sess-1".to_string());
        state.register_session("sess-2".to_string());
        // Then: two active sessions
        assert_eq!(state.active_session_count(), 2);
    }

    #[test]
    fn test_cancel_tokens_are_isolated_per_session() {
        // Given: two registered sessions
        let state = AppState::new();
        state.register_session("sess-1".to_string());
        state.register_session("sess-2".to_string());

        // When: only sess-1 is cancelled
        state.cancel_session("sess-1");

        // Then: sess-1 is cancelled but sess-2 is not
        let token_1 = state
            .cancel_token_for("sess-1")
            .expect("sess-1 should be registered");
        let token_2 = state
            .cancel_token_for("sess-2")
            .expect("sess-2 should be registered");

        assert!(token_1.is_cancelled(), "sess-1 token should be cancelled");
        assert!(
            !token_2.is_cancelled(),
            "sess-2 token must NOT be cancelled"
        );
    }

    #[test]
    fn test_cancel_all_cancels_every_active_session() {
        // Given: two registered sessions
        let state = AppState::new();
        state.register_session("sess-1".to_string());
        state.register_session("sess-2".to_string());

        // When: cancel_all_sessions
        state.cancel_all_sessions();

        // Then: both tokens are cancelled
        let token_1 = state.cancel_token_for("sess-1").unwrap();
        let token_2 = state.cancel_token_for("sess-2").unwrap();
        assert!(token_1.is_cancelled(), "sess-1 should be cancelled");
        assert!(token_2.is_cancelled(), "sess-2 should be cancelled");
    }

    // ── Option routing ────────────────────────────────────────────────────────

    #[test]
    fn test_respond_to_option_routes_to_correct_session() {
        // Given: two sessions, each waiting for a response
        let state = AppState::new();
        let (responder_1, _) = state.register_session("sess-1".to_string());
        let (responder_2, _) = state.register_session("sess-2".to_string());

        let (tx1, mut rx1) = oneshot::channel::<OptionResult>();
        let (tx2, mut rx2) = oneshot::channel::<OptionResult>();
        *responder_1.lock().unwrap_or_else(|e| panic!("{e}")) = Some(tx1);
        *responder_2.lock().unwrap_or_else(|e| panic!("{e}")) = Some(tx2);

        // When: respond to sess-2 only
        let sent = state.respond_to_option("sess-2", make_option_result("step-b"));

        // Then: sent is true; sess-2 receives the result; sess-1's sender is untouched
        assert!(sent, "expected respond_to_option to return true");
        let result = rx2
            .try_recv()
            .expect("sess-2 should have received a result");
        assert_eq!(result.next_step.as_deref(), Some("step-b"));
        assert!(
            rx1.try_recv().is_err(),
            "sess-1 should NOT have received a result"
        );
    }

    #[test]
    fn test_respond_to_option_returns_false_for_unknown_session() {
        // Given: a fresh state with no sessions
        let state = AppState::new();
        // When: respond to an unregistered session
        let sent = state.respond_to_option("nonexistent", make_option_result("step"));
        // Then: returns false (silently)
        assert!(!sent, "expected false for unknown session");
    }

    #[test]
    fn test_respond_to_option_returns_false_when_no_pending_response() {
        // Given: a registered session with no pending sender
        let state = AppState::new();
        state.register_session("sess-1".to_string()); // responder slot is None
        // When: respond
        let sent = state.respond_to_option("sess-1", make_option_result("step"));
        // Then: returns false (nothing to resolve)
        assert!(!sent, "expected false when no pending sender");
    }

    // ── Session unregistration ────────────────────────────────────────────────

    #[test]
    fn test_unregister_session_removes_it_from_active_map() {
        // Given: one registered session
        let state = AppState::new();
        state.register_session("sess-1".to_string());
        assert_eq!(state.active_session_count(), 1);

        // When: unregister
        state.unregister_session("sess-1");

        // Then: no active sessions
        assert_eq!(state.active_session_count(), 0);
    }

    #[test]
    fn test_unregister_nonexistent_session_is_idempotent() {
        // Given: a fresh state
        let state = AppState::new();
        // When: unregister a session that was never registered
        state.unregister_session("nonexistent"); // must not panic
        // Then: still 0 sessions
        assert_eq!(state.active_session_count(), 0);
    }

    #[test]
    fn test_cancel_token_for_unregistered_session_returns_none() {
        // Given: a fresh state
        let state = AppState::new();
        // When: query cancel token for a non-registered session
        let token = state.cancel_token_for("nonexistent");
        // Then: None
        assert!(token.is_none(), "expected None for unregistered session");
    }

    #[test]
    fn test_cancel_session_on_unregistered_is_idempotent() {
        // Given: a fresh state
        let state = AppState::new();
        // When: cancel a session that was never registered — must not panic
        state.cancel_session("nonexistent");
        // Then: still 0 sessions
        assert_eq!(state.active_session_count(), 0);
    }
}
