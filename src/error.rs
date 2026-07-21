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

    #[error("session error: {0}")]
    SessionError(String),

    #[error("session state.json changed externally during run: {0}")]
    SessionStateConflict(String),

    #[error("run aborted to preserve external session state: {0}")]
    SessionStateConflictAborted(String),

    #[error("step '{0}' made no workspace file changes (fail-if-no-file-changes)")]
    StepMadeNoFileChanges(String),

    #[error("step '{step}' timed out after {after_secs}s")]
    StepTimeout { step: String, after_secs: u64 },

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
}
