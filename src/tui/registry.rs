use std::collections::HashMap;
use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use crate::application::{
    ApplicationEvent, ApplicationEventSink, CruiseApplication, LogEvent, LogSink, PlanRequest,
    RunRequest, SessionSettingsRequest,
};
use crate::error::{CruiseError, Result};
use crate::issue_publish::PublishedIssue;
use crate::session::{CleanupReport, SessionState, WorkspaceMode};
use futures::FutureExt;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::JoinHandle;

/// Messages crossing from application workers to the event loop.
#[derive(Debug)]
pub enum UiEvent {
    Control(ApplicationEvent),
    Log(LogEvent),
    LogDropped,
    Error(String),
    SessionCreated {
        result: std::result::Result<SessionState, String>,
        plan: PlanRequest,
    },
    DraftCreated {
        result: std::result::Result<SessionState, String>,
    },
    Published {
        id: String,
        result: std::result::Result<PublishedIssue, String>,
    },
    Cleaned {
        result: std::result::Result<CleanupReport, String>,
    },
    SettingsFinished {
        id: String,
        result: std::result::Result<SessionState, String>,
        regenerate: bool,
    },
    Repositories {
        result: std::result::Result<Vec<String>, String>,
    },
}

/// Reliable control sink. High-volume plan chunks have a finite per-operation
/// budget; lifecycle and prompt events always retain reliable delivery.
#[derive(Clone)]
pub struct ControlSink {
    pub sender: UnboundedSender<UiEvent>,
    chunk_budget: Arc<AtomicUsize>,
}
impl ControlSink {
    pub fn new(sender: UnboundedSender<UiEvent>) -> Self {
        Self {
            sender,
            chunk_budget: Arc::new(AtomicUsize::new(1024 * 1024)),
        }
    }
}
impl ApplicationEventSink for ControlSink {
    fn send(&self, event: ApplicationEvent) -> Result<()> {
        if let ApplicationEvent::PlanChunk { text, .. } = &event {
            let length = text.len();
            if self
                .chunk_budget
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |budget| {
                    budget.checked_sub(length)
                })
                .is_err()
            {
                return Ok(());
            }
        }
        self.sender
            .send(UiEvent::Control(event))
            .map_err(|_| CruiseError::Other("TUI event loop closed".to_string()))
    }
}

