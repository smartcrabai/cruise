use std::{
    collections::{HashMap, HashSet},
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
    /// Create a new [`PerSessionState`] using a caller-supplied cancellation token.
    #[must_use]
    pub fn with_cancel_token(cancel_token: CancellationToken) -> Self {
        Self {
            cancel_token,
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
#[derive(Clone)]
pub struct AppState {
    /// Active sessions keyed by session ID.
    ///
    /// A session is present in this map while it is in the `Running` phase.
    /// It is removed when the session finishes (Completed / Failed / Cancelled).
    pub sessions: Arc<Mutex<HashMap<String, PerSessionState>>>,
    /// Shared cancellation token for an active Run All batch, if any.
    pub batch_cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    /// Session IDs that currently have a plan-fix request in flight.
    ///
    /// Tracked in memory (not persisted to disk) so that stale flags never
    /// survive an app restart.  A fix that was interrupted by a crash will
    /// therefore show up as `fix_in_progress: false` on the next launch.
    pub fixing_sessions: Arc<Mutex<HashSet<String>>>,
}

impl AppState {
    /// Create a new [`AppState`] with no active sessions.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            batch_cancel_token: Arc::new(Mutex::new(None)),
            fixing_sessions: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, PerSessionState>> {
        self.sessions
            .lock()
            .unwrap_or_else(|e| panic!("sessions mutex poisoned: {e}"))
    }

    fn lock_batch_token(&self) -> std::sync::MutexGuard<'_, Option<CancellationToken>> {
        self.batch_cancel_token
            .lock()
            .unwrap_or_else(|e| panic!("batch_cancel_token mutex poisoned: {e}"))
    }

    fn fixing_set(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.fixing_sessions
            .lock()
            .unwrap_or_else(|e| panic!("fixing_sessions mutex poisoned: {e}"))
    }

    /// Record that a plan-fix request has started for `session_id`.
    pub fn start_fixing(&self, session_id: &str) {
        self.fixing_set().insert(session_id.to_owned());
    }

    /// Return a snapshot of session IDs that currently have a fix in flight.
    ///
    /// Acquires the lock once so that callers iterating over many sessions
    /// avoid N separate lock acquisitions.
    pub fn snapshot_fixing(&self) -> HashSet<String> {
        self.fixing_set().clone()
    }

    /// Clear the in-flight fix flag for `session_id` (idempotent).
    pub fn stop_fixing(&self, session_id: &str) {
        self.fixing_set().remove(session_id);
    }

    /// Return `true` if a plan-fix request is currently in flight for `session_id`.
    pub fn is_fixing(&self, session_id: &str) -> bool {
        self.fixing_set().contains(session_id)
    }


    /// Register a new active session, returning its option-responder and cancellation token.
    ///
    /// Returns `(option_responder, cancel_token)` in a single lock acquisition.
    /// If a session with the same `session_id` was already registered, it is replaced.
    ///
    /// # Panics
    ///
    /// Panics if the inner mutex is poisoned.
    #[cfg(test)]
    pub fn register_session(
        &self,
        session_id: String,
    ) -> (
        Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
        CancellationToken,
    ) {
        let cancel_token = CancellationToken::new();
        let responder = self.register_session_with_token(session_id, cancel_token.clone());
        (responder, cancel_token)
    }

    /// Register a new active session using a caller-supplied cancellation token.
    ///
    /// Returns the session's option-responder slot so the caller can hand it to the
    /// GUI option handler without creating a second session entry.
    ///
    /// # Panics
    ///
    /// Panics if the inner mutex is poisoned.
    pub fn register_session_with_token(
        &self,
        session_id: String,
        cancel_token: CancellationToken,
    ) -> Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> {
        let state = PerSessionState::with_cancel_token(cancel_token);
        let responder = Arc::clone(&state.option_responder);
        self.lock_sessions().insert(session_id, state);
        responder
    }

    /// Return a clone of the cancellation token for an active session.
    ///
    /// Returns `None` if the session is not currently registered (not running).
    #[cfg(test)]
    pub(crate) fn cancel_token_for(&self, session_id: &str) -> Option<CancellationToken> {
        self.lock_sessions()
            .get(session_id)
            .map(|s| s.cancel_token.clone())
    }

    /// Cancel the workflow for a specific session.
    ///
    /// Does nothing if the session is not registered.
    pub fn cancel_session(&self, session_id: &str) {
        if let Some(state) = self.lock_sessions().get(session_id) {
            state.cancel_token.cancel();
        }
    }

    /// Cancel **all** active sessions.
    ///
    /// Used when the user clicks "Cancel" during a Run All batch.
    pub fn cancel_all_sessions(&self) {
        if let Some(token) = self.lock_batch_token().as_ref() {
            token.cancel();
        }
        for state in self.lock_sessions().values() {
            state.cancel_token.cancel();
        }
    }

    /// Register the shared cancellation token for an active Run All batch.
    ///
    /// Replaces any previously registered batch token.
    pub fn register_batch_cancel_token(&self, cancel_token: CancellationToken) {
        *self.lock_batch_token() = Some(cancel_token);
    }

    /// Clear the shared cancellation token for the active Run All batch.
    pub fn clear_batch_cancel_token(&self) {
        *self.lock_batch_token() = None;
    }

    /// Route an option-step response to the correct session by its request/session ID.
    ///
    /// Returns `true` if a pending sender was found and the result was sent.
    /// Returns `false` if the session is not registered or has no pending dialog.
    pub fn respond_to_option(&self, session_id: &str, result: OptionResult) -> bool {
        // Clone the Arc while holding the outer lock, then release it before locking the inner
        // mutex so that register_session / unregister_session are not blocked during send.
        let responder = {
            let sessions = self.lock_sessions();
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
    pub fn unregister_session(&self, session_id: &str) {
        self.lock_sessions().remove(session_id);
    }

    /// Return the number of currently active (registered) sessions.
    #[cfg(test)]
    pub(crate) fn active_session_count(&self) -> usize {
        self.lock_sessions().len()
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

    // -- Initial state ---------------------------------------------------------

    #[test]
    fn test_new_state_has_no_active_sessions() {
        // Given/When: a fresh AppState
        let state = AppState::new();
        // Then: no active sessions
        assert_eq!(state.active_session_count(), 0);
    }

    // -- Session registration --------------------------------------------------

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
    fn test_register_session_with_token_keeps_shared_token_wired_to_session() {
        // Given: a caller-provided token shared with external batch state
        let state = AppState::new();
        let shared = CancellationToken::new();

        // When: the session is registered with that token and then cancelled via AppState
        state.register_session_with_token("sess-1".to_string(), shared.clone());
        state.cancel_session("sess-1");

        // Then: the original shared token is also cancelled
        assert!(
            shared.is_cancelled(),
            "shared token should observe AppState cancellation"
        );
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

    #[test]
    fn test_cancel_all_cancels_registered_batch_token_without_active_sessions() {
        // Given: a Run All batch token is registered before any session is active
        let state = AppState::new();
        let batch = CancellationToken::new();
        state.register_batch_cancel_token(batch.clone());

        // When: cancel_all_sessions is invoked
        state.cancel_all_sessions();

        // Then: the shared batch token is cancelled too
        assert!(batch.is_cancelled(), "batch token should be cancelled");
    }

    // -- Option routing --------------------------------------------------------

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

    // -- Session unregistration ------------------------------------------------

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
        // When: cancel a session that was never registered -- must not panic
        state.cancel_session("nonexistent");
        // Then: still 0 sessions
        assert_eq!(state.active_session_count(), 0);
    }
}
