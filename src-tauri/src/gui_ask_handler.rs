//! GUI implementation of [`cruise::ask_handler::AskHandler`].
//!
//! Mirrors [`crate::gui_option_handler::GuiOptionHandler`]: when the SDK
//! `ask_user` tool fires during plan generation, this handler emits a
//! [`PlanEvent::AskUserRequired`] to the frontend and blocks the calling thread
//! until the frontend replies via the `respond_to_ask` command.
//!
//! Uses [`std::sync::mpsc`] for the answer channel rather than
//! `tokio::sync::oneshot`. The tool closure may run inside a tokio runtime —
//! `seher::claude_agent::stream_agent` drives the CLI on a dedicated
//! current-thread runtime, and the toolbox handler is awaited inline on that
//! runtime — so `tokio::sync::oneshot::Receiver::blocking_recv()` would panic
//! with "Cannot block the current thread from within a runtime". `std::sync::mpsc`
//! has no tokio thread-local dependency and parks the OS thread directly,
//! which is what we want here: the runtime has nothing else useful to do until
//! the user answers.

use cruise::ask_handler::AskHandler;
use cruise::error::{CruiseError, Result};
use cruise::session::{SessionManager, SessionPhase};

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
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        {
            let mut guard = self
                .pending
                .lock()
                .map_err(|e| CruiseError::Other(format!("ask_responder lock poisoned: {e}")))?;
            *guard = Some(tx);
        }
        match self.manager.load(&self.session_id) {
            Ok(mut s)
                if matches!(
                    s.phase,
                    SessionPhase::Draft
                        | SessionPhase::AwaitingInput
                        | SessionPhase::AwaitingApproval
                ) =>
            {
                s.phase = SessionPhase::AwaitingInput;
                s.pending_ask_question = Some(question.to_string());
                if let Err(e) = self.manager.save(&s) {
                    eprintln!(
                        "[cruise] warn: failed to save ask state for {}: {e}",
                        self.session_id
                    );
                }
            }
            Ok(_) => {
                // Session has already terminated (Failed/Cancelled/Suspended/etc.);
                // don't resurrect it.
            }
            Err(e) => {
                eprintln!(
                    "[cruise] warn: failed to load session {} to set ask state: {e}",
                    self.session_id
                );
            }
        }
        self.emitter.emit(PlanEvent::AskUserRequired {
            session_id: self.session_id.clone(),
            question: question.to_string(),
        });
        let answer = rx.recv().map_err(|_| CruiseError::Interrupted)?;
        // Clear persisted ask state now that the answer has been received.
        // This runs on the agent thread before the caller can advance the phase,
        // eliminating the lost-update race that would occur if respond_to_ask_impl
        // attempted a concurrent load-modify-save.
        match self.manager.load(&self.session_id) {
            Ok(mut s) if matches!(s.phase, SessionPhase::AwaitingInput) => {
                s.pending_ask_question = None;
                if let Err(e) = self.manager.save(&s) {
                    eprintln!(
                        "[cruise] warn: failed to clear ask state for {}: {e}",
                        self.session_id
                    );
                }
            }
            _ => {}
        }
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cruise::session::SessionState;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

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
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = tmp.path().join(".cruise");
        let manager = SessionManager::new(cruise_dir.clone());
        let session = SessionState::new(
            "sess-1".to_string(),
            tmp.path().to_path_buf(),
            "test".to_string(),
            "test input".to_string(),
        );
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        let events = Arc::new(Mutex::new(Vec::new()));
        let emitter = RecordingEmitter {
            events: Arc::clone(&events),
        };
        let pending: AskResponder = Arc::new(Mutex::new(None));
        let handler =
            GuiAskHandler::new(emitter, "sess-1".to_string(), manager, Arc::clone(&pending));

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
        drop(evs);
        let saved = SessionManager::new(cruise_dir)
            .load("sess-1")
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(saved.phase, SessionPhase::AwaitingInput);
        // ask_user clears pending_ask_question after the answer is received.
        assert_eq!(saved.pending_ask_question, None);
    }

    /// Regression: `ask_user` must not panic when invoked from inside a tokio
    /// runtime. `seher::claude_agent::stream_agent` drives the CLI on a
    /// dedicated current-thread runtime and awaits its toolbox handler inline
    /// on that runtime; the previous implementation called
    /// `tokio::sync::oneshot::Receiver::blocking_recv()` from the tool closure,
    /// which panics with "Cannot block the current thread from within a
    /// runtime" in that context. The user-visible symptom was that the
    /// planning task aborted before the user answered, the `GuiPlanCtx` was
    /// dropped, the ask slot got unregistered, and the frontend's eventual
    /// `respond_to_ask` failed with "no pending ask_user".
    #[test]
    fn ask_user_does_not_panic_inside_tokio_runtime() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = tmp.path().join(".cruise");
        let manager = SessionManager::new(cruise_dir);
        let session = SessionState::new(
            "sess-1".to_string(),
            tmp.path().to_path_buf(),
            "test".to_string(),
            "test input".to_string(),
        );
        manager.create(&session).unwrap_or_else(|e| panic!("{e}"));
        let events = Arc::new(Mutex::new(Vec::new()));
        let emitter = RecordingEmitter {
            events: Arc::clone(&events),
        };
        let pending: AskResponder = Arc::new(Mutex::new(None));
        let handler =
            GuiAskHandler::new(emitter, "sess-1".to_string(), manager, Arc::clone(&pending));

        // Sender thread lives outside the runtime so the runtime can block.
        let responder = respond_async(Arc::clone(&pending), "JWT".to_string());

        // Mimic claude_agent: a dedicated thread running a current-thread tokio
        // runtime whose spawned task drives the sync tool handler inline (the
        // toolbox's `handle` awaits `(tool.handler)(args)` synchronously).
        let agent_thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap_or_else(|e| panic!("{e}"));
            rt.block_on(async move {
                tokio::spawn(async move { handler.ask_user("q?") })
                    .await
                    .unwrap_or_else(|e| panic!("agent task panicked: {e}"))
            })
        });

        let answer = agent_thread
            .join()
            .unwrap_or_else(|e| panic!("agent thread panicked: {e:?}"));
        responder.join().unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(answer.unwrap_or_else(|e| panic!("{e}")), "JWT");
    }

    #[test]
    fn ask_user_returns_interrupted_when_sender_dropped() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let emitter = RecordingEmitter {
            events: Arc::new(Mutex::new(Vec::new())),
        };
        let pending: AskResponder = Arc::new(Mutex::new(None));
        let handler =
            GuiAskHandler::new(emitter, "sess-1".to_string(), manager, Arc::clone(&pending));

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
