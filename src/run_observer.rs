/// Coarse lifecycle phases shown by the live `run --all` dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPhase {
    Preparing,
    Step(String),
    CreatingPr,
    WaitingInput,
}

/// Receives lifecycle updates from a single session run.
pub trait RunObserver: Send + Sync {
    fn on_phase(&self, session_id: &str, phase: RunPhase);
}

/// Observer used by ordinary single-session runs.
pub struct NoopObserver;

impl RunObserver for NoopObserver {
    fn on_phase(&self, _session_id: &str, _phase: RunPhase) {}
}
