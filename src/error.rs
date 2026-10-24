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

    #[error("{0}")]
    MissingJcodeModel(String),

    #[error("undefined variable: {{{0}}}")]
    UndefinedVariable(String),

    #[error("empty variable reference: {{}}")]
    EmptyVariableReference,

    #[error("invalid template syntax: {0} (use `{{{{` / `}}}}` to escape a literal brace)")]
    InvalidTemplateSyntax(String),

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
        /// Subset of the caller's user-skipped steps referenced by `from`, `to`, or
        /// any `edge_counts` entry.
        skipped_steps: Vec<String>,
    },

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("not a git repository")]
    NotGitRepository,

    #[error("git worktree error: {0}")]
    WorktreeError(String),

    #[error("session busy: {0}")]
    Busy(String),

    #[error("session error: {0}")]
    SessionError(String),

    #[error("session state.json changed externally during run: {0}")]
    SessionStateConflict(String),

    #[error("run aborted to preserve external session state: {0}")]
    SessionStateConflictAborted(String),

    #[error("step '{0}' made no workspace file changes (if.no-file-changes: failed)")]
    StepMadeNoFileChanges(String),

    #[error("step '{step}' timed out after {after_secs}s")]
    StepTimeout { step: String, after_secs: u64 },

    #[error("github-review step '{step}' failed: {detail}")]
    GitHubReviewFailed { step: String, detail: String },

    #[error("interrupted by user (Ctrl+C)")]
    Interrupted,
    #[error("automatic squash merge failed: {0}")]
    SquashMergeFailed(String),

    #[error("commit guard violation: {0}")]
    CommitGuardViolation(String),

    #[error("read-only step changed the workspace")]
    ReadOnlyWorkspaceChanged,

    #[error("{0}")]
    Other(String),

    #[error("step paused by user interrupt")]
    StepPaused,
}

pub type Result<T> = std::result::Result<T, CruiseError>;

