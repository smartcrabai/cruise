//! Server-side mirror of the Run All reducer the React app kept in memory
//! (`ui/src/App.tsx` `handleRunAll`), so a page reload shows live batch state.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use crate::application::ApplicationEvent;

const LOG_CAP: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunAllStatus {
    Idle,
    Running,
    Completed,
    Cancelled,
    Error,
}

impl RunAllStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RunningInfo {
    pub(crate) title: String,
    pub(crate) step: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResultInfo {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) phase: String,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RunAllSnapshot {
    pub(crate) status: RunAllStatus,
    pub(crate) total: usize,
    pub(crate) parallelism: Option<usize>,
    pub(crate) running: BTreeMap<String, RunningInfo>,
    pub(crate) results: Vec<ResultInfo>,
    pub(crate) log: VecDeque<String>,
    pub(crate) run_error: Option<String>,
}

impl Default for RunAllSnapshot {
    fn default() -> Self {
        Self {
            status: RunAllStatus::Idle,
            total: 0,
            parallelism: None,
            running: BTreeMap::new(),
            results: Vec::new(),
            log: VecDeque::new(),
            run_error: None,
        }
    }
}

/// Shared Run All view state updated by the batch event sink.
#[derive(Debug, Default)]
pub(crate) struct RunAllState {
    inner: Mutex<RunAllSnapshot>,
    /// Session titles captured when the batch starts; `BatchSessionStarted`
    /// only carries an id.
    titles: Mutex<BTreeMap<String, String>>,
}

impl RunAllState {
    pub(crate) fn snapshot(&self) -> RunAllSnapshot {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RunAllSnapshot> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reset to a freshly started batch, seeding the titles of the sessions the
    /// batch is about to claim.
    pub(crate) fn start(&self, titles: BTreeMap<String, String>) {
        let mut guard = self.lock();
        *guard = RunAllSnapshot {
            status: RunAllStatus::Running,
            ..RunAllSnapshot::default()
        };
        guard.running.clear();
        drop(guard);
        let mut titles_guard = self
            .titles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *titles_guard = titles;
    }

    /// Apply one batch event, mirroring the React reducer.
    pub(crate) fn apply(&self, event: &ApplicationEvent) {
        let title_of = |id: &str| -> String {
            self.titles
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.to_string())
        };
        let mut guard = self.lock();
        match event {
            ApplicationEvent::BatchStarted { total, parallelism } => {
                guard.total = *total;
                guard.parallelism = Some(*parallelism);
                push_log(
                    &mut guard.log,
                    format!(
                        "--- Run All started ({total} sessions, parallelism: {parallelism}) ---"
                    ),
                );
            }
            ApplicationEvent::BatchTotalChanged { total } => guard.total = *total,
            ApplicationEvent::BatchSessionStarted { id } => {
                let title = title_of(id);
                push_log(&mut guard.log, format!("--- Session: {title} ({id}) ---"));
                guard
                    .running
                    .insert(id.clone(), RunningInfo { title, step: None });
            }
            ApplicationEvent::RunPhase { session_id, phase } => {
                if let Some(running) = guard.running.get_mut(session_id) {
                    running.step = Some(phase.clone());
                }
                push_log(&mut guard.log, format!("[{session_id}] {phase}"));
            }
            ApplicationEvent::StepStarted { session_id, step } => {
                if let Some(running) = guard.running.get_mut(session_id) {
                    running.step = Some(step.clone());
                    push_log(&mut guard.log, format!("[{session_id}] {step}"));
                }
            }
            ApplicationEvent::PrCreated { session_id, url } => {
                push_log(&mut guard.log, format!("[{session_id}] PR created: {url}"));
            }
            ApplicationEvent::BatchSessionFinished { id, phase, error } => {
                let title = guard
                    .running
                    .remove(id)
                    .map_or_else(|| title_of(id), |running| running.title);
                let detail = error
                    .as_ref()
                    .map_or_else(String::new, |error| format!(": {error}"));
                push_log(&mut guard.log, format!("[{id}] {phase}{detail}"));
                guard.results.push(ResultInfo {
                    id: id.clone(),
                    title,
                    phase: phase.clone(),
                    error: error.clone(),
                });
            }
            ApplicationEvent::BatchFinished { cancelled } => {
                guard.status = if *cancelled {
                    RunAllStatus::Cancelled
                } else {
                    RunAllStatus::Completed
                };
                push_log(
                    &mut guard.log,
                    format!(
                        "--- Run All finished (cancelled: {}) ---",
                        usize::from(*cancelled)
                    ),
                );
            }
            ApplicationEvent::BatchFailed { error } => {
                guard.status = RunAllStatus::Error;
                guard.run_error = Some(error.clone());
            }
            ApplicationEvent::LogChunk {
                session_id, text, ..
            } => {
                let line = session_id
                    .as_ref()
                    .map_or_else(|| text.clone(), |id| format!("[{id}] {text}"));
                push_log(&mut guard.log, line);
            }
            _ => {}
        }
    }
}

fn push_log(log: &mut VecDeque<String>, line: String) {
    log.push_back(line);
    while log.len() > LOG_CAP {
        log.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::{RunAllState, RunAllStatus};
    use crate::application::ApplicationEvent;
    use std::collections::BTreeMap;

    #[test]
    fn reducer_tracks_running_results_and_completion() {
        let state = RunAllState::default();
        let mut titles = BTreeMap::new();
        titles.insert("s1".to_string(), "first task".to_string());
        state.start(titles);
        state.apply(&ApplicationEvent::BatchStarted {
            total: 2,
            parallelism: 2,
        });
        state.apply(&ApplicationEvent::BatchSessionStarted {
            id: "s1".to_string(),
        });
        state.apply(&ApplicationEvent::StepStarted {
            session_id: "s1".to_string(),
            step: "implement".to_string(),
        });
        let mid = state.snapshot();
        assert_eq!(mid.total, 2);
        assert_eq!(mid.parallelism, Some(2));
        assert_eq!(mid.running.len(), 1);
        let Some(running) = mid.running.get("s1") else {
            panic!("missing running session");
        };
        assert_eq!(running.title, "first task");
        assert_eq!(running.step.as_deref(), Some("implement"));

        state.apply(&ApplicationEvent::BatchSessionFinished {
            id: "s1".to_string(),
            phase: "Completed".to_string(),
            error: None,
        });
        state.apply(&ApplicationEvent::BatchFinished { cancelled: true });
        let done = state.snapshot();
        assert!(done.running.is_empty());
        assert_eq!(done.results.len(), 1);
        assert_eq!(done.results[0].title, "first task");
        assert_eq!(done.status, RunAllStatus::Cancelled);
        assert!(
            done.log
                .iter()
                .any(|line| line.contains("Run All finished (cancelled: 1)"))
        );
    }
}
