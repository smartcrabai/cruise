// Core modules exported for library consumers.
mod agent_auth;
pub mod app_config;
pub mod application;
pub mod artifacts;
pub mod ask_handler;
pub mod attachments;
pub mod auto_requeue;
#[cfg_attr(
    all(target_os = "linux", feature = "jcode-bubblewrap"),
    expect(
        dead_code,
        unused_imports,
        reason = "CLI-only Bubblewrap relay and verifier"
    )
)]
pub mod backend;
mod base_ci;
pub mod branch_diff;
pub mod builtin_workflows;
pub mod cancellation;
#[cfg(test)]
#[path = "worktree_pr/ci_followup.rs"]
mod ci_followup;
pub(crate) mod commit_guard;
pub mod condition;
pub mod config;
pub mod config_env;
pub mod configs;
pub mod dag;
pub mod engine;
pub mod error;
pub mod executor;
pub mod file_tracker;
pub mod flags;
pub(crate) mod forge;
pub mod formal_verify;
pub mod github_review;
pub mod graph;
#[cfg(test)]
#[path = "worktree_pr/host_sync.rs"]
mod host_sync;
#[expect(
    dead_code,
    reason = "the binary-only updater also uses these shared download helpers"
)]
mod http_download;
pub mod i18n;
pub mod instruction_inbox;
pub(crate) mod issue_images;
pub(crate) mod issue_input;
pub mod issue_publish;
#[cfg(test)]
mod ledger_snapshot;
pub mod local_merge;
pub mod metadata;
pub mod model_schedule;
pub mod model_selection;
pub mod new_session_draft;
pub mod new_session_history;
pub mod option_handler;
pub mod paths;
pub mod planning;
pub mod pr_merge;
pub mod pr_review_fix;
mod pr_template;
pub mod prompt_render;
pub mod provider_install;
pub mod quality_gate;
pub mod repo_clone;
pub mod request_context;
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
pub mod usage;
pub mod usage_cmd;
pub mod variable;
pub mod workflow;
mod workflow_validate;
pub mod workflow_generate {
    pub use crate::workflow_validate::validate_generated_yaml;
}
pub mod workflow_call;
pub mod workflow_categories;
pub mod workflow_cmd;
pub mod workflow_defaults;
pub(crate) mod workflow_packages;
pub mod workspace;
pub mod worktree;
#[expect(
    dead_code,
    reason = "the CLI uses the pinned review and base-sync entry points"
)]
pub mod worktree_pr;
pub mod yaml_metadata;

// Display and CLI runtime utilities (available to library consumers) and CLI input handling (crate-internal only).
pub mod batch_dashboard;
pub mod console_mode;
pub mod display;
pub mod herdr;
pub(crate) mod multiline_input;
#[expect(
    dead_code,
    reason = "the daemon login opener is used only by the runtime binary"
)]
pub mod platform;
pub mod run_observer;
pub mod sleep_guard;
mod spinner;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_support;

#[expect(
    dead_code,
    reason = "the library registers the binary-facing notification helper module"
)]
mod desktop_notifications;
mod notification_sound;
mod notification_webhook;
