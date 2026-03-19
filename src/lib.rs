// Core modules exported for the Tauri GUI (src-tauri) and other library consumers.
pub mod cancellation;
pub mod condition;
pub mod config;
pub mod engine;
pub mod error;
pub mod file_tracker;
pub mod option_handler;
pub mod resolver;
pub mod session;
pub mod step;
pub mod variable;
pub mod workflow;
pub mod worktree;

// Display utilities and input handling, available to library consumers.
pub mod display;
pub mod multiline_input;
pub(crate) mod platform;
mod spinner;

#[cfg(test)]
mod test_support;
