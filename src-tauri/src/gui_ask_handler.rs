//! GUI implementation of [`cruise::ask_handler::AskHandler`].
//!
//! Mirrors [`crate::gui_option_handler::GuiOptionHandler`]: when the SDK
//! `ask_user` tool fires during plan generation, this handler emits a
//! [`PlanEvent::AskUserRequired`] to the frontend and blocks the calling thread
//! until the frontend replies via the `respond_to_ask` command.
//!
//! The tool closure runs on seher's dedicated pi worker thread, so `blocking_recv`
//! here does not starve the async runtime that drives the plan command.

use cruise::ask_handler::AskHandler;
use cruise::error::{CruiseError, Result};
use cruise::session::{SessionManager, SessionPhase};
use tokio::sync::oneshot;

use crate::events::PlanEvent;
use crate::state::AskResponder;

/// Abstraction over the plan-event channel, to allow testing without Tauri.
pub trait PlanEmitter: Send + Sync {
    fn emit(&self, event: PlanEvent);
}

/// Allow `tauri::ipc::Channel<PlanEvent>` to be used directly as a [`PlanEmitter`].
impl PlanEmitter for tauri::ipc::Channel<PlanEvent> {
    fn emit(&self, event: PlanEvent) {
        if let Err(e) = self.send(event) {
            eprintln!("[cruise] PlanEmitter::emit failed: {e}");
        }
    }
}

/// GUI [`AskHandler`]: emits `AskUserRequired` and waits for `respond_to_ask`.
pub struct GuiAskHandler<E: PlanEmitter> {
    emitter: E,
    session_id: String,
    /// Manager used to persist the Awaiting Input phase and pending question.
    manager: SessionManager,
    /// Slot shared with the `respond_to_ask` IPC command (via `AppState`).
    pending: AskResponder,
}

impl<E: PlanEmitter> GuiAskHandler<E> {
    pub fn new(
        emitter: E,
        session_id: String,
        manager: SessionManager,
        pending: AskResponder,
    ) -> Self {
        Self {
            emitter,
            session_id,
            manager,
            pending,
        }
    }
}

impl<E: PlanEmitter> AskHandler for GuiAskHandler<E> {
    fn ask_user(&self, question: &str) -> Result<String> {
        let (tx, rx) = oneshot::channel::<String>();
        {
            let mut guard = self
                .pending
                .lock()
                .map_err(|e| CruiseError::Other(format!("ask_responder lock poisoned: {e}")))?;
            *guard = Some(tx);
        }
        if let Ok(mut state) = self.manager.load(&self.session_id) {
            state.phase = SessionPhase::AwaitingInput;
            state.pending_ask_question = Some(question.to_string());
            let _ = self.manager.save(&state);
        }
        self.emitter.emit(PlanEvent::AskUserRequired {
            session_id: self.session_id.clone(),
            question: question.to_string(),
        });
        rx.blocking_recv().map_err(|_| CruiseError::Interrupted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct RecordingEmitter {
        events: Arc<Mutex<Vec<PlanEvent>>>,
    }

    impl PlanEmitter for RecordingEmitter {
        fn emit(&self, event: PlanEvent) {
            self.events
                .lock()
                .unwrap_or_else(|e| panic!("lock poisoned: {e}"))
                .push(event);
        }
    }

    /// Polls `pending` until a sender is installed, then sends `answer`.
    fn respond_async(pending: AskResponder, answer: String) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            loop {
                let mut guard = pending
                    .lock()
                    .unwrap_or_else(|e| panic!("lock poisoned: {e}"));
                if let Some(sender) = guard.take() {
                    let _ = sender.send(answer);
                    return;
                }
                drop(guard);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })
    }

    #[test]
    fn ask_user_emits_event_and_returns_answer() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let emitter = RecordingEmitter {
            events: Arc::clone(&events),
        };
        let pending: AskResponder = Arc::new(Mutex::new(None));
        let handler = GuiAskHandler::new(emitter, "sess-1".to_string(), Arc::clone(&pending));

        let responder = respond_async(Arc::clone(&pending), "JWT".to_string());
        let answer = handler.ask_user("JWT or sessions?");
        responder.join().unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(answer.unwrap_or_else(|e| panic!("{e}")), "JWT");
        let evs = events.lock().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            PlanEvent::AskUserRequired {
                session_id,
                question,
            } => {
                assert_eq!(session_id, "sess-1");
                assert_eq!(question, "JWT or sessions?");
            }
            other => panic!("expected AskUserRequired, got {other:?}"),
        }
    }

    #[test]
    fn ask_user_returns_interrupted_when_sender_dropped() {
        let emitter = RecordingEmitter {
            events: Arc::new(Mutex::new(Vec::new())),
        };
        let pending: AskResponder = Arc::new(Mutex::new(None));
        let handler = GuiAskHandler::new(emitter, "sess-1".to_string(), Arc::clone(&pending));

        // Drop the sender without sending.
        let dropper = {
            let pending = Arc::clone(&pending);
            std::thread::spawn(move || {
                loop {
                    let mut guard = pending
                        .lock()
                        .unwrap_or_else(|e| panic!("lock poisoned: {e}"));
                    if guard.is_some() {
                        let _ = guard.take();
                        return;
                    }
                    drop(guard);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })
        };
        let answer = handler.ask_user("q");
        dropper.join().unwrap_or_else(|e| panic!("{e:?}"));
        assert!(matches!(answer, Err(CruiseError::Interrupted)));
    }
}
