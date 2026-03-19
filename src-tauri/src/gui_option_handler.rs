use std::sync::{Arc, Mutex};

use cruise::error::Result;
use cruise::option_handler::OptionHandler;
use cruise::step::OptionChoice;
use cruise::step::option::OptionResult;
use tokio::sync::oneshot;

use crate::events::{ChoiceDto, ChoiceKind, WorkflowEvent};

/// Abstraction over the event channel to allow testing without Tauri.
///
/// In production, implemented for `tauri::ipc::Channel<WorkflowEvent>`.
/// In tests, implemented for `RecordingEmitter`.
pub trait EventEmitter: Send + Sync {
    fn emit(&self, event: WorkflowEvent);
}

/// GUI implementation of [`OptionHandler`].
///
/// When `select_option` is called by the engine:
/// 1. Stores a `oneshot::Sender` in `pending_response` (shared with `respond_to_option` command).
/// 2. Emits a [`WorkflowEvent::OptionRequired`] to the frontend via the emitter.
/// 3. Blocks the current thread until the frontend responds via `respond_to_option`.
///
/// The engine must be invoked on a blocking thread (e.g. `tokio::task::spawn_blocking`)
/// so that `blocking_recv()` does not starve the async runtime.
pub struct GuiOptionHandler<E: EventEmitter> {
    emitter: Arc<E>,
    request_id: String,
    /// Shared slot between this handler and the `respond_to_option` IPC command.
    pub pending_response: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
}

impl<E: EventEmitter> GuiOptionHandler<E> {
    pub fn new(
        emitter: Arc<E>,
        request_id: String,
        pending_response: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
    ) -> Self {
        Self {
            emitter,
            request_id,
            pending_response,
        }
    }
}

/// Allow `tauri::ipc::Channel<WorkflowEvent>` to be used directly as an [`EventEmitter`].
///
/// The `Channel` is already `Clone + Send + Sync` (internally Arc-backed), so this impl
/// lets `GuiOptionHandler<Channel<WorkflowEvent>>` be constructed in Tauri command handlers.
impl EventEmitter for tauri::ipc::Channel<crate::events::WorkflowEvent> {
    fn emit(&self, event: crate::events::WorkflowEvent) {
        if let Err(e) = self.send(event) {
            eprintln!("[cruise] EventEmitter::emit failed: {e}");
        }
    }
}

fn choices_to_dtos(choices: &[OptionChoice]) -> Vec<ChoiceDto> {
    choices
        .iter()
        .map(|c| match c {
            OptionChoice::Selector { label, next } => ChoiceDto {
                label: label.clone(),
                kind: ChoiceKind::Selector,
                next: next.clone(),
            },
            OptionChoice::TextInput { label, next } => ChoiceDto {
                label: label.clone(),
                kind: ChoiceKind::TextInput,
                next: next.clone(),
            },
        })
        .collect()
}

impl<E: EventEmitter> OptionHandler for GuiOptionHandler<E> {
    fn select_option(&self, choices: &[OptionChoice], plan: Option<&str>) -> Result<OptionResult> {
        let (tx, rx) = oneshot::channel::<OptionResult>();

        {
            let mut guard = self
                .pending_response
                .lock()
                .map_err(|e| cruise::error::CruiseError::Other(format!("lock poisoned: {e}")))?;
            if guard.is_some() {
                return Err(cruise::error::CruiseError::Other(
                    "pending_response slot already occupied — previous request was not consumed"
                        .to_string(),
                ));
            }
            *guard = Some(tx);
        }

        self.emitter.emit(WorkflowEvent::OptionRequired {
            request_id: self.request_id.clone(),
            choices: choices_to_dtos(choices),
            plan: plan.map(str::to_owned),
        });

        rx.blocking_recv()
            .map_err(|_| cruise::error::CruiseError::Interrupted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cruise::error::CruiseError;
    use cruise::step::OptionChoice;

    /// A mock emitter that records emitted events for later inspection.
    struct RecordingEmitter {
        events: Mutex<Vec<WorkflowEvent>>,
    }

    impl RecordingEmitter {
        fn new() -> Self {
            Self {
                events: Mutex::new(Vec::new()),
            }
        }

        fn events(&self) -> Vec<WorkflowEvent> {
            self.events
                .lock()
                .unwrap_or_else(|e| panic!("lock poisoned: {e}"))
                .clone()
        }
    }

    impl EventEmitter for RecordingEmitter {
        fn emit(&self, event: WorkflowEvent) {
            self.events
                .lock()
                .unwrap_or_else(|e| panic!("lock poisoned: {e}"))
                .push(event);
        }
    }

    fn make_handler(
        emitter: Arc<RecordingEmitter>,
        pending: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
    ) -> GuiOptionHandler<RecordingEmitter> {
        GuiOptionHandler::new(emitter, "test-req-id".to_string(), pending)
    }

    /// Spawns a thread that polls `pending` until a sender is available, then sends `result`.
    fn respond_async(
        pending: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
        result: OptionResult,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            loop {
                let mut guard = pending
                    .lock()
                    .unwrap_or_else(|e| panic!("lock poisoned: {e}"));
                if let Some(sender) = guard.take() {
                    let _ = sender.send(result);
                    return;
                }
                drop(guard);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })
    }

    /// Spawns a thread that polls `pending` until a sender is available, then drops it
    /// (simulates a lost connection / cancelled dialog).
    fn drop_sender_async(
        pending: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            loop {
                let mut guard = pending
                    .lock()
                    .unwrap_or_else(|e| panic!("lock poisoned: {e}"));
                if guard.is_some() {
                    let _ = guard.take(); // drop without sending
                    return;
                }
                drop(guard);
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        })
    }

