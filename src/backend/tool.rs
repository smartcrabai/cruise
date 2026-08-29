//! Custom tool definitions (function calling) handed to an SDK backend.
//!
//! A [`CruiseTool`] pairs a JSON Schema with a synchronous handler; the backend
//! adapts it to its own registration path. The tools themselves are built in
//! [`crate::sdk_tools`].

use std::sync::Arc;

/// Synchronous tool handler. Receives the raw JSON input the model produced
/// (validation/parsing is the handler's responsibility). `Ok(text)` becomes the
/// tool result; `Err(message)` is surfaced to the model as an error so it can
/// recover or retry.
///
/// Handlers are invoked from a backend worker thread and may block — see
/// [`crate::ask_handler`], whose `ask_user` implementation waits on the user.
pub type ToolHandler = Arc<dyn Fn(serde_json::Value) -> Result<String, String> + Send + Sync>;

/// A custom tool the model can call: name/description, a JSON Schema
/// (`type: object` with `properties`) describing its input, and the handler
/// invoked with that input.
///
/// Cloning is cheap and shares the handler: all clones invoke the same `Arc`'d
/// closure, so any interior state (the plan-persist flag, the title store) is
/// shared between them.
#[derive(Clone)]
pub struct CruiseTool {
    pub name: String,
    pub description: String,
    /// JSON Schema (`type: object` with `properties`) describing the tool input.
    pub parameters: serde_json::Value,
    pub handler: ToolHandler,
}

impl CruiseTool {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
        handler: ToolHandler,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            handler,
        }
    }
}

impl std::fmt::Debug for CruiseTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CruiseTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .field("parameters", &self.parameters)
            .finish_non_exhaustive()
    }
}
