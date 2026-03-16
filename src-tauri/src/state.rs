use std::sync::{Arc, Mutex};

use cruise::cancellation::CancellationToken;
use cruise::step::option::OptionResult;
use tokio::sync::oneshot;

/// Shared Tauri application state, injected into command handlers via `tauri::State`.
pub struct AppState {
    /// [`CancellationToken`] for the currently running workflow.
    /// `None` when no workflow is executing.
    pub cancel_token: Mutex<Option<CancellationToken>>,
    /// Oneshot sender slot shared with [`crate::gui_option_handler::GuiOptionHandler`].
    /// Populated by `select_option` and consumed by `respond_to_option`.
    pub option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
}

impl AppState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            cancel_token: Mutex::new(None),
            option_responder: Arc::new(Mutex::new(None)),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