impl CruiseError {
    /// Returns a stable, payload-free category for telemetry.
    #[must_use]
    pub const fn telemetry_type(&self) -> &'static str {
        match self {
            Self::ConfigNotFound(_) => "config_not_found",
            Self::ConfigParseError(_) => "config_parse_error",
            Self::StepNotFound(_) => "step_not_found",
            Self::InvalidStepConfig(_) => "invalid_step_config",
            Self::MissingJcodeModel(_) => "model_unavailable",
            Self::UndefinedVariable(_) => "undefined_variable",
            Self::EmptyVariableReference => "empty_variable_reference",
            Self::InvalidTemplateSyntax(_) => "invalid_template_syntax",
            Self::CommandError(_) => "command_error",
            Self::ProcessSpawnError(_) => "process_spawn_error",
            Self::LoopProtection { .. } => "loop_protection",
            Self::IoError(_) => "io_error",
            Self::SerializationError(_) => "serialization_error",
            Self::NotGitRepository => "not_git_repository",
            Self::WorktreeError(_) => "worktree_error",
            Self::Busy(_) => "session_busy",
            Self::SessionError(_) => "session_error",
            Self::SessionStateConflict(_) => "session_state_conflict",
            Self::SessionStateConflictAborted(_) => "session_state_conflict_aborted",
            Self::StepMadeNoFileChanges(_) => "step_no_file_changes",
            Self::StepTimeout { .. } => "timeout",
            Self::GitHubReviewFailed { .. } => "github_review_failed",
            Self::Interrupted => "cancelled",
            Self::SquashMergeFailed(_) => "squash_merge_failed",
            Self::CommitGuardViolation(_) => "commit_guard_violation",
            Self::ReadOnlyWorkspaceChanged => "read_only_workspace_changed",
            Self::Other(_) => "other",
            Self::StepPaused => "paused",
        }
    }

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
                skipped_steps,
            } => {
                let mut annotated_any = false;
                let mut annotate = |name: &str| -> String {
                    if skipped_steps.iter().any(|s| s == name) {
                        annotated_any = true;
                        format!("{name} (skipped)")
                    } else {
                        name.to_string()
                    }
                };
                let mut msg = format!(
                    "loop protection: edge {} -> {} exceeded max retries {max_retries}",
                    annotate(from),
                    annotate(to)
                );
                if !edge_counts.is_empty() {
                    msg.push_str("\n  edge counts:");
                    for (f, t, c) in edge_counts {
                        let _ = write!(msg, "\n    {} -> {}: {c}", annotate(f), annotate(t));
                    }
                }
                if annotated_any {
                    msg.push_str(
                        "\n  note: skip suppresses only the step's own execution; transitions into a skipped step still count toward loop protection",
                    );
                }
                msg
            }
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detailed_message_annotates_skipped_steps() {
        // Given: a LoopProtection error where "only-english" is a user-skipped step
        // that is also the loop's edge target and appears in the edge-counts table.
        let err = CruiseError::LoopProtection {
            from: "test".to_string(),
            to: "only-english".to_string(),
            max_retries: 3,
            edge_counts: vec![
                ("test".to_string(), "only-english".to_string(), 4),
                ("a".to_string(), "test".to_string(), 3),
            ],
            skipped_steps: vec!["only-english".to_string()],
        };

        // When: rendering the detailed message
        let msg = err.detailed_message();

        // Then: the headline marks the skipped step
        assert!(
            msg.contains("test -> only-english (skipped) exceeded"),
            "expected headline to annotate the skipped step, got: {msg}"
        );
        // And: the matching edge-counts row is annotated too
        assert!(
            msg.contains("test -> only-english (skipped): 4"),
            "expected edge-counts row to annotate the skipped step, got: {msg}"
        );
        // And: the untouched row (no skipped step involved) stays bare
        assert!(
            msg.contains("a -> test: 3"),
            "expected untouched row to remain unannotated, got: {msg}"
        );
        // And: the note explaining skip semantics appears exactly once
        let note_count = msg
            .matches("note: skip suppresses only the step's own execution")
            .count();
        assert_eq!(
            note_count, 1,
            "expected exactly one note line, got {note_count} in: {msg}"
        );
    }

    #[test]
    fn test_detailed_message_annotates_skipped_step_when_it_is_the_from_side() {
        // Given: the skipped step is the edge's `from` side rather than `to`
        let err = CruiseError::LoopProtection {
            from: "only-english".to_string(),
            to: "test".to_string(),
            max_retries: 3,
            edge_counts: vec![("only-english".to_string(), "test".to_string(), 4)],
            skipped_steps: vec!["only-english".to_string()],
        };

        // When: rendering the detailed message
        let msg = err.detailed_message();

        // Then: the headline and edge-counts row both annotate the `from` side
        assert!(
            msg.contains("only-english (skipped) -> test exceeded"),
            "expected headline to annotate the skipped `from` step, got: {msg}"
        );
        assert!(
            msg.contains("only-english (skipped) -> test: 4"),
            "expected edge-counts row to annotate the skipped `from` step, got: {msg}"
        );
    }

    #[test]
    fn test_detailed_message_no_skipped_steps_is_unannotated() {
        // Given: the same LoopProtection scenario but nothing was user-skipped
        let err = CruiseError::LoopProtection {
            from: "test".to_string(),
            to: "only-english".to_string(),
            max_retries: 3,
            edge_counts: vec![
                ("test".to_string(), "only-english".to_string(), 4),
                ("a".to_string(), "test".to_string(), 3),
            ],
            skipped_steps: vec![],
        };

        // When: rendering the detailed message
        let msg = err.detailed_message();

        // Then: no "(skipped)" annotation appears anywhere
        assert!(
            !msg.contains("(skipped)"),
            "expected no annotation when nothing is skipped, got: {msg}"
        );
        // And: no note line is appended
        assert!(
            !msg.contains("note:"),
            "expected no note line when nothing is skipped, got: {msg}"
        );
        // And: the message matches the historical (pre-annotation) format exactly,
        // guarding backward compatibility of the edge-counts table.
        assert_eq!(
            msg,
            "loop protection: edge test -> only-english exceeded max retries 3\n  edge counts:\n    test -> only-english: 4\n    a -> test: 3"
        );
    }

    #[test]
    fn test_display_ignores_skipped_steps() {
        // Given: a LoopProtection error whose edge target is user-skipped
        let err = CruiseError::LoopProtection {
            from: "test".to_string(),
            to: "only-english".to_string(),
            max_retries: 3,
            edge_counts: vec![],
            skipped_steps: vec!["only-english".to_string()],
        };

        // When / Then: Display (to_string()) stays the stable, unannotated form
        assert_eq!(
            err.to_string(),
            "loop protection: edge test -> only-english exceeded max retries 3"
        );
    }

    #[test]
    fn telemetry_type_is_fixed_and_does_not_expose_error_payloads() {
        let cases = [
            (
                CruiseError::ConfigNotFound("secret".into()),
                "config_not_found",
            ),
            (
                CruiseError::ConfigParseError("secret".into()),
                "config_parse_error",
            ),
            (CruiseError::StepNotFound("secret".into()), "step_not_found"),
            (
                CruiseError::InvalidStepConfig("secret".into()),
                "invalid_step_config",
            ),
            (
                CruiseError::MissingJcodeModel("secret".into()),
                "model_unavailable",
            ),
            (
                CruiseError::UndefinedVariable("secret".into()),
                "undefined_variable",
            ),
            (
                CruiseError::EmptyVariableReference,
                "empty_variable_reference",
            ),
            (
                CruiseError::InvalidTemplateSyntax("secret".into()),
                "invalid_template_syntax",
            ),
            (CruiseError::CommandError("secret".into()), "command_error"),
            (
                CruiseError::ProcessSpawnError("secret".into()),
                "process_spawn_error",
            ),
            (
                CruiseError::LoopProtection {
                    from: "secret".into(),
                    to: "secret".into(),
                    max_retries: 1,
                    edge_counts: vec![],
                    skipped_steps: vec![],
                },
                "loop_protection",
            ),
            (
                CruiseError::IoError(std::io::Error::other("secret")),
                "io_error",
            ),
            (
                CruiseError::SerializationError(
                    match serde_json::from_str::<serde_json::Value>("{") {
                        Ok(_) => panic!("invalid JSON unexpectedly parsed"),
                        Err(error) => error,
                    },
                ),
                "serialization_error",
            ),
            (CruiseError::NotGitRepository, "not_git_repository"),
            (
                CruiseError::WorktreeError("secret".into()),
                "worktree_error",
            ),
            (CruiseError::Busy("secret".into()), "session_busy"),
            (CruiseError::SessionError("secret".into()), "session_error"),
            (
                CruiseError::SessionStateConflict("secret".into()),
                "session_state_conflict",
            ),
            (
                CruiseError::SessionStateConflictAborted("secret".into()),
                "session_state_conflict_aborted",
            ),
            (
                CruiseError::StepMadeNoFileChanges("secret".into()),
                "step_no_file_changes",
            ),
            (
                CruiseError::StepTimeout {
                    step: "secret".into(),
                    after_secs: 1,
                },
                "timeout",
            ),
            (
                CruiseError::GitHubReviewFailed {
                    step: "secret".into(),
                    detail: "secret".into(),
                },
                "github_review_failed",
            ),
            (CruiseError::Interrupted, "cancelled"),
            (
                CruiseError::SquashMergeFailed("secret".into()),
                "squash_merge_failed",
            ),
            (
                CruiseError::CommitGuardViolation("secret".into()),
                "commit_guard_violation",
            ),
            (
                CruiseError::ReadOnlyWorkspaceChanged,
                "read_only_workspace_changed",
            ),
            (CruiseError::Other("secret".into()), "other"),
            (CruiseError::StepPaused, "paused"),
        ];

        for (error, expected) in cases {
            assert_eq!(error.telemetry_type(), expected);
            assert_ne!(error.telemetry_type(), "secret");
        }
    }
}
