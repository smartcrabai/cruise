use cruise::application::CruiseApplication;

/// Managed Tauri state. Operation claims, cancellation tokens, pending prompt
/// requests, and batch reservations all live in the shared application runtime;
/// this type intentionally contains no second registry.
#[derive(Clone, Debug)]
pub struct AppState {
    pub application: CruiseApplication,
}

impl AppState {
    #[must_use]
    pub fn new(application: CruiseApplication) -> Self {
        Self { application }
    }
}