#[derive(Clone)]
pub struct BoundedLogSink {
    pub sender: mpsc::Sender<UiEvent>,
    pub notice: UnboundedSender<UiEvent>,
    notice_pending: Arc<AtomicBool>,
}
impl BoundedLogSink {
    pub fn new(sender: mpsc::Sender<UiEvent>, notice: UnboundedSender<UiEvent>) -> Self {
        Self {
            sender,
            notice,
            notice_pending: Arc::new(AtomicBool::new(false)),
        }
    }
}
impl LogSink for BoundedLogSink {
    fn try_send(&self, event: LogEvent) -> bool {
        match self.sender.try_send(UiEvent::Log(event)) {
            Ok(()) => {
                self.notice_pending.store(false, Ordering::Release);
                true
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                if !self.notice_pending.swap(true, Ordering::AcqRel) {
                    let _ = self.notice.send(UiEvent::LogDropped);
                }
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

/// Owns worker handles only. Claims, cancellation, and prompt responders stay
/// in `CruiseApplication::runtime`, allowing tasks to outlive navigation.
#[derive(Default)]
pub struct OperationRegistry {
    tasks: HashMap<String, JoinHandle<()>>,
    batch: Option<JoinHandle<()>>,
    notifications: Option<UnboundedSender<UiEvent>>,
}

fn report_task_panic(tx: &UnboundedSender<UiEvent>, name: &str) {
    let _ = tx.send(UiEvent::Error(format!(
        "TUI {name} worker panicked; active work was stopped"
    )));
}
fn report_error(tx: &UnboundedSender<UiEvent>, error: &CruiseError) {
    if !matches!(error, &CruiseError::Interrupted) {
        let _ = tx.send(UiEvent::Error(error.to_string()));
    }
}

impl OperationRegistry {
    #[must_use]
    pub fn busy(&self, id: &str) -> bool {
        self.tasks.contains_key(id)
    }
    #[must_use]
    pub fn tasks_empty(&self) -> bool {
        self.tasks.is_empty()
    }
    #[must_use]
    pub fn batch_busy(&self) -> bool {
        self.batch.is_some()
    }

    pub fn reap(&mut self) {
        let mut tasks = std::mem::take(&mut self.tasks);
        let tx = self.notifications.clone();
        for (id, task) in tasks.drain() {
            if !task.is_finished() {
                self.tasks.insert(id, task);
                continue;
            }
            if let Some(Err(_)) = task.now_or_never()
                && let Some(tx) = tx.as_ref()
            {
                report_task_panic(tx, &id);
            }
        }
        if let Some(task) = self.batch.take() {
            if !task.is_finished() {
                self.batch = Some(task);
            } else if let Some(Err(_)) = task.now_or_never()
                && let Some(tx) = tx.as_ref()
            {
                report_task_panic(tx, "Run All");
            }
        }
    }

    pub async fn shutdown(&mut self) {
        for (_, task) in self.tasks.drain() {
            let _ = task.await;
        }
        if let Some(task) = self.batch.take() {
            let _ = task.await;
        }
    }
    fn spawn_operation<F>(
        &mut self,
        id: String,
        label: &'static str,
        tx: UnboundedSender<UiEvent>,
        future: F,
    ) -> bool
    where
        F: Future<Output = Result<SessionState>> + Send + 'static,
    {
        if self.busy(&id) {
            return false;
        }
        self.notifications = Some(tx.clone());
        self.tasks.insert(
            id,
            tokio::spawn(async move {
                match std::panic::AssertUnwindSafe(future).catch_unwind().await {
                    Ok(Ok(_) | Err(CruiseError::Interrupted)) => {}
                    Ok(Err(error)) => report_error(&tx, &error),
                    Err(_) => report_task_panic(&tx, label),
                }
            }),
        );
        true
    }

    pub fn create_draft(
        &mut self,
        app: CruiseApplication,
        request: crate::application::NewSessionRequest,
        tx: UnboundedSender<UiEvent>,
    ) -> bool {
        if self.busy("__create") {
            return false;
        }
        self.notifications = Some(tx.clone());
        self.tasks.insert(
            "__create".to_string(),
            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || app.create_session(request))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()));
                let _ = tx.send(UiEvent::DraftCreated { result });
            }),
        );
        true
    }

    pub fn plan(
        &mut self,
        app: CruiseApplication,
        id: String,
        request: PlanRequest,
        tx: UnboundedSender<UiEvent>,
    ) -> bool {
        let sink = Arc::new(ControlSink::new(tx.clone()));
        self.spawn_operation(id.clone(), "plan", tx, async move {
            app.generate(&id, request, sink).await
        })
    }

    pub fn replan(
        &mut self,
        app: CruiseApplication,
        id: String,
        request: PlanRequest,
        tx: UnboundedSender<UiEvent>,
    ) -> bool {
        let sink = Arc::new(ControlSink::new(tx.clone()));
        self.spawn_operation(id.clone(), "replan", tx, async move {
            app.replan(&id, request, sink).await
        })
    }

    pub fn fix(
        &mut self,
        app: CruiseApplication,
        id: String,
        feedback: String,
        tx: UnboundedSender<UiEvent>,
    ) -> bool {
        let sink = Arc::new(ControlSink::new(tx.clone()));
        self.spawn_operation(id.clone(), "fix", tx, async move {
            app.fix(&id, feedback, sink).await
        })
    }

    pub fn ask(
        &mut self,
        app: CruiseApplication,
        id: String,
        question: String,
        tx: UnboundedSender<UiEvent>,
    ) -> bool {
        let sink = Arc::new(ControlSink::new(tx.clone()));
        self.spawn_operation(id.clone(), "ask", tx, async move {
            app.ask(&id, question, sink).await
        })
    }

    pub fn run(
        &mut self,
        app: CruiseApplication,
        id: String,
        mode: Option<WorkspaceMode>,
        tx: UnboundedSender<UiEvent>,
        logs: mpsc::Sender<UiEvent>,
    ) -> bool {
        let sink = Arc::new(ControlSink::new(tx.clone()));
        let log_sink = Arc::new(BoundedLogSink::new(logs, tx.clone()));
        self.spawn_operation(id.clone(), "run", tx, async move {
            app.run_with_log_sink(
                &id,
                RunRequest {
                    workspace_mode: mode,
                    ..RunRequest::default()
                },
                sink,
                Some(log_sink),
            )
            .await
        })
    }

    pub fn run_all(
        &mut self,
        app: CruiseApplication,
        parallelism: usize,
        tx: UnboundedSender<UiEvent>,
        logs: mpsc::Sender<UiEvent>,
    ) -> bool {
        if self.batch_busy() {
            return false;
        }
        let config_app = app.clone();
        let configured_parallelism = parallelism;
        self.notifications = Some(tx.clone());
        let sink = Arc::new(ControlSink::new(tx.clone()));
        let log_sink = Arc::new(BoundedLogSink::new(logs, tx.clone()));
        self.batch = Some(tokio::spawn(async move {
            let parallelism_provider = move || {
                if configured_parallelism == 0 {
                    Ok(0)
                } else {
                    config_app
                        .app_config()
                        .map(|config| config.run_all_parallelism)
                }
            };
            match std::panic::AssertUnwindSafe(app.run_all_with_parallelism_provider(
                parallelism_provider,
                sink,
                Some(log_sink),
            ))
            .catch_unwind()
            .await
            {
                Ok(Ok(_) | Err(CruiseError::Interrupted)) => {}
                Ok(Err(error)) => report_error(&tx, &error),
                Err(_) => report_task_panic(&tx, "Run All"),
            }
        }));
        true
    }

    pub fn create(
        &mut self,
        app: CruiseApplication,
        request: crate::application::NewSessionRequest,
        plan: PlanRequest,
        tx: UnboundedSender<UiEvent>,
    ) -> bool {
        if self.busy("__create") {
            return false;
        }
        self.notifications = Some(tx.clone());
        self.tasks.insert(
            "__create".to_string(),
            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || app.create_session(request))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()));
                let _ = tx.send(UiEvent::SessionCreated { result, plan });
            }),
        );
        true
    }

    pub fn publish(
        &mut self,
        app: CruiseApplication,
        id: String,
        trigger_cruise: bool,
        tx: UnboundedSender<UiEvent>,
    ) -> bool {
        if self.busy(&id) {
            return false;
        }
        self.notifications = Some(tx.clone());
        let task_id = id.clone();
        let operation_id = id.clone();
        self.tasks.insert(
            task_id,
            tokio::spawn(async move {
                let result =
                    tokio::task::spawn_blocking(move || app.publish(&operation_id, trigger_cruise))
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|result| result.map_err(|error| error.to_string()));
                let _ = tx.send(UiEvent::Published { id, result });
            }),
        );
        true
    }

    pub fn clean(&mut self, app: CruiseApplication, tx: UnboundedSender<UiEvent>) -> bool {
        if self.busy("__clean") {
            return false;
        }
        self.notifications = Some(tx.clone());
        self.tasks.insert(
            "__clean".to_string(),
            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || app.clean())
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()));
                let _ = tx.send(UiEvent::Cleaned { result });
            }),
        );
        true
    }

    pub fn edit_settings(
        &mut self,
        app: CruiseApplication,
        id: String,
        request: SessionSettingsRequest,
        regenerate: bool,
        tx: UnboundedSender<UiEvent>,
    ) -> bool {
        if self.busy(&id) {
            return false;
        }
        self.notifications = Some(tx.clone());
        let task_id = id.clone();
        let operation_id = id.clone();
        self.tasks.insert(
            task_id,
            tokio::spawn(async move {
                let result =
                    tokio::task::spawn_blocking(move || app.edit_settings(&operation_id, request))
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|result| result.map_err(|error| error.to_string()));
                let _ = tx.send(UiEvent::SettingsFinished {
                    id,
                    result,
                    regenerate,
                });
            }),
        );
        true
    }

    pub fn repositories(&mut self, app: CruiseApplication, tx: UnboundedSender<UiEvent>) -> bool {
        if self.busy("__repositories") {
            return false;
        }
        self.notifications = Some(tx.clone());
        self.tasks.insert(
            "__repositories".to_string(),
            tokio::spawn(async move {
                let result = app
                    .list_github_repositories()
                    .await
                    .map_err(|error| error.to_string());
                let _ = tx.send(UiEvent::Repositories { result });
            }),
        );
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_starts_empty() {
        let registry = OperationRegistry::default();
        assert!(!registry.busy("missing"));
        assert!(!registry.batch_busy());
    }

    #[test]
    fn interrupted_worker_does_not_emit_error_event() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        report_error(&sender, &CruiseError::Interrupted);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn real_worker_errors_are_reported() {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        report_error(&sender, &CruiseError::Other("worker failed".to_string()));
        assert!(
            matches!(receiver.try_recv(), Ok(UiEvent::Error(message)) if message == "worker failed")
        );
    }
}
