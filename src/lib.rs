// Core modules exported for the Tauri GUI (src-tauri) and other library consumers.
pub mod cancellation;
pub mod condition;
pub mod config;
pub mod engine;
pub mod error;
pub mod file_tracker;
pub mod llm_api;
pub mod metadata;
pub mod option_handler;
pub mod resolver;
pub mod session;
pub mod step;
pub mod variable;
pub mod workflow;
pub mod workspace;
pub mod worktree;
pub mod worktree_pr;

// Display utilities (available to library consumers) and CLI input handling (crate-internal only).
pub mod display;
pub(crate) mod multiline_input;
pub(crate) mod platform;
mod spinner;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_support;
