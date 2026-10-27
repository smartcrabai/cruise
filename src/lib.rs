// Core modules exported for library consumers.
mod agent_auth;
pub mod app_config;
pub mod application;
pub mod artifacts;
pub mod ask_handler;
pub mod attachments;
pub mod backend;
pub mod batch_run;
pub mod builtin_workflows;
pub mod cancellation;
pub(crate) mod commit_guard;
pub mod condition;
pub mod config;
pub mod configs;
pub mod dag;
pub mod engine;
pub mod error;
pub mod executor;
pub mod file_tracker;
pub mod flags;
pub(crate) mod forge;
pub mod github_review;
pub mod graph;
pub mod issue_publish;
pub mod metadata;
pub mod model_schedule;
pub mod model_selection;
pub mod new_session_draft;
pub mod new_session_history;
pub mod option_handler;
pub mod paths;
pub mod planning;
pub mod pr_merge;
pub mod repo_clone;
pub mod resolver;
pub mod retry;
pub mod sdk_tools;
pub mod session;
pub mod session_config;
pub mod session_edit;
pub mod step;
pub mod telemetry;
pub mod timeout;
#[expect(
    dead_code,
    reason = "pre-tool gate entry point is used only by the runtime binary"
)]
pub mod tool_policy;
pub mod variable;
pub mod workflow;
pub mod workflow_call;
pub mod workflow_cmd;
pub mod workflow_defaults;
pub(crate) mod workflow_packages;
pub mod workspace;
pub mod worktree;
pub mod worktree_pr;
pub mod yaml_metadata;

// Display and CLI runtime utilities (available to library consumers) and CLI input handling (crate-internal only).
pub mod batch_dashboard;
pub mod console_mode;
pub mod display;
pub mod herdr;
pub(crate) mod multiline_input;
pub mod platform;
pub mod run_observer;
mod spinner;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_support;

mod desktop_notifications;
