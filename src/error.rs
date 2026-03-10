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

    #[error("loop protection: edge {0} -> {1} exceeded max retries {2}")]
    LoopProtection(String, String, usize),

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

    #[error("step '{0}' made no tracked file changes (fail-if-no-file-changes)")]
    StepMadeNoFileChanges(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CruiseError>;
