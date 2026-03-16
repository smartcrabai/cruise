use std::sync::{Arc, Mutex};

use cruise::cancellation::CancellationToken;
use cruise::step::option::OptionResult;
use tokio::sync::oneshot;

/// Inner logic for the `cancel_session` IPC command.
///
/// Extracted from the Tauri command handler for testability.
/// Calls `cancel()` on the active token if one is present; succeeds silently if not.
/// The token is removed from the slot after cancellation to free the underlying `Arc`.
///
/// # Errors
///
/// Returns an error string if the mutex is poisoned.
pub fn do_cancel_session(
    cancel_token: &Mutex<Option<CancellationToken>>,
) -> std::result::Result<(), String> {
    let mut guard = cancel_token
        .lock()
        .map_err(|e| format!("lock poisoned: {e}"))?;
    if let Some(token) = guard.take() {
        token.cancel();
    }
    Ok(())
}

/// Inner logic for the `respond_to_option` IPC command.
///
/// Extracted from the Tauri command handler for testability.
/// Takes the pending `oneshot::Sender` from `option_responder` and delivers the user's choice.
///
/// # Errors
///
/// Returns an error string if the mutex is poisoned or no option request is currently pending.
pub fn do_respond_to_option(
    option_responder: &Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
    result: OptionResult,
) -> std::result::Result<(), String> {
    let mut guard = option_responder
        .lock()
        .map_err(|e| format!("lock poisoned: {e}"))?;
    let sender = guard
        .take()
        .ok_or_else(|| "no pending option request".to_string())?;
    sender
        .send(result)
        .map_err(|_| "option receiver dropped: response not delivered".to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cruise::cancellation::CancellationToken;

    /// Polls `pending` until a sender is available.
    fn wait_for_pending(pending: &Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>) {
        loop {
            let guard = pending.lock().unwrap_or_else(|e| panic!("{e}"));
            if guard.is_some() {
                return;
            }
            drop(guard);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    // ─── cancel_session ──────────────────────────────────────────────────────

    #[test]
    fn test_cancel_session_with_no_active_token_succeeds() {
        // Given: no active cancellation token in state
        let cancel_token: Mutex<Option<CancellationToken>> = Mutex::new(None);
        // When: cancel is requested
        let result = do_cancel_session(&cancel_token);
        // Then: succeeds without error
        assert!(result.is_ok());
    }

    #[test]
    fn test_cancel_session_with_active_token_cancels_it() {
        // Given: an active token stored in state
        let token = CancellationToken::new();
        let token_for_assert = token.clone();
        let cancel_token: Mutex<Option<CancellationToken>> = Mutex::new(Some(token));
        // When: cancel is requested
        let result = do_cancel_session(&cancel_token);
        // Then: succeeds and the token reports cancelled
        assert!(result.is_ok());
        assert!(token_for_assert.is_cancelled());
    }

    #[test]
    fn test_cancel_session_clears_token_from_slot_after_cancelling() {
        // Given: an active token
        let token = CancellationToken::new();
        let cancel_token: Mutex<Option<CancellationToken>> = Mutex::new(Some(token));
        // When: cancel is requested
        let _ = do_cancel_session(&cancel_token);
        // Then: the token slot is cleared (frees the Arc)
        assert!(
            cancel_token
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none()
        );
    }

    // ─── respond_to_option ───────────────────────────────────────────────────

    #[test]
    fn test_respond_to_option_with_no_pending_request_returns_error() {
        // Given: no pending option request
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(None));
        // When: respond_to_option is called
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // Then: returns an error mentioning no pending request
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_lowercase()
                .contains("no pending option request"),
            "error message should mention 'no pending option request'"
        );
    }

    #[test]
    fn test_respond_to_option_sends_next_step_to_handler() {
        // Given: a pending option request (sender in state)
        let (tx, rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        // When: respond_to_option is called with a next_step choice
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: Some("next_step".to_string()),
                text_input: None,
            },
        );
        // Then: succeeds and the handler receives the next_step
        assert!(result.is_ok());
        let received = rx.blocking_recv().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(received.next_step, Some("next_step".to_string()));
        assert_eq!(received.text_input, None);
    }

    #[test]
    fn test_respond_to_option_sends_text_input_to_handler() {
        // Given: a pending option request
        let (tx, rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        // When: respond_to_option is called with text input
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: Some("my text input".to_string()),
            },
        );
        // Then: the text is delivered to the handler
        assert!(result.is_ok());
        let received = rx.blocking_recv().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(received.text_input, Some("my text input".to_string()));
    }

    #[test]
    fn test_respond_to_option_clears_sender_from_state_after_use() {
        // Given: a pending option request
        let (tx, _rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        // When: respond_to_option is called
        let _ = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // Then: the sender slot is cleared (idempotency guard)
        assert!(
            option_responder
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none()
        );
    }

    #[test]
    fn test_respond_to_option_second_call_returns_error() {
        // Given: a request that was already handled
        let (tx, _rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        let _ = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // When: respond_to_option is called again
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // Then: returns an error (no pending request remains)
        assert!(result.is_err());
    }

    // ─── Integration: full option-selection round-trip ────────────────────────
    //
    // Data flow:
    //   GuiOptionHandler::select_option (engine thread)
    //     → stores sender in shared pending_response slot
    //     → emits WorkflowEvent::OptionRequired
    //   do_respond_to_option (IPC command handler / test thread)
    //     → extracts sender from slot
    //     → sends OptionResult
    //   GuiOptionHandler::select_option (engine thread)
    //     → blocking_recv returns OptionResult
    //
    // Modules covered: events, gui_option_handler, state, commands
    //
    #[test]
    fn test_option_flow_integration_select_and_respond_round_trip() {
        use crate::events::WorkflowEvent;
        use crate::gui_option_handler::{EventEmitter, GuiOptionHandler};
        use cruise::option_handler::OptionHandler;
        use cruise::step::OptionChoice;

        /// Minimal emitter that records the last emitted event.
        struct CapturingEmitter {
            last: Mutex<Option<WorkflowEvent>>,
        }
        impl CapturingEmitter {
            fn new() -> Self {
                Self {
                    last: Mutex::new(None),
                }
            }
        }
        impl EventEmitter for CapturingEmitter {
            fn emit(&self, event: WorkflowEvent) {
                *self.last.lock().unwrap_or_else(|e| panic!("{e}")) = Some(event);
            }
        }

        // Given: a GuiOptionHandler wired to a shared pending_response slot
        let emitter = Arc::new(CapturingEmitter::new());
        let pending: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> = Arc::new(Mutex::new(None));
        let handler = GuiOptionHandler::new(
            Arc::clone(&emitter),
            "integration-req".to_string(),
            Arc::clone(&pending),
        );
        let choices = vec![OptionChoice::Selector {
            label: "Proceed".to_string(),
            next: Some("finalize".to_string()),
        }];

        // When: the engine thread calls select_option (blocks until response)
        let pending_for_cmd = Arc::clone(&pending);
        let engine_thread =
            std::thread::spawn(move || handler.select_option(&choices, Some("plan text")));

        // And: the IPC command thread responds once the sender is populated
        wait_for_pending(&pending_for_cmd);
        do_respond_to_option(
            &pending_for_cmd,
            OptionResult {
                next_step: Some("finalize".to_string()),
                text_input: None,
            },
        )
        .unwrap_or_else(|e| panic!("respond_to_option failed: {e}"));

        // Then: the engine thread receives the OptionResult
        let result = engine_thread
            .join()
            .unwrap_or_else(|e| panic!("engine thread panicked: {e:?}"))
            .unwrap_or_else(|e| panic!("select_option failed: {e}"));
        assert_eq!(result.next_step, Some("finalize".to_string()));
        assert_eq!(result.text_input, None);

        // And: the OptionRequired event was emitted with the correct data
        let emitted = emitter.last.lock().unwrap_or_else(|e| panic!("{e}")).take();
        match emitted {
            Some(WorkflowEvent::OptionRequired {
                request_id,
                plan,
                choices,
            }) => {
                assert_eq!(request_id, "integration-req");
                assert_eq!(plan.as_deref(), Some("plan text"));
                assert_eq!(choices.len(), 1);
                assert_eq!(choices[0].label, "Proceed");
            }
            other => panic!("expected OptionRequired event, got: {other:?}"),
        }
    }
}
