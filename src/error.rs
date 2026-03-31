use std::fmt::Write as _;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum CruiseError {
    #[error("config file not found: {0}")]
    ConfigNotFound(String),

    #[error("failed to parse config file: {0}")]
    ConfigParseError(String),

    #[error("step not found: {0}")]
    StepNotFound(String),

    #[error("invalid step config: {0}")]
    InvalidStepConfig(String),

    #[error("undefined variable: {{{0}}}")]
    UndefinedVariable(String),

    #[error("command error: {0}")]
    CommandError(String),

    #[error("process spawn error: {0}")]
    ProcessSpawnError(String),

    #[error("loop protection: edge {from} -> {to} exceeded max retries {max_retries}")]
    LoopProtection {
        from: String,
        to: String,
        max_retries: usize,
        /// All edge traversal counts at the time of the error, sorted by count descending.
        edge_counts: Vec<(String, String, usize)>,
    },

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("not a git repository")]
    NotGitRepository,

    #[error("git worktree error: {0}")]
    WorktreeError(String),

    #[error("session error: {0}")]
    SessionError(String),

    #[error("session state.json changed externally during run: {0}")]
    SessionStateConflict(String),

    #[error("run aborted to preserve external session state: {0}")]
    SessionStateConflictAborted(String),

    #[error("step '{0}' made no workspace file changes (fail-if-no-file-changes)")]
    StepMadeNoFileChanges(String),

    #[error("interrupted by user (Ctrl+C)")]
    Interrupted,

    #[error("{0}")]
    Other(String),

    #[error("step paused by user interrupt")]
    StepPaused,
}

pub type Result<T> = std::result::Result<T, CruiseError>;

impl CruiseError {
    /// Returns a detailed error message with additional diagnostic context.
    ///
    /// For `LoopProtection`, includes the full edge traversal count table.
    /// For all other variants, falls back to the standard `Display` output.
    #[must_use]
    pub fn detailed_message(&self) -> String {
        match self {
            CruiseError::LoopProtection {
                from,
                to,
                max_retries,
                edge_counts,
            } => {
                let mut msg = format!(
                    "loop protection: edge {from} -> {to} exceeded max retries {max_retries}"
                );
                if !edge_counts.is_empty() {
                    msg.push_str("\n  edge counts:");
                    for (f, t, c) in edge_counts {
                        let _ = write!(msg, "\n    {f} -> {t}: {c}");
                    }
                }
                msg
            }
            other => other.to_string(),
        }
    }
}