    #[test]
    fn test_select_option_emits_option_required_event() {
        // Given: a handler with a recording emitter
        let emitter = Arc::new(RecordingEmitter::new());
        let pending = Arc::new(Mutex::new(None));
        let handler = make_handler(Arc::clone(&emitter), Arc::clone(&pending));
        let choices = vec![OptionChoice::Selector {
            label: "Yes".to_string(),
            next: Some("done".to_string()),
        }];
        // When: select_option is called (a responder thread unblocks it)
        let responder = respond_async(
            Arc::clone(&pending),
            OptionResult {
                next_step: Some("done".to_string()),
                text_input: None,
            },
        );
        let _ = handler.select_option(&choices, None);
        responder.join().unwrap_or_else(|e| panic!("{e:?}"));
        // Then: exactly one OptionRequired event was emitted
        let events = emitter.events();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], WorkflowEvent::OptionRequired { request_id, .. } if request_id == "test-req-id"),
            "expected OptionRequired with correct request_id"
        );
    }

    #[test]
    fn test_select_option_passes_choices_to_emitted_event() {
        // Given: two choices (selector + text-input)
        let emitter = Arc::new(RecordingEmitter::new());
        let pending = Arc::new(Mutex::new(None));
        let handler = make_handler(Arc::clone(&emitter), Arc::clone(&pending));
        let choices = vec![
            OptionChoice::Selector {
                label: "Option A".to_string(),
                next: Some("step_a".to_string()),
            },
            OptionChoice::TextInput {
                label: "Enter value".to_string(),
                next: None,
            },
        ];
        // When: select_option is called
        let responder = respond_async(
            Arc::clone(&pending),
            OptionResult {
                next_step: None,
                text_input: Some("hello".to_string()),
            },
        );
        let _ = handler.select_option(&choices, None);
        responder.join().unwrap_or_else(|e| panic!("{e:?}"));
        // Then: the emitted event contains both choices with correct kinds
        let events = emitter.events();
        if let WorkflowEvent::OptionRequired { choices: dtos, .. } = &events[0] {
            assert_eq!(dtos.len(), 2);
            assert_eq!(dtos[0].label, "Option A");
            assert_eq!(dtos[0].kind, ChoiceKind::Selector);
            assert_eq!(dtos[1].label, "Enter value");
            assert_eq!(dtos[1].kind, ChoiceKind::TextInput);
        } else {
            panic!("expected OptionRequired event");
        }
    }

    #[test]
    fn test_select_option_passes_plan_to_emitted_event() {
        // Given: a plan string is provided
        let emitter = Arc::new(RecordingEmitter::new());
        let pending = Arc::new(Mutex::new(None));
        let handler = make_handler(Arc::clone(&emitter), Arc::clone(&pending));
        // When: select_option is called with a plan
        let responder = respond_async(
            Arc::clone(&pending),
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        let _ = handler.select_option(&[], Some("## Plan\nStep 1: do X"));
        responder.join().unwrap_or_else(|e| panic!("{e:?}"));
        // Then: the emitted event includes the plan text
        let events = emitter.events();
        if let WorkflowEvent::OptionRequired { plan, .. } = &events[0] {
            assert_eq!(plan.as_deref(), Some("## Plan\nStep 1: do X"));
        } else {
            panic!("expected OptionRequired event");
        }
    }

    #[test]
    fn test_select_option_returns_result_sent_by_responder() {
        // Given: a handler and a responder that sends a specific OptionResult
        let emitter = Arc::new(RecordingEmitter::new());
        let pending = Arc::new(Mutex::new(None));
        let handler = make_handler(Arc::clone(&emitter), Arc::clone(&pending));
        // When: the responder sends next_step = "my_step" with text_input
        let responder = respond_async(
            Arc::clone(&pending),
            OptionResult {
                next_step: Some("my_step".to_string()),
                text_input: Some("user input".to_string()),
            },
        );
        let result = handler.select_option(&[], None);
        responder.join().unwrap_or_else(|e| panic!("{e:?}"));
        // Then: the returned OptionResult matches what the responder sent
        let result = result.unwrap_or_else(|e| panic!("expected Ok, got: {e}"));
        assert_eq!(result.next_step, Some("my_step".to_string()));
        assert_eq!(result.text_input, Some("user input".to_string()));
    }

    #[test]
    fn test_select_option_returns_interrupted_when_sender_is_dropped() {
        // Given: a handler and a thread that drops the sender without responding
        let emitter = Arc::new(RecordingEmitter::new());
        let pending = Arc::new(Mutex::new(None));
        let handler = make_handler(Arc::clone(&emitter), Arc::clone(&pending));
        // When: the sender is dropped (simulates dialog cancel / connection lost)
        let dropper = drop_sender_async(Arc::clone(&pending));
        let result = handler.select_option(&[], None);
        dropper.join().unwrap_or_else(|e| panic!("{e:?}"));
        // Then: select_option returns CruiseError::Interrupted
        match result {
            Err(CruiseError::Interrupted) => {} // expected
            other => panic!("expected Interrupted, got: {other:?}"),
        }
    }
}
