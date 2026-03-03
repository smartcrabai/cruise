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

    #[error("failed to read variable file: {0}: {1}")]
    VariableFileReadError(String, String),

    #[error("command error: {0}")]
    CommandError(String),

    #[error("process spawn error: {0}")]
    ProcessSpawnError(String),

    #[error("loop protection: edge {0} -> {1} exceeded max retries {2}")]
    LoopProtection(String, String, usize),

    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CruiseError>;
