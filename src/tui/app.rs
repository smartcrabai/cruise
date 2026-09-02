use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc::{self, UnboundedSender};

use super::forms::{Editor, Launch, NewSessionForm, SourceKind, Step};
use super::input::{self, Action};
use super::prompts::PromptQueue;
use super::registry::{OperationRegistry, UiEvent};
use crate::application::{
    ApplicationEvent, CruiseApplication, CurrentStepUpdateDto, EventStream, Interactive,
    PendingPromptKind, PlanRequest, SessionAction, SessionSettingsRequest,
};
use crate::session::{SessionState, WorkspaceMode};
use std::path::{Path, PathBuf};
const SESSION_LOG_LIMIT: usize = 10_000;
const BATCH_LOG_LIMIT: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Sessions,
    NewSession,
    RunAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailTab {
    Info,
    Dag,
    Plan,
    Log,
}

impl DetailTab {
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Info => Self::Dag,
            Self::Dag => Self::Plan,
            Self::Plan => Self::Log,
            Self::Log => Self::Info,
        }
    }
    #[must_use]
    pub fn previous(self) -> Self {
        match self {
            Self::Info => Self::Log,
            Self::Dag => Self::Info,
            Self::Plan => Self::Dag,
            Self::Log => Self::Plan,
        }
    }
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Dag => "DAG",
            Self::Plan => "Plan",
            Self::Log => "Log",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingCommand {
    Quit,
    RunAll,
    CancelRunAll,
    Clean,
    Session(SessionAction),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayPreferences {
    pub no_color: bool,
    pub follow_log: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ModalState {
    pub resized: bool,
    pub resize_had_prompt: bool,
    pub prompt_modal_pending: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct OperationState {
    pub quit_requested: bool,
    pub batch_cancelled: bool,
    pub bell_pending: bool,
}

pub enum Modal {
    Help,
    Error(String),
    Confirm {
        command: PendingCommand,
        message: String,
    },
    Palette {
        actions: Vec<SessionAction>,
        selected: usize,
    },
    Prompt,
    Input {
        title: String,
        command: PendingCommand,
        editor: Box<Editor>,
        multiline: bool,
        regenerate: bool,
    },
    Publish {
        trigger_cruise: bool,
    },
    Resize,
}

#[derive(Debug, Clone)]
pub struct BatchRow {
    pub id: String,
    pub title: String,
    pub phase: String,
    pub finished: bool,
}

pub struct TuiApp {
    pub application: CruiseApplication,
    pub registry: OperationRegistry,
    pub events: UnboundedSender<UiEvent>,
    pub logs_sender: mpsc::Sender<UiEvent>,
    pub view: View,
    pub tab: DetailTab,
    pub sessions: Vec<SessionState>,
    pub selected: usize,
    pub dag_selected: usize,
    pub form: NewSessionForm,
    pub modal: Option<Modal>,
    pub prompts: PromptQueue,
    pub logs: HashMap<String, VecDeque<String>>,
    pub batch_logs: VecDeque<String>,
    pub plan_cache: HashMap<String, String>,
    pub dag_cache: HashMap<String, crate::dag::ExecutionDag>,
    pub ask_responses: HashMap<String, VecDeque<String>>,
    ask_active: std::collections::HashSet<String>,
    pub plan_scroll: usize,
    pub log_scroll: usize,
    pub status: Option<String>,
    pub last_error: Option<String>,
    pub dropped_logs: usize,
    pub batch_total: usize,
    pub batch_finished: usize,
    pub batch_parallelism: usize,
    pub batch_rows: Vec<BatchRow>,
    pub batch_finished_ids: std::collections::HashSet<String>,
    active_planning: std::collections::HashSet<String>,
    pending_notifications: VecDeque<crate::desktop_notifications::NotificationPayload>,
    pub skip_cursor: usize,
    pub spinner_frame: usize,
    pub display: DisplayPreferences,
    pub modal_state: ModalState,
    pub operation_state: OperationState,
    pub github_repositories: Vec<String>,
    pub last_refresh: Instant,
    pub history_summary: Option<crate::application::NewSessionHistorySummary>,
    pub config_sources: Vec<crate::resolver::ConfigCandidate>,
    pub config_defaults: Option<crate::application::NewSessionConfigDefaults>,
    #[cfg(test)]
    test_process_lock: Option<crate::test_support::ProcessLock>,
}

impl TuiApp {
    pub fn new(
        application: CruiseApplication,
        events: UnboundedSender<UiEvent>,
        logs_sender: mpsc::Sender<UiEvent>,
    ) -> Self {
        let draft = application.draft().ok().flatten();
        let mut form = NewSessionForm::from_draft(draft.as_ref());
        let app_config = application.app_config().unwrap_or_default();
        let history_summary = application.new_session_history_summary().ok();
        if let Some(summary) = history_summary.as_ref() {
            form.apply_history_defaults(summary);
        }
        let mut app = Self {
            application,
            registry: OperationRegistry::default(),
            events,
            logs_sender,
            view: View::Sessions,
            tab: DetailTab::Info,
            sessions: Vec::new(),
            selected: 0,
            dag_selected: 0,
            form,
            modal: None,
            prompts: PromptQueue::default(),
            logs: HashMap::new(),
            batch_logs: VecDeque::new(),
            ask_responses: HashMap::new(),
            ask_active: std::collections::HashSet::new(),
            plan_cache: HashMap::new(),
            dag_cache: HashMap::new(),
            plan_scroll: 0,
            log_scroll: 0,
            status: None,
            last_error: None,
            dropped_logs: 0,
            batch_total: 0,
            batch_finished: 0,
            batch_parallelism: app_config.run_all_parallelism,
            batch_rows: Vec::new(),
            batch_finished_ids: std::collections::HashSet::new(),
            active_planning: std::collections::HashSet::new(),
            pending_notifications: VecDeque::new(),
            skip_cursor: 0,
            spinner_frame: 0,
            display: DisplayPreferences {
                no_color: std::env::var_os("NO_COLOR").is_some(),
                follow_log: true,
            },
            modal_state: ModalState::default(),
            operation_state: OperationState::default(),
            github_repositories: Vec::new(),
            last_refresh: Instant::now(),
            history_summary,
            config_sources: Vec::new(),
            config_defaults: None,
            #[cfg(test)]
            test_process_lock: None,
        };
        app.refresh();
        app
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_lock(
        application: CruiseApplication,
        events: UnboundedSender<UiEvent>,
        logs_sender: mpsc::Sender<UiEvent>,
        process_lock: Option<crate::test_support::ProcessLock>,
    ) -> Self {
        let mut app = Self::new(application, events, logs_sender);
        app.test_process_lock = process_lock;
        app
    }

    pub(crate) fn take_notifications(
        &mut self,
    ) -> std::collections::vec_deque::Drain<'_, crate::desktop_notifications::NotificationPayload>
    {
        self.pending_notifications.drain(..)
    }

    fn notification_subject(&self, session_id: &str) -> String {
        self.sessions
            .iter()
            .find(|session| session.id == session_id)
            .map(|session| session.title_or_input().to_string())
            .or_else(|| {
                self.application
                    .read_session(session_id)
                    .ok()
                    .map(|session| session.title_or_input().to_string())
            })
            .filter(|subject| !subject.trim().is_empty())
            .unwrap_or_else(|| session_id.to_string())
    }

    fn enqueue_notification(
        &mut self,
        kind: crate::desktop_notifications::WorkflowNotificationKind,
        session_id: &str,
        detail: Option<&str>,
    ) {
        let subject = self.notification_subject(session_id);
        self.pending_notifications
            .push_back(crate::desktop_notifications::build_payload(
                kind,
                Some(&subject),
                detail,
                session_id,
            ));
    }

    fn enqueue_action_required(&mut self, session_id: &str, prompt: &str) {
        let subject = self.notification_subject(session_id);
        let detail = format!("{prompt} ({subject})");
        self.pending_notifications
            .push_back(crate::desktop_notifications::build_payload(
                crate::desktop_notifications::WorkflowNotificationKind::ActionRequired,
                None,
                Some(&detail),
                session_id,
            ));
    }

    fn config_base_dir(&self) -> PathBuf {
        let working_dir = self.form.working_dir.text();
        if working_dir.trim().is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(crate::new_session_history::expand_tilde(working_dir.trim()))
        }
    }

    fn refresh_config_sources(&mut self, base_dir: &Path) -> bool {
        let mut sources = match self.application.discover_tui_config_candidates(base_dir) {
            Ok(sources) => sources,
            Err(error) => {
                self.config_sources.clear();
                self.config_defaults = None;
                self.set_error(error.to_string());
                return false;
            }
        };
        if self.form.source == SourceKind::GitHub {
            sources.retain(|source| !source.is_local());
        }
        self.config_sources = sources;
        true
    }

    fn sync_config_defaults(&mut self) -> bool {
        let base_dir = self.config_base_dir();
        let config = self.form.config.text();
        let config = config.trim();
        // GitHub sessions resolve an empty config selection only after the
        // repository has been cloned.  Resolving it against the caller's
        // working directory here could load an unrelated local cruise.yaml
        // and populate skip choices that do not exist in the clone.
        if self.form.source == SourceKind::GitHub && config.is_empty() {
            self.form.apply_default_skips(&[]);
            self.config_defaults = None;
            return true;
        }
        let repository = self.form.repository.text();
        let repository = repository.trim();
        let config = (!config.is_empty()).then_some(config);
        let repository = (self.form.source == SourceKind::GitHub && !repository.is_empty())
            .then_some(repository);
        match self
            .application
            .new_session_config_defaults(&base_dir, config, repository)
        {
            Ok(defaults) => {
                self.form
                    .apply_default_skips(&defaults.default_skipped_steps);
                self.config_defaults = Some(defaults);
                true
            }
            Err(error) => {
                self.config_defaults = None;
                self.set_error(error.to_string());
                false
            }
        }
    }

    fn refresh_config_context(&mut self) {
        let base_dir = self.config_base_dir();
        if !self.refresh_config_sources(&base_dir) {
            return;
        }

        let config = self.form.config.text();
        let config = config.trim();
        let config_is_candidate = config.is_empty()
            || self
                .config_sources
                .iter()
                .any(|source| source.selection_value() == config);
        if config_is_candidate {
            self.sync_config_defaults();
        } else {
            self.config_defaults = None;
        }
    }

    pub fn refresh(&mut self) {
        self.refresh_config_context();
        self.history_summary = self.application.new_session_history_summary().ok();
        match self.application.list_sessions() {
            Ok(mut sessions) => {
                for state in &mut sessions {
                    if let Ok(reconciled) = self.application.reconcile_session(&state.id) {
                        *state = reconciled;
                    }
                }
                let old = self
                    .sessions
                    .iter()
                    .map(|state| (state.id.as_str(), state))
                    .collect::<HashMap<_, _>>();
                for state in &sessions {
                    if old
                        .get(state.id.as_str())
                        .is_some_and(|previous| *previous != state)
                    {
                        self.plan_cache.remove(&state.id);
                        self.dag_cache.remove(&state.id);
                    }
                }
                self.sessions = sessions;
                self.selected = self.selected.min(self.sessions.len().saturating_sub(1));
                self.evict_inactive_caches();
                if self.status.is_none() || !self.is_busy() {
                    self.status = Some(format!(
                        "{} session{}",
                        self.sessions.len(),
                        if self.sessions.len() == 1 { "" } else { "s" }
                    ));
                }
                self.last_refresh = Instant::now();
                self.sync_prompts();
            }
            Err(error) => self.set_error(error.to_string()),
        }
        self.registry.reap();
    }

    fn evict_inactive_caches(&mut self) {
        let selected = self.active_session().map(|session| session.id.clone());
        self.plan_cache
            .retain(|id, _| selected.as_deref().is_some_and(|selected| selected == id));
        self.dag_cache
            .retain(|id, _| selected.as_deref().is_some_and(|selected| selected == id));
        self.logs
            .retain(|id, _| selected.as_deref().is_some_and(|selected| selected == id));
    }

    pub fn refresh_if_due(&mut self) {
        if self.last_refresh.elapsed() >= Duration::from_secs(3) {
            self.refresh();
        }
    }

    pub fn tick_spinner(&mut self) {
        self.registry.reap();
        if self.is_busy() {
            self.spinner_frame = (self.spinner_frame + 1) % 4;
        }
    }

    #[must_use]
    pub fn is_busy(&self) -> bool {
        !self.registry.tasks_empty() || self.registry.batch_busy() || self.application_has_claims()
    }

    fn application_has_claims(&self) -> bool {
        self.sessions.iter().any(|session| {
            self.application
                .runtime()
                .active_identity(&session.id)
                .is_some()
        })
    }

    #[must_use]
    pub fn active_session(&self) -> Option<&SessionState> {
        self.sessions.get(self.selected)
    }

    pub fn select_move(&mut self, delta: isize) {
        if self.sessions.is_empty() {
            return;
        }
        let len = self.sessions.len().cast_signed();
        self.selected = (self.selected.cast_signed() + delta)
            .rem_euclid(len)
            .cast_unsigned();
        self.log_scroll = 0;
        self.display.follow_log = true;
        self.dag_selected = 0;
        self.evict_inactive_caches();
        self.load_tab_data();
    }
    pub fn select_home(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = 0;
            self.evict_inactive_caches();
            self.load_tab_data();
        }
    }
    pub fn select_end(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = self.sessions.len() - 1;
            self.evict_inactive_caches();
            self.load_tab_data();
        }
    }
    pub fn move_detail(&mut self, delta: isize) {
        let Some(id) = self.active_session().map(|s| s.id.clone()) else {
            return;
        };
        let Some(dag) = self.dag_cache.get(&id) else {
            return;
        };
        if dag.nodes.is_empty() {
            return;
        }
        let len = dag.nodes.len().cast_signed();
        self.dag_selected = (self.dag_selected.cast_signed() + delta)
            .rem_euclid(len)
            .cast_unsigned();
    }

    pub fn load_tab_data(&mut self) {
        let Some(id) = self.active_session().map(|s| s.id.clone()) else {
            return;
        };
        match self.tab {
            DetailTab::Plan => {
                if !self.plan_cache.contains_key(&id)
                    && let Ok(plan) = self.application.session_plan(&id)
                {
                    self.plan_cache.insert(id, plan);
                }
            }
            DetailTab::Dag => {
                if !self.dag_cache.contains_key(&id)
                    && let Ok(Some(dag)) = self.application.session_dag(&id)
                {
                    self.dag_cache.insert(id, dag);
                }
            }
            DetailTab::Log => {
                if !self.logs.contains_key(&id)
                    && let Ok(log) = self.application.session_log(&id, Some(SESSION_LOG_LIMIT))
                {
                    let mut lines = VecDeque::new();
                    for line in log.lines() {
                        lines.push_back(line.to_string());
                    }
                    self.logs.insert(id, lines);
                }
            }
            DetailTab::Info => {}
        }
    }

    pub fn append_log(
        &mut self,
        session_id: Option<String>,
        stream: EventStream,
        text: &str,
        batch: bool,
    ) {
        let prefix = match stream {
            EventStream::Stdout => "stdout",
            EventStream::Stderr => "stderr",
            EventStream::Info => "info",
        };
        let session_prefix = session_id
            .as_deref()
            .map_or_else(String::new, |id| format!("[{id}] "));
        let lines = text
            .lines()
            .map(|line| format!("{session_prefix}[{prefix}] {line}"))
            .collect::<Vec<_>>();
        if batch {
            for line in &lines {
                push_bounded(&mut self.batch_logs, line, BATCH_LOG_LIMIT);
            }
        }
        if let Some(id) = session_id {
            let buffer = self.logs.entry(id).or_default();
            for line in lines {
                push_bounded(buffer, &line, SESSION_LOG_LIMIT);
            }
        }
        if self.display.follow_log {
            self.log_scroll = 0;
        }
    }

    pub fn append_plan_chunk(&mut self, id: String, stream: EventStream, text: &str) {
        if self.ask_active.contains(&id) {
            let prefix = match stream {
                EventStream::Stdout => "stdout",
                EventStream::Stderr => "stderr",
                EventStream::Info => "info",
            };
            let buffer = self.ask_responses.entry(id).or_default();
            for line in text.lines() {
                let line = format!("[{prefix}] {line}");
                push_bounded(buffer, &line, SESSION_LOG_LIMIT);
            }
            if self.display.follow_log {
                self.log_scroll = 0;
            }
        } else {
            self.append_log(Some(id), stream, text, false);
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.last_error = Some(error.clone());
        if self.prompts.active.is_some() || matches!(self.modal.as_ref(), Some(Modal::Prompt)) {
            self.modal_state.prompt_modal_pending = true;
        }
        self.modal = Some(Modal::Error(error));
    }

    fn set_error_and_refresh(&mut self, error: String) {
        self.set_error(error);
        self.refresh();
    }

    pub fn apply_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Control(event) => self.apply_control(event),
            UiEvent::Log(event) => {
                self.append_log(event.session_id, event.stream, &event.text, event.batch);
            }
            UiEvent::LogDropped => {
                self.dropped_logs = self.dropped_logs.saturating_add(1);
                self.status = Some(format!(
                    "Log queue full; {} chunk{} dropped (durable log retained)",
                    self.dropped_logs,
                    if self.dropped_logs == 1 { "" } else { "s" }
                ));
            }
            UiEvent::Error(error) => self.set_error_and_refresh(error),
            UiEvent::SessionCreated { result, plan } => match result {
                Ok(state) => {
                    let _ = self.application.clear_draft();
                    self.view = View::Sessions;
                    self.refresh();
                    self.selected = self
                        .sessions
                        .iter()
                        .position(|item| item.id == state.id)
                        .unwrap_or(self.selected);
                    self.evict_inactive_caches();
                    self.start_plan(state.id, plan);
                    self.form.mark_saved();
                    self.form.rewind();
                }
                Err(error) => self.set_error(error),
            },
            UiEvent::DraftCreated { result } => match result {
                Ok(state) => {
                    let _ = self.application.clear_draft();
                    self.form.mark_saved();
                    self.form.rewind();
                    self.view = View::Sessions;
                    self.refresh();
                    self.selected = self
                        .sessions
                        .iter()
                        .position(|item| item.id == state.id)
                        .unwrap_or(self.selected);
                    self.evict_inactive_caches();
                    self.status = Some(format!("Saved draft {}", state.id));
                }
                Err(error) => self.set_error(error),
            },
            UiEvent::Published { id, result } => match result {
                Ok(issue) => {
                    self.status = Some(format!("Published issue: {}", issue.url));
                    if let Err(error) = open_url(&issue.url) {
                        self.set_error(error.to_string());
                    }
                    self.invalidate(&id, true, true, true);
                    self.refresh();
                }
                Err(error) => self.set_error(error),
            },
            UiEvent::Cleaned { result } => match result {
                Ok(report) => {
                    self.status = Some(format!(
                        "Cleaned {} session{}; skipped {}",
                        report.deleted,
                        if report.deleted == 1 { "" } else { "s" },
                        report.skipped
                    ));
                    self.refresh();
                }
                Err(error) => self.set_error(error),
            },
            UiEvent::SettingsFinished {
                id,
                result,
                regenerate,
            } => match result {
                Ok(_) => {
                    self.invalidate(&id, true, true, false);
                    self.refresh();
                    if regenerate {
                        let request = PlanRequest {
                            interactive: Interactive::new(true),
                            ..PlanRequest::default()
                        };
                        self.start_replan(id, request);
                    }
                }
                Err(error) => self.set_error(error),
            },
            UiEvent::Repositories { result } => match result {
                Ok(repositories) => {
                    self.github_repositories = repositories;
                    self.status = Some(format!(
                        "{} GitHub repositories found",
                        self.github_repositories.len()
                    ));
                }
                Err(error) => self.set_error(error),
            },
        }
    }
    #[expect(
        clippy::too_many_lines,
        reason = "central event routing keeps application lifecycle handling together"
    )]
    fn apply_control(&mut self, event: ApplicationEvent) {
        match event {
            ApplicationEvent::PlanStarted {
                session_id,
                operation,
            } => {
                if matches!(
                    operation,
                    crate::application::OperationKind::Generate
                        | crate::application::OperationKind::Fix
                        | crate::application::OperationKind::Replan
                ) {
                    self.active_planning.insert(session_id);
                } else if operation == crate::application::OperationKind::Ask {
                    self.active_planning.remove(&session_id);
                }
                self.status = Some(format!("{} started", operation_label(operation)));
            }
            ApplicationEvent::PlanChunk {
                session_id,
                stream,
                text,
            } => self.append_plan_chunk(session_id, stream, &text),
            ApplicationEvent::AskUserRequired {
                session_id,
                request_id,
                question,
            } => {
                self.enqueue_action_required(&session_id, &question);
                self.queue_prompt(
                    request_id,
                    session_id,
                    PendingPromptKind::Ask,
                    question,
                    Vec::new(),
                );
            }
            ApplicationEvent::PlanFinished { session_id, phase } => {
                let was_planning = self.active_planning.remove(&session_id);
                if (phase == crate::session::SessionPhase::AwaitingApproval.label()
                    || phase == crate::session::SessionPhase::Planned.label())
                    && was_planning
                {
                    self.enqueue_notification(
                        crate::desktop_notifications::WorkflowNotificationKind::PlanReady,
                        &session_id,
                        None,
                    );
                }
                self.finish_plan(&session_id, &phase);
            }
            ApplicationEvent::PlanFailed { session_id, error } => {
                self.active_planning.remove(&session_id);
                self.enqueue_notification(
                    crate::desktop_notifications::WorkflowNotificationKind::Failed,
                    &session_id,
                    Some(&error),
                );
                self.fail_plan(&session_id, error);
            }
            ApplicationEvent::PlanCancelled { session_id } => {
                self.active_planning.remove(&session_id);
                self.cancel_plan(&session_id);
            }
            ApplicationEvent::RunStarted { session_id } => {
                self.status = Some(format!("Run started: {session_id}"));
                self.refresh();
            }
            ApplicationEvent::RunPhase { session_id, phase } => {
                self.status = Some(format!("{session_id}: {phase}"));
            }
            ApplicationEvent::StepStarted { session_id, step } => {
                self.status = Some(format!("{session_id}: {step}"));
            }
            ApplicationEvent::OptionRequired {
                session_id,
                request_id,
                prompt,
                choices,
            } => {
                self.enqueue_action_required(&session_id, &prompt);
                self.queue_prompt(
                    request_id,
                    session_id,
                    PendingPromptKind::Option,
                    prompt,
                    choices,
                );
            }
            ApplicationEvent::PrCreated { url, .. } => {
                self.status = Some(format!("Pull request created: {url}"));
                self.refresh();
            }
            ApplicationEvent::RunFinished { session_id, phase } => {
                if phase == crate::session::SessionPhase::Completed.label() {
                    self.enqueue_notification(
                        crate::desktop_notifications::WorkflowNotificationKind::Completed,
                        &session_id,
                        None,
                    );
                }
                self.finish_run(&session_id, &phase);
            }
            ApplicationEvent::RunFailed { session_id, error } => {
                self.enqueue_notification(
                    crate::desktop_notifications::WorkflowNotificationKind::Failed,
                    &session_id,
                    Some(&error),
                );
                self.fail_run(&session_id, error);
            }
            ApplicationEvent::RunCancelled { .. } => self.cancel_run(),
            ApplicationEvent::BatchStarted { total, parallelism } => {
                self.start_batch(total, parallelism);
            }
            ApplicationEvent::BatchTotalChanged { total } => {
                self.batch_total = total.max(self.batch_rows.len());
            }
            ApplicationEvent::BatchSessionStarted { id } => self.start_batch_session(&id),
            ApplicationEvent::BatchSessionFinished { id, phase, error } => {
                self.finish_batch_session(id, phase, error);
            }
            ApplicationEvent::BatchFinished { cancelled } => self.finish_batch(cancelled),
            ApplicationEvent::LogChunk {
                session_id,
                stream,
                text,
                batch,
            } => self.append_log(session_id, stream, &text, batch),
        }
    }

    fn finish_plan(&mut self, session_id: &str, phase: &str) {
        self.ask_active.remove(session_id);
        self.invalidate(session_id, true, true, false);
        self.status = Some(format!("Planning finished: {phase}"));
        self.refresh();
    }

    fn fail_plan(&mut self, session_id: &str, error: String) {
        self.ask_active.remove(session_id);
        self.invalidate(session_id, true, true, false);
        self.status = Some("Planning failed".to_string());
        self.set_error(error);
        self.refresh();
    }

    fn cancel_plan(&mut self, session_id: &str) {
        self.ask_active.remove(session_id);
        self.status = Some("Planning cancelled".to_string());
        self.refresh();
    }

    fn finish_run(&mut self, session_id: &str, phase: &str) {
        self.invalidate(session_id, true, true, false);
        self.status = Some(format!("Run finished: {phase}"));
        self.refresh();
    }

    fn fail_run(&mut self, session_id: &str, error: String) {
        self.invalidate(session_id, false, false, false);
        self.status = Some("Run failed".to_string());
        self.set_error(error);
        self.refresh();
    }

    fn cancel_run(&mut self) {
        self.status = Some("Run cancelled".to_string());
        self.refresh();
    }

    fn queue_prompt(
        &mut self,
        request_id: String,
        session_id: String,
        kind: PendingPromptKind,
        question: String,
        choices: Vec<crate::application::OptionChoicePayload>,
    ) {
        self.prompts.enqueue(super::prompts::PromptItem {
            request_id,
            session_id,
            kind,
            question,
            choices,
        });
        self.open_prompt_if_allowed();
    }

    fn start_batch(&mut self, total: usize, parallelism: usize) {
        self.batch_rows = self
            .application
            .run_all_candidates()
            .unwrap_or_default()
            .into_iter()
            .map(|session| {
                let title = session.title_or_input().to_string();
                let phase = session.phase.label().to_string();
                BatchRow {
                    id: session.id,
                    title,
                    phase,
                    finished: false,
                }
            })
            .collect();
        self.batch_total = total.max(self.batch_rows.len());
        self.batch_finished = 0;
        self.batch_finished_ids.clear();
        self.batch_parallelism = parallelism;
        self.operation_state.batch_cancelled = false;
        self.status = Some(format!(
            "Run All: {} session{}",
            self.batch_total,
            if self.batch_total == 1 { "" } else { "s" }
        ));
        self.refresh();
    }

    fn start_batch_session(&mut self, id: &str) {
        if !self.batch_rows.iter().any(|row| row.id == id)
            && let Ok(session) = self.application.read_session(id)
        {
            let title = session.title_or_input().to_string();
            let phase = session.phase.label().to_string();
            self.batch_rows.push(BatchRow {
                id: session.id,
                title,
                phase,
                finished: false,
            });
            self.batch_total = self.batch_total.max(self.batch_rows.len());
        }
        if let Some(row) = self.batch_rows.iter_mut().find(|row| row.id == id) {
            row.phase = "Running".to_string();
        }
        self.status = Some(format!("Run All started {id}"));
    }

    fn finish_batch_session(&mut self, id: String, phase: String, error: Option<String>) {
        let is_new = !self.batch_finished_ids.contains(&id);
        let line = error.map_or_else(
            || format!("{id}: {phase}"),
            |error| format!("{id}: {phase} ({error})"),
        );
        if let Some(row) = self.batch_rows.iter_mut().find(|row| row.id == id) {
            row.phase = phase;
            row.finished = true;
        }
        if is_new {
            self.batch_finished_ids.insert(id);
            self.batch_finished = self.batch_finished.saturating_add(1);
        }
        push_bounded(&mut self.batch_logs, &line, BATCH_LOG_LIMIT);
        self.refresh();
    }

    fn finish_batch(&mut self, cancelled: bool) {
        self.operation_state.batch_cancelled = cancelled;
        if cancelled {
            for row in &mut self.batch_rows {
                if !row.finished {
                    row.phase = "Cancelled".to_string();
                    row.finished = true;
                    self.batch_finished_ids.insert(row.id.clone());
                }
            }
        }
        self.batch_finished = self
            .batch_finished
            .max(self.batch_rows.iter().filter(|row| row.finished).count());
        self.operation_state.bell_pending = true;
        self.refresh();
        self.status = Some(if cancelled {
            "Run All cancelled".to_string()
        } else {
            "Run All finished".to_string()
        });
    }

    fn sync_prompts(&mut self) {
        let ids = self
            .sessions
            .iter()
            .map(|s| s.id.clone())
            .collect::<Vec<_>>();
        let known = ids
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        self.prompts.retain_sessions(&known);
        for id in ids {
            self.prompts
                .sync_session(&id, self.application.pending_prompts(&id));
        }
        if self.prompts.active.is_none() && matches!(self.modal.as_ref(), Some(Modal::Prompt)) {
            self.modal = None;
        }
        if self.prompts.active.is_none() {
            self.modal_state.prompt_modal_pending = false;
        }
        self.open_prompt_if_allowed();
    }

    fn open_prompt_if_allowed(&mut self) {
        if self.registry.batch_busy() {
            return;
        }
        if self.prompts.active.is_none() && !self.prompts.is_empty() {
            self.prompts.open_next();
            self.modal = Some(Modal::Prompt);
        }
    }

    fn open_queued_prompt(&mut self) {
        if self.prompts.active.is_none() && !self.prompts.is_empty() {
            self.prompts.open_next();
            self.modal = Some(Modal::Prompt);
        }
    }
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if self.operation_state.quit_requested {
            return false;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && self.handle_control_key(key) {
            return false;
        }
        if !key.modifiers.contains(KeyModifiers::CONTROL) {
            if matches!(self.modal.as_ref(), Some(Modal::Prompt)) {
                match key.code {
                    KeyCode::Char(_)
                    | KeyCode::Backspace
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Home
                    | KeyCode::End => {
                        self.prompts.answer.input(key);
                        return false;
                    }
                    _ => {}
                }
            } else if let Some(Modal::Input {
                editor, multiline, ..
            }) = self.modal.as_mut()
            {
                match key.code {
                    KeyCode::Char(_)
                    | KeyCode::Backspace
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::Up
                    | KeyCode::Down => {
                        editor.input(key);
                        return false;
                    }
                    KeyCode::Enter if *multiline => {
                        editor.input(key);
                        return false;
                    }
                    _ => {}
                }
            }
        }
        let action = input::action_for(key);
        if self.is_form_editor_key(key) {
            self.form.input(key);
            return false;
        }
        let should_quit = self.handle_action(action);
        if should_quit {
            self.operation_state.quit_requested = true;
        }
        should_quit
    }

    fn handle_control_key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Enter
            && matches!(
                self.modal.as_ref(),
                Some(Modal::Input {
                    multiline: true,
                    ..
                })
            )
        {
            if let Some(Modal::Input {
                command,
                editor,
                regenerate,
                ..
            }) = self.modal.take()
            {
                self.apply_input_command(command, editor.text(), regenerate);
            }
            return true;
        }
        if key.code == KeyCode::Char('r')
            && matches!(
                self.modal.as_ref(),
                Some(Modal::Input {
                    multiline: true,
                    ..
                })
            )
        {
            let regenerate = if let Some(Modal::Input { regenerate, .. }) = self.modal.as_mut() {
                *regenerate = !*regenerate;
                *regenerate
            } else {
                false
            };
            self.status = Some(
                if regenerate {
                    "Settings will save and regenerate"
                } else {
                    "Settings will save only"
                }
                .to_string(),
            );
            return true;
        }
        if self.modal.is_none() && self.view == View::NewSession {
            let launch = match key.code {
                KeyCode::Char('p') => Some(Launch::Planning),
                KeyCode::Char('g') => Some(Launch::Grill),
                KeyCode::Char('u') => Some(Launch::InputPlan),
                KeyCode::Char('s') => Some(Launch::SaveDraft),
                KeyCode::Enter => {
                    self.advance_step();
                    return true;
                }
                _ => None,
            };
            if let Some(launch) = launch {
                self.launch(launch);
                return true;
            }
        }
        matches!(key.code, KeyCode::Char('p' | 'g' | 'u' | 's'))
    }
    /// Skipped steps are picked from the workflow's step list when one is
    /// known and typed as comma-separated ids otherwise.
    fn step_is_text(&self) -> bool {
        self.form.step.is_text()
            || (self.form.step == Step::SkippedSteps && self.skip_choices().is_empty())
    }
    fn is_form_editor_key(&self, key: KeyEvent) -> bool {
        if self.modal.is_some()
            || self.view != View::NewSession
            || key.modifiers.contains(KeyModifiers::CONTROL)
            || !self.step_is_text()
        {
            return false;
        }
        matches!(
            key.code,
            KeyCode::Char(_)
                | KeyCode::Backspace
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Home
                | KeyCode::End
        ) || (self.form.step.is_multiline()
            && matches!(key.code, KeyCode::Up | KeyCode::Down | KeyCode::Enter))
    }
    pub fn handle_action(&mut self, action: Action) -> bool {
        if matches!(action, Action::Quit) {
            return self.request_quit();
        }
        if let Some(modal) = self.modal.take() {
            return self.handle_modal_action(modal, action);
        }
        self.handle_primary_action(action)
    }

    fn handle_primary_action(&mut self, action: Action) -> bool {
        match action {
            Action::ViewSessions => {
                self.view = View::Sessions;
                self.refresh();
                false
            }
            Action::ViewNewSession => {
                self.view = View::NewSession;
                false
            }
            Action::NewSession => {
                self.view = View::NewSession;
                self.form.rewind();
                false
            }
            Action::ViewRunAll => {
                self.view = View::RunAll;
                self.refresh();
                false
            }
            Action::Refresh => {
                self.refresh();
                false
            }
            Action::Help => {
                self.modal = Some(Modal::Help);
                false
            }
            Action::TabNext => self.handle_tab(true),
            Action::TabPrevious => self.handle_tab(false),
            Action::Up => self.navigate(-1),
            Action::Down => self.navigate(1),
            Action::PageUp => self.navigate(-8),
            Action::PageDown => self.navigate(8),
            Action::Home => self.navigate_home(),
            Action::End => self.navigate_end(),
            Action::DetailPrevious | Action::Left => {
                self.tab = self.tab.previous();
                self.load_tab_data();
                false
            }
            Action::DetailNext | Action::Right => {
                self.tab = self.tab.next();
                self.load_tab_data();
                false
            }
            Action::Palette => {
                self.open_palette();
                false
            }
            Action::Open => {
                if self.prompts.active.is_none() && !self.prompts.is_empty() {
                    self.open_queued_prompt();
                } else {
                    self.open_context_url();
                }
                false
            }
            Action::Follow => {
                self.display.follow_log = !self.display.follow_log;
                self.status = Some(
                    if self.display.follow_log {
                        "Following latest log output"
                    } else {
                        "Log follow paused"
                    }
                    .to_string(),
                );
                false
            }
            Action::Enter => self.enter_action(),
            Action::Escape => {
                // Text questions swallow letters and digits, so Esc is the
                // way out: one question back, and off the screen at the first.
                if self.view == View::NewSession && !self.form.retreat() {
                    self.view = View::Sessions;
                }
                false
            }
            Action::Character(' ') => {
                self.handle_space();
                false
            }
            Action::Character('c') if self.view == View::Sessions => {
                self.modal = Some(Modal::Confirm {
                    command: PendingCommand::Clean,
                    message: "Clean reclaimable sessions and closed PR sessions?".to_string(),
                });
                false
            }
            Action::Character(_) | Action::Backspace | Action::None => false,
            Action::Quit => self.request_quit(),
        }
    }

    fn handle_tab(&mut self, next: bool) -> bool {
        if self.view == View::NewSession {
            if !next {
                self.form.retreat();
            } else if !self.complete_current_path() {
                self.advance_step();
            }
        } else {
            self.tab = if next {
                self.tab.next()
            } else {
                self.tab.previous()
            };
            self.load_tab_data();
        }
        false
    }

    /// Space reaches here only on list steps; text steps consume it as input.
    fn handle_space(&mut self) {
        if self.view != View::NewSession {
            return;
        }
        if self.form.step == Step::SkippedSteps {
            self.toggle_skip_choice();
        } else if self.form.step.is_choice() {
            self.choose(1);
        }
    }
    fn choose(&mut self, delta: isize) {
        self.form.choose(delta);
        if self.form.step == Step::Source
            && self.form.source == SourceKind::GitHub
            && self.github_repositories.is_empty()
        {
            let _ = self
                .registry
                .repositories(self.application.clone(), self.events.clone());
        }
    }
    /// Accept the current answer and ask the next question; the last question
    /// starts (or drafts) the session with the chosen launch mode.
    fn advance_step(&mut self) {
        if self.form.step == Step::Config && !self.sync_config_defaults() {
            return;
        }
        if let Err(error) = self.form.validate_step() {
            self.set_error(error);
            return;
        }
        if self.form.step == Step::Launch {
            self.launch(self.form.launch);
        } else {
            let advanced = self.form.advance();
            if advanced && self.form.step == Step::Config {
                self.refresh_config_context();
            }
        }
    }
    fn launch(&mut self, launch: Launch) {
        if self.form.step == Step::Config && !self.sync_config_defaults() {
            return;
        }
        self.form.select_launch(launch);
        if launch == Launch::SaveDraft {
            self.save_as_draft();
        } else {
            self.create_session();
        }
    }
    /// Complete the path under the cursor; `false` when nothing changed so
    /// Tab can move on to the next question instead.
    fn complete_current_path(&mut self) -> bool {
        let step = self.form.step;
        let (raw, attachment_lines) = match step {
            Step::WorkingDirectory => (self.form.working_dir.text(), None),
            Step::Config => (self.form.config.text(), None),
            Step::Attachments => {
                let text = self.form.attachments.text();
                let lines = text
                    .split('\n')
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                let current = lines.last().cloned().unwrap_or_default();
                (current, Some(lines))
            }
            _ => return false,
        };
        let raw = raw.trim();
        if raw.is_empty() {
            return false;
        }
        let trailing_separator =
            raw == "~" || raw.ends_with('/') || raw.ends_with(std::path::MAIN_SEPARATOR);
        let (parent_raw, prefix) = if raw == "~" {
            ("~/".to_string(), String::new())
        } else if trailing_separator {
            (raw.to_string(), String::new())
        } else {
            let path = Path::new(raw);
            let parent = path
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default();
            let prefix = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (parent, prefix)
        };
        let parent_for_fs = if parent_raw.is_empty() {
            "."
        } else {
            &parent_raw
        };
        let expanded = crate::new_session_history::expand_tilde(parent_for_fs);
        let Some(name) = Self::path_completion_candidates(step, Path::new(&expanded), &prefix)
            .into_iter()
            .next()
        else {
            return false;
        };
        let completed = if parent_raw.is_empty() {
            name
        } else if trailing_separator
            || parent_raw.ends_with('/')
            || parent_raw.ends_with(std::path::MAIN_SEPARATOR)
        {
            format!("{parent_raw}{name}")
        } else {
            format!("{parent_raw}/{name}")
        };
        if completed == raw {
            return false;
        }
        match step {
            Step::WorkingDirectory => self.form.working_dir.set_text(&completed),
            Step::Config => self.form.config.set_text(&completed),
            Step::Attachments => {
                let mut lines = attachment_lines.unwrap_or_default();
                if let Some(last) = lines.last_mut() {
                    *last = completed;
                }
                self.form.attachments.set_text(&lines.join("\n"));
            }
            _ => {}
        }
        self.form.mark_changed();
        true
    }

    fn path_completion_candidates(step: Step, expanded: &Path, prefix: &str) -> Vec<String> {
        let mut candidates = std::fs::read_dir(expanded)
            .ok()
            .into_iter()
            .flat_map(|entries| entries.filter_map(std::result::Result::ok))
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with(prefix) || name.starts_with('.') {
                    return None;
                }
                let is_dir = entry.file_type().ok()?.is_dir();
                let allowed = match step {
                    Step::WorkingDirectory => is_dir,
                    Step::Config => {
                        is_dir
                            || Path::new(&name)
                                .extension()
                                .is_some_and(|ext| matches!(ext.to_str(), Some("yaml" | "yml")))
                    }
                    Step::Attachments => {
                        is_dir
                            || Path::new(&name).extension().is_some_and(|ext| {
                                matches!(
                                    ext.to_str().map(str::to_ascii_lowercase).as_deref(),
                                    Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp")
                                )
                            })
                    }
                    _ => false,
                };
                allowed.then_some(name)
            })
            .collect::<Vec<_>>();
        candidates.sort();
        candidates
    }

    /// Skippable steps of the resolved workflow as `(label, expanded ids)`.
    pub(super) fn skip_choices(&self) -> Vec<(String, Vec<String>)> {
        fn visit(
            nodes: &[crate::workflow::SkippableStepNode],
            out: &mut Vec<(String, Vec<String>)>,
        ) {
            for node in nodes {
                out.push((node.id.clone(), node.expanded_step_ids.clone()));
                visit(&node.children, out);
            }
        }
        let mut choices = Vec::new();
        if let Some(defaults) = self.config_defaults.as_ref() {
            visit(&defaults.steps, &mut choices);
            visit(&defaults.after_pr_steps, &mut choices);
        }
        choices
    }

    fn toggle_skip_choice(&mut self) {
        let choices = self.skip_choices();
        if choices.is_empty() {
            self.form.skipped_explicit = true;
            self.form.mark_changed();
            return;
        }
        self.skip_cursor %= choices.len();
        let selected = self.form.selected_skipped_steps();
        let mut values = selected
            .iter()
            .cloned()
            .collect::<std::collections::HashSet<_>>();
        let ids = &choices[self.skip_cursor].1;
        if ids.iter().all(|id| values.contains(id)) {
            for id in ids {
                values.remove(id);
            }
        } else {
            values.extend(ids.iter().cloned());
        }
        let mut values = values.into_iter().collect::<Vec<_>>();
        values.sort();
        self.form.skipped.set_text(&values.join(", "));
        self.form.skipped_explicit = true;
        self.form.mark_changed();
    }
    fn navigate(&mut self, delta: isize) -> bool {
        if self.view == View::NewSession {
            match self.form.step {
                Step::SkippedSteps => {
                    let count = self.skip_choices().len();
                    if count > 0 {
                        self.skip_cursor = (self.skip_cursor.cast_signed() + delta)
                            .rem_euclid(count.cast_signed())
                            .cast_unsigned();
                    }
                }
                Step::WorkingDirectory => {
                    if let Some(summary) = self.history_summary.as_ref() {
                        self.form
                            .cycle_working_directory(&summary.recent_working_dirs, delta);
                    }
                }
                Step::Config => {
                    self.form.cycle_config(&self.config_sources, delta);
                    self.sync_config_defaults();
                }
                Step::Repository => self.form.cycle_repository(&self.github_repositories, delta),
                step if step.is_choice() => self.choose(delta),
                _ => {}
            }
        } else if self.view == View::Sessions && self.tab == DetailTab::Dag {
            self.move_detail(delta);
        } else if self.view == View::Sessions && self.tab == DetailTab::Log {
            self.update_log_scroll(delta);
        } else if self.view == View::Sessions && self.tab == DetailTab::Plan {
            self.update_plan_scroll(delta);
        } else {
            self.select_move(delta);
        }
        false
    }
    fn navigate_home(&mut self) -> bool {
        if self.view == View::NewSession {
            self.form.rewind();
        } else if self.tab == DetailTab::Log {
            self.display.follow_log = false;
            self.log_scroll = self.current_line_count();
        } else if self.tab == DetailTab::Plan {
            self.plan_scroll = 0;
        } else {
            self.select_home();
        }
        false
    }
    fn navigate_end(&mut self) -> bool {
        if self.view == View::NewSession {
            self.form.step = Step::Launch;
        } else if self.tab == DetailTab::Log {
            self.display.follow_log = true;
            self.log_scroll = 0;
        } else if self.tab == DetailTab::Plan {
            self.plan_scroll = self
                .active_plan()
                .map_or(0, |plan| plan.lines().count().saturating_sub(1));
        } else {
            self.select_end();
        }
        false
    }

    fn handle_modal_action(&mut self, modal: Modal, action: Action) -> bool {
        match modal {
            Modal::Help | Modal::Error(_) | Modal::Resize => {
                self.handle_simple_modal(modal, action)
            }
            modal @ Modal::Confirm { command, .. } => {
                self.handle_confirm_modal(modal, command, action)
            }
            Modal::Palette { actions, selected } => {
                self.handle_palette_modal(actions, selected, action)
            }
            Modal::Prompt => self.handle_prompt_modal(action),
            Modal::Input {
                title,
                command,
                editor,
                multiline,
                regenerate,
            } => self.handle_input_modal(title, command, editor, multiline, regenerate, action),
            Modal::Publish { trigger_cruise } => self.handle_publish_modal(trigger_cruise, action),
        }
    }

    fn handle_simple_modal(&mut self, modal: Modal, action: Action) -> bool {
        if matches!(action, Action::Escape | Action::Enter | Action::Help) {
            if self.modal_state.prompt_modal_pending && self.prompts.active.is_some() {
                self.modal_state.prompt_modal_pending = false;
                self.modal = Some(Modal::Prompt);
            }
        } else {
            self.modal = Some(modal);
        }
        false
    }

    fn handle_confirm_modal(
        &mut self,
        modal: Modal,
        command: PendingCommand,
        action: Action,
    ) -> bool {
        match action {
            Action::Enter => self.apply_command(command),
            Action::Escape => {}
            _ => self.modal = Some(modal),
        }
        false
    }

    fn handle_palette_modal(
        &mut self,
        actions: Vec<SessionAction>,
        mut selected: usize,
        action: Action,
    ) -> bool {
        match action {
            Action::Up => {
                if !actions.is_empty() {
                    selected = (selected + actions.len() - 1) % actions.len();
                }
                self.modal = Some(Modal::Palette { actions, selected });
            }
            Action::Down => {
                if !actions.is_empty() {
                    selected = (selected + 1) % actions.len();
                }
                self.modal = Some(Modal::Palette { actions, selected });
            }
            Action::Enter => {
                if let Some(command) = actions.get(selected).copied() {
                    self.apply_action(command);
                }
            }
            Action::Escape | Action::Palette => {}
            _ => self.modal = Some(Modal::Palette { actions, selected }),
        }
        false
    }

    fn handle_prompt_modal(&mut self, action: Action) -> bool {
        match action {
            Action::Up => {
                self.prompts.move_choice(-1);
                self.modal = Some(Modal::Prompt);
            }
            Action::Down => {
                self.prompts.move_choice(1);
                self.modal = Some(Modal::Prompt);
            }
            Action::Enter => self.submit_prompt(),
            Action::Escape => {
                self.prompts.requeue_active();
                self.modal = None;
            }
            Action::Character(c) => {
                self.prompts
                    .answer
                    .input(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
                self.modal = Some(Modal::Prompt);
            }
            Action::Backspace => {
                self.prompts
                    .answer
                    .input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
                self.modal = Some(Modal::Prompt);
            }
            _ => self.modal = Some(Modal::Prompt),
        }
        false
    }

    fn handle_input_modal(
        &mut self,
        title: String,
        command: PendingCommand,
        mut editor: Box<Editor>,
        multiline: bool,
        regenerate: bool,
        action: Action,
    ) -> bool {
        match action {
            Action::Enter if multiline => {
                editor.input(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                self.modal = Some(Modal::Input {
                    title,
                    command,
                    editor,
                    multiline,
                    regenerate,
                });
            }
            Action::Enter => {
                self.apply_input_command(command, editor.text(), regenerate);
            }
            Action::Escape => {}
            Action::Character(c) => {
                editor.input(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
                self.modal = Some(Modal::Input {
                    title,
                    command,
                    editor,
                    multiline,
                    regenerate,
                });
            }
            Action::Backspace => {
                editor.input(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
                self.modal = Some(Modal::Input {
                    title,
                    command,
                    editor,
                    multiline,
                    regenerate,
                });
            }
            _ => {
                self.modal = Some(Modal::Input {
                    title,
                    command,
                    editor,
                    multiline,
                    regenerate,
                });
            }
        }
        false
    }

    fn handle_publish_modal(&mut self, mut trigger_cruise: bool, action: Action) -> bool {
        match action {
            Action::Up | Action::Down | Action::Character(' ') => {
                trigger_cruise = !trigger_cruise;
                self.modal = Some(Modal::Publish { trigger_cruise });
            }
            Action::Enter => self.apply_publish(trigger_cruise),
            Action::Escape => {}
            _ => self.modal = Some(Modal::Publish { trigger_cruise }),
        }
        false
    }
    fn enter_action(&mut self) -> bool {
        match self.view {
            View::NewSession => {
                self.advance_step();
                false
            }
            View::RunAll | View::Sessions => {
                self.open_palette();
                false
            }
        }
    }
    fn request_quit(&mut self) -> bool {
        if self.is_busy() {
            self.modal = Some(Modal::Confirm {
                command: PendingCommand::Quit,
                message: "Active work will be cancelled. Quit Cruise TUI?".to_string(),
            });
            false
        } else {
            true
        }
    }

    pub fn cancel_and_quit(&mut self) {
        let _ = self.application.cancel_run_all();
        for session in &self.sessions {
            let _ = self.application.cancel_session(&session.id);
        }
        self.modal = None;
        self.status = Some("Cancelling active work".to_string());
        self.operation_state.quit_requested = true;
    }

    fn open_palette(&mut self) {
        if self.view == View::RunAll {
            let command = if self.registry.batch_busy() {
                PendingCommand::CancelRunAll
            } else {
                PendingCommand::RunAll
            };
            self.modal = Some(Modal::Confirm {
                command,
                message: if self.registry.batch_busy() {
                    "Cancel Run All and suspend active sessions?"
                } else {
                    "Run all Planned and Suspended sessions?"
                }
                .to_string(),
            });
            return;
        }
        let Some(state) = self.active_session() else {
            self.status = Some("No session selected".to_string());
            return;
        };
        let mut actions = self.application.capabilities(state);
        if matches!(&state.phase, crate::session::SessionPhase::AwaitingApproval) {
            for action in [SessionAction::EditSettings, SessionAction::Delete] {
                if !actions.contains(&action) {
                    actions.push(action);
                }
            }
        }
        if self
            .application
            .runtime()
            .active_identity(&state.id)
            .is_some()
            && !actions.contains(&SessionAction::Cancel)
        {
            actions.push(SessionAction::Cancel);
        }
        if actions.is_empty() {
            self.status = Some("No actions available".to_string());
        } else {
            self.modal = Some(Modal::Palette {
                actions,
                selected: 0,
            });
        }
    }

    fn apply_action(&mut self, action: SessionAction) {
        match action {
            SessionAction::Delete | SessionAction::Discard | SessionAction::ResetToPlanned => {
                let message = format!(
                    "{} {}?",
                    action_label(action),
                    self.active_session()
                        .map_or("this session", |s| s.id.as_str())
                );
                self.modal = Some(Modal::Confirm {
                    command: PendingCommand::Session(action),
                    message,
                });
            }
            SessionAction::Publish => {
                let trigger_cruise = self.active_session().is_some_and(|session| {
                    matches!(&session.phase, crate::session::SessionPhase::Planned)
                });
                self.modal = Some(Modal::Publish { trigger_cruise });
            }
            SessionAction::Fix => self.open_input(
                "Describe the plan fix",
                PendingCommand::Session(SessionAction::Fix),
            ),
            SessionAction::Ask => self.open_input(
                "Ask about this plan",
                PendingCommand::Session(SessionAction::Ask),
            ),
            SessionAction::EditSettings => self.open_input(
                "Config path (blank keeps current); skipped steps (one per line)",
                PendingCommand::Session(SessionAction::EditSettings),
            ),
            SessionAction::EditCurrentStep => self.open_input(
                "Current step (blank resumes from beginning)",
                PendingCommand::Session(SessionAction::EditCurrentStep),
            ),
            SessionAction::Replan => self.open_input(
                "Describe the changes for the new plan",
                PendingCommand::Session(SessionAction::Replan),
            ),
            SessionAction::Generate
            | SessionAction::Answer
            | SessionAction::Cancel
            | SessionAction::Approve
            | SessionAction::RunWorktree
            | SessionAction::RunCurrentBranch
            | SessionAction::Retry
            | SessionAction::Resume
            | SessionAction::OpenPr => self.apply_command(PendingCommand::Session(action)),
        }
    }

    fn open_input(&mut self, title: &str, command: PendingCommand) {
        let multiline = matches!(
            command,
            PendingCommand::Session(SessionAction::EditSettings)
        );
        let mut editor = Editor::default();
        if multiline && let Some(session) = self.active_session() {
            editor.set_text(&format!("\n{}", session.skipped_steps.join("\n")));
        }
        self.modal = Some(Modal::Input {
            title: title.to_string(),
            command,
            editor: Box::new(editor),
            multiline,
            regenerate: false,
        });
    }

    fn apply_input_command(&mut self, command: PendingCommand, value: String, regenerate: bool) {
        self.modal = None;
        let Some(id) = self.active_session().map(|s| s.id.clone()) else {
            return;
        };
        match command {
            PendingCommand::Session(SessionAction::Fix) => {
                if value.trim().is_empty() {
                    self.set_error("Fix feedback must not be empty".to_string());
                    return;
                }
                if !self
                    .registry
                    .fix(self.application.clone(), id, value, self.events.clone())
                {
                    self.set_error("Session already has active work".to_string());
                }
            }
            PendingCommand::Session(SessionAction::Ask) => {
                if value.trim().is_empty() {
                    self.set_error("Question must not be empty".to_string());
                    return;
                }
                if self.registry.ask(
                    self.application.clone(),
                    id.clone(),
                    value,
                    self.events.clone(),
                ) {
                    self.ask_responses.insert(id.clone(), VecDeque::new());
                    self.ask_active.insert(id);
                } else {
                    self.set_error("Session already has active work".to_string());
                }
            }
            PendingCommand::Session(SessionAction::Replan) => {
                if value.trim().is_empty() {
                    self.set_error("Replan feedback must not be empty".to_string());
                    return;
                }
                let request = PlanRequest {
                    feedback: Some(value),
                    interactive: Interactive::new(true),
                    ..PlanRequest::default()
                };
                self.start_replan(id, request);
            }
            PendingCommand::Session(SessionAction::EditSettings) => {
                let mut lines = value.lines();
                let config_path = lines
                    .next()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(ToString::to_string)
                    .or_else(|| {
                        self.active_session().and_then(|session| {
                            session
                                .config_path
                                .as_ref()
                                .map(|path| path.to_string_lossy().into_owned())
                        })
                    });
                let skipped_steps = lines
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(ToString::to_string)
                    .collect();
                if !self.registry.edit_settings(
                    self.application.clone(),
                    id,
                    SessionSettingsRequest {
                        config_path,
                        skipped_steps,
                        current_step_update: CurrentStepUpdateDto::Unchanged,
                    },
                    regenerate,
                    self.events.clone(),
                ) {
                    self.set_error("Session already has active work".to_string());
                }
            }
            PendingCommand::Session(SessionAction::EditCurrentStep) => {
                let update = if value.trim().is_empty() {
                    CurrentStepUpdateDto::Clear
                } else {
                    CurrentStepUpdateDto::Set(value.trim().to_string())
                };
                match self.application.edit_current_step(&id, update) {
                    Ok(_) => {
                        self.invalidate(&id, true, true, true);
                        self.refresh();
                    }
                    Err(error) => self.set_error(error.to_string()),
                }
            }
            _ => self.status = Some("Input was not used by this action".to_string()),
        }
    }

    fn apply_command(&mut self, command: PendingCommand) {
        self.modal = None;
        match command {
            PendingCommand::Quit => self.cancel_and_quit(),
            PendingCommand::RunAll => {
                if self.registry.run_all(
                    self.application.clone(),
                    self.batch_parallelism.max(1),
                    self.events.clone(),
                    self.logs_sender.clone(),
                ) {
                    self.view = View::RunAll;
                    self.status = Some("Starting Run All".to_string());
                } else {
                    self.set_error("Run All is already active".to_string());
                }
            }
            PendingCommand::CancelRunAll => {
                if !self.application.cancel_run_all() {
                    self.status = Some("No Run All operation is active".to_string());
                }
            }
            PendingCommand::Clean => {
                if self
                    .registry
                    .clean(self.application.clone(), self.events.clone())
                {
                    self.status = Some("Cleaning sessions".to_string());
                } else {
                    self.set_error("Clean is already active".to_string());
                }
            }
            PendingCommand::Session(action) => self.apply_session_command(action),
        }
    }

    fn apply_session_command(&mut self, action: SessionAction) {
        let Some(id) = self.active_session().map(|s| s.id.clone()) else {
            return;
        };
        match action {
            SessionAction::Generate => {
                let request = PlanRequest {
                    interactive: Interactive::new(true),
                    ..PlanRequest::default()
                };
                self.start_plan(id, request);
            }
            SessionAction::Answer => {
                self.sync_prompts();
                self.open_queued_prompt();
            }
            SessionAction::Cancel => {
                if !self.application.cancel_session(&id) {
                    self.status = Some("Session is not running in this TUI process".to_string());
                }
            }
            SessionAction::Delete => match self.application.delete_session(&id) {
                Ok(()) => {
                    self.invalidate(&id, true, true, true);
                    self.status = Some(format!("Deleted {id}"));
                    self.refresh();
                }
                Err(error) => self.set_error(error.to_string()),
            },
            SessionAction::Approve => match self.application.approve(&id) {
                Ok(_) => {
                    self.invalidate(&id, true, true, true);
                    self.refresh();
                }
                Err(error) => self.set_error(error.to_string()),
            },
            SessionAction::Publish => {
                self.apply_publish(self.active_session().is_some_and(|session| {
                    matches!(&session.phase, crate::session::SessionPhase::Planned)
                }));
            }
            SessionAction::Fix
            | SessionAction::Ask
            | SessionAction::EditSettings
            | SessionAction::EditCurrentStep
            | SessionAction::Replan => self.apply_action(action),
            SessionAction::Discard => match self.application.discard_session(&id) {
                Ok(()) => {
                    self.invalidate(&id, true, true, true);
                    self.status = Some(format!("Discarded {id}"));
                    self.refresh();
                }
                Err(error) => self.set_error(error.to_string()),
            },
            SessionAction::RunWorktree => self.start_run(id, Some(WorkspaceMode::Worktree)),
            SessionAction::RunCurrentBranch => {
                self.start_run(id, Some(WorkspaceMode::CurrentBranch));
            }
            SessionAction::Retry | SessionAction::Resume => self.start_run(id, None),
            SessionAction::ResetToPlanned => match self.application.reset_to_planned(&id) {
                Ok(_) => {
                    self.invalidate(&id, true, true, true);
                    self.refresh();
                }
                Err(error) => self.set_error(error.to_string()),
            },
            SessionAction::OpenPr => match self.application.open_pr(&id) {
                Ok(url) => match open_url(&url) {
                    Ok(()) => self.status = Some(format!("Opening {url}")),
                    Err(error) => self.set_error(error.to_string()),
                },
                Err(error) => self.set_error(error.to_string()),
            },
        }
    }

    fn apply_publish(&mut self, trigger_cruise: bool) {
        let Some(id) = self.active_session().map(|s| s.id.clone()) else {
            return;
        };
        if self.registry.publish(
            self.application.clone(),
            id,
            trigger_cruise,
            self.events.clone(),
        ) {
            self.status = Some("Publishing issue".to_string());
        } else {
            self.set_error("Session already has active work".to_string());
        }
    }

    fn start_plan(&mut self, id: String, request: PlanRequest) {
        if self
            .registry
            .plan(self.application.clone(), id, request, self.events.clone())
        {
            self.status = Some("Planning started".to_string());
        } else {
            self.set_error("Session already has active work".to_string());
        }
    }

    fn start_replan(&mut self, id: String, request: PlanRequest) {
        if self
            .registry
            .replan(self.application.clone(), id, request, self.events.clone())
        {
            self.status = Some("Replanning started".to_string());
        } else {
            self.set_error("Session already has active work".to_string());
        }
    }

    fn invalidate(&mut self, id: &str, plan: bool, dag: bool, log: bool) {
        if plan {
            self.plan_cache.remove(id);
            self.plan_scroll = 0;
        }
        if dag {
            self.dag_cache.remove(id);
            self.dag_selected = 0;
        }
        if log {
            self.logs.remove(id);
            self.log_scroll = 0;
        }
    }
    fn start_run(&mut self, id: String, mode: Option<WorkspaceMode>) {
        if self.registry.run(
            self.application.clone(),
            id,
            mode,
            self.events.clone(),
            self.logs_sender.clone(),
        ) {
            self.status = Some("Run started".to_string());
        } else {
            self.set_error("Session already has active work".to_string());
        }
    }

    fn submit_prompt(&mut self) {
        let Some(prompt) = self.prompts.active.clone() else {
            self.modal = None;
            return;
        };
        let result = match prompt.kind {
            PendingPromptKind::Ask => self.prompts.answer_text().map_or_else(
                || {
                    Err(crate::error::CruiseError::Other(
                        "answer must not be empty".to_string(),
                    ))
                },
                |answer| {
                    self.application
                        .respond_to_ask(&prompt.session_id, &prompt.request_id, answer)
                },
            ),
            PendingPromptKind::Option => self.prompts.selected_option().map_or_else(
                || {
                    Err(crate::error::CruiseError::Other(
                        "select a non-empty option".to_string(),
                    ))
                },
                |result| {
                    self.application.respond_to_option(
                        &prompt.session_id,
                        &prompt.request_id,
                        result,
                    )
                },
            ),
        };
        match result {
            Ok(()) => {
                self.prompts.close_active();
                self.modal_state.prompt_modal_pending = false;
                self.modal = None;
                self.sync_prompts();
            }
            Err(error) => {
                self.modal_state.prompt_modal_pending = true;
                self.set_error(error.to_string());
            }
        }
    }

    fn open_context_url(&mut self) {
        if let Some(session) = self.active_session() {
            if let Some(url) = session
                .pr_url
                .as_deref()
                .or(session.published_issue_url.as_deref())
            {
                match open_url(url) {
                    Ok(()) => self.status = Some(format!("Opening {url}")),
                    Err(error) => self.set_error(error.to_string()),
                }
            } else {
                self.status = Some("No dedicated PR or Issue URL for this session".to_string());
            }
        }
    }

    pub fn update_log_scroll(&mut self, delta: isize) {
        if self.display.follow_log {
            self.display.follow_log = false;
        }
        self.log_scroll = if delta.is_negative() {
            self.log_scroll.saturating_add(delta.unsigned_abs())
        } else {
            self.log_scroll.saturating_sub(delta.cast_unsigned())
        };
    }

    pub fn update_plan_scroll(&mut self, delta: isize) {
        let max_scroll = self
            .active_plan()
            .map_or(0, |plan| plan.lines().count().saturating_sub(1));
        self.plan_scroll = if delta.is_negative() {
            self.plan_scroll.saturating_sub(delta.unsigned_abs())
        } else {
            self.plan_scroll.saturating_add(delta.cast_unsigned())
        }
        .min(max_scroll);
    }
    fn current_line_count(&self) -> usize {
        if self.view == View::RunAll {
            return self.batch_logs.len();
        }
        let Some(id) = self.active_session().map(|s| s.id.as_str()) else {
            return 0;
        };
        self.logs.get(id).map_or(0, VecDeque::len)
            + self.ask_responses.get(id).map_or(0, VecDeque::len)
    }

    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.operation_state.bell_pending)
    }

    pub fn visible_lines(&self, height: usize) -> Vec<String> {
        if self.view == View::RunAll {
            let end = self.batch_logs.len().saturating_sub(self.log_scroll);
            let start = end.saturating_sub(height);
            return self
                .batch_logs
                .iter()
                .skip(start)
                .take(end.saturating_sub(start))
                .cloned()
                .collect();
        }
        let Some(id) = self.active_session().map(|s| s.id.as_str()) else {
            return Vec::new();
        };
        let logs = self.logs.get(id);
        let ask = self.ask_responses.get(id);
        let log_len = logs.map_or(0, VecDeque::len);
        let ask_len = ask.map_or(0, VecDeque::len);
        let total = log_len.saturating_add(ask_len);
        let end = total.saturating_sub(self.log_scroll);
        let start = end.saturating_sub(height);
        let count = end.saturating_sub(start);
        let mut visible = Vec::with_capacity(count);
        if start < log_len {
            let take = count.min(log_len - start);
            if let Some(lines) = logs {
                visible.extend(lines.iter().skip(start).take(take).cloned());
            }
        }
        let ask_start = start.saturating_sub(log_len);
        let ask_take = count.saturating_sub(visible.len());
        if ask_take > 0
            && let Some(lines) = ask
        {
            visible.extend(lines.iter().skip(ask_start).take(ask_take).cloned());
        }
        visible
    }
    #[must_use]
    pub fn active_plan(&self) -> Option<&str> {
        self.active_session()
            .and_then(|s| self.plan_cache.get(&s.id))
            .map(String::as_str)
    }
    #[must_use]
    pub fn active_dag(&self) -> Option<&crate::dag::ExecutionDag> {
        self.active_session()
            .and_then(|s| self.dag_cache.get(&s.id))
    }

    fn session_request(&mut self) -> Option<crate::application::NewSessionRequest> {
        if let Err(error) = self.form.validate() {
            self.set_error(error);
            return None;
        }
        let mut request = self.form.request();
        if !self.form.skipped_explicit
            && request.skipped_steps.is_empty()
            && (request.repo.is_none() || request.config_path.is_some())
        {
            let base = request.base_dir.clone();
            let config = request
                .config_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned());
            let repo = request.repo.as_deref();
            if let Ok(defaults) =
                self.application
                    .new_session_config_defaults(&base, config.as_deref(), repo)
            {
                request.skipped_steps = defaults.default_skipped_steps;
            }
        }
        if request.input.trim().is_empty() && request.attachments.is_empty() {
            self.set_error("Task description or an image attachment is required".to_string());
            return None;
        }
        Some(request)
    }

    pub fn create_session(&mut self) {
        let Some(request) = self.session_request() else {
            return;
        };
        let plan_request = PlanRequest {
            grill: self.form.options.planning.grill,
            formal_spec: self.form.options.planning.formal_spec,
            skip_planning: self.form.options.planning.skip_planning,
            no_interactive_planning: false,
            interactive: Interactive::new(true),
            ..PlanRequest::default()
        };
        if self.registry.create(
            self.application.clone(),
            request,
            plan_request,
            self.events.clone(),
        ) {
            self.status = Some("Creating session".to_string());
        } else {
            self.set_error("Session creation is already active".to_string());
        }
    }

    pub fn save_as_draft(&mut self) {
        let Some(request) = self.session_request() else {
            return;
        };
        if self
            .registry
            .create_draft(self.application.clone(), request, self.events.clone())
        {
            self.status = Some("Saving draft".to_string());
        } else {
            self.set_error("Session creation is already active".to_string());
        }
    }

    pub fn autosave_draft(&mut self, now: Instant) {
        if self.form.should_autosave(now) {
            if let Err(error) = self.application.save_draft(&self.form.draft()) {
                self.status = Some(format!("Draft autosave failed: {error}"));
            } else {
                self.form.mark_saved();
                self.status = Some("Draft autosaved".to_string());
            }
        }
    }

    pub fn on_resize(&mut self, width: u16, height: u16) {
        let too_small = width < 80 || height < 24;
        if too_small {
            if !self.modal_state.resized {
                self.modal_state.resize_had_prompt =
                    matches!(self.modal.as_ref(), Some(Modal::Prompt));
            }
            self.modal_state.resized = true;
            self.modal = Some(Modal::Resize);
        } else {
            self.modal_state.resized = false;
            if matches!(self.modal.as_ref(), Some(Modal::Resize)) {
                self.modal = None;
            }
            if self.modal_state.resize_had_prompt && self.prompts.active.is_some() {
                self.modal = Some(Modal::Prompt);
            }
            self.modal_state.resize_had_prompt = false;
            self.open_prompt_if_allowed();
        }
    }
}

fn push_bounded(buffer: &mut VecDeque<String>, line: &str, limit: usize) {
    for part in line.split('\n') {
        buffer.push_back(part.to_string());
        while buffer.len() > limit {
            buffer.pop_front();
        }
    }
}
fn operation_label(operation: crate::application::OperationKind) -> &'static str {
    match operation {
        crate::application::OperationKind::Generate => "Generate",
        crate::application::OperationKind::Fix => "Fix",
        crate::application::OperationKind::Ask => "Ask",
        crate::application::OperationKind::Replan => "Replan",
        crate::application::OperationKind::Run => "Run",
        crate::application::OperationKind::BatchQueued => "Batch queued",
        crate::application::OperationKind::BatchRun => "Batch run",
        crate::application::OperationKind::Mutate => "Update",
    }
}

#[must_use]
pub fn action_label(action: SessionAction) -> &'static str {
    match action {
        SessionAction::Generate => "Generate Plan",
        SessionAction::Answer => "Answer Prompt",
        SessionAction::Cancel => "Cancel",
        SessionAction::Delete => "Delete",
        SessionAction::Approve => "Approve",
        SessionAction::Publish => "Publish Issue",
        SessionAction::Fix => "Fix Plan",
        SessionAction::Ask => "Ask About Plan",
        SessionAction::Discard => "Discard",
        SessionAction::RunWorktree => "Run in Worktree",
        SessionAction::RunCurrentBranch => "Run on Current Branch",
        SessionAction::Replan => "Replan",
        SessionAction::EditSettings => "Edit Settings",
        SessionAction::Retry => "Retry",
        SessionAction::ResetToPlanned => "Reset to Planned",
        SessionAction::EditCurrentStep => "Edit Current Step",
        SessionAction::Resume => "Resume",
        SessionAction::OpenPr => "Open Pull Request",
    }
}

fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let command = "open";
    #[cfg(not(target_os = "macos"))]
    let command = "xdg-open";
    std::process::Command::new(command)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn app_with_lock(test_process_lock: Option<crate::test_support::ProcessLock>) -> TuiApp {
        let temp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let application = CruiseApplication::new(crate::session::SessionManager::new(
            temp.path().to_path_buf(),
        ));
        let (events, _) = tokio::sync::mpsc::unbounded_channel();
        let (logs_sender, _) = tokio::sync::mpsc::channel(2);
        let mut app = TuiApp::new(application, events, logs_sender);
        app.test_process_lock = test_process_lock;
        app
    }

    fn app() -> TuiApp {
        app_with_lock(Some(crate::test_support::lock_process()))
    }

    fn app_without_lock() -> TuiApp {
        app_with_lock(None)
    }

    fn add_session(app: &mut TuiApp, id: &str, phase: crate::session::SessionPhase) {
        let mut state = SessionState::new(
            id.to_string(),
            PathBuf::from("."),
            "cruise.yaml".to_string(),
            format!("task {id}"),
        );
        state.phase = phase;
        app.sessions.push(state);
    }

    fn pending_ask(request_id: &str) -> crate::application::PendingPrompt {
        crate::application::PendingPrompt {
            request_id: request_id.to_string(),
            session_id: "session".to_string(),
            kind: PendingPromptKind::Ask,
            question: Some("What should happen next?".to_string()),
            choices: vec![],
        }
    }

    #[test]
    fn detail_tabs_cycle_in_both_directions() {
        assert_eq!(DetailTab::Info.next(), DetailTab::Dag);
        assert_eq!(DetailTab::Info.previous(), DetailTab::Log);
    }

    #[test]
    fn new_session_shortcut_starts_the_dialogue_at_the_task_question() {
        let mut app = app();
        app.form.step = Step::Config;
        assert!(!app.handle_action(Action::NewSession));
        assert_eq!(app.view, View::NewSession);
        assert_eq!(app.form.step, Step::Task);
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(app: &mut TuiApp, text: &str) {
        for character in text.chars() {
            assert!(!app.handle_key(key(KeyCode::Char(character))));
        }
    }

    fn write_workflow(path: &std::path::Path, step: &str) {
        let yaml = format!("command: [echo]\nsteps:\n  {step}:\n    command: echo {step}\n");
        std::fs::write(path, yaml).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn task_question_takes_typed_text_and_enter_inserts_a_newline() {
        let mut app = app();
        app.handle_action(Action::NewSession);
        type_text(&mut app, "quit now jk");
        assert!(!app.handle_key(key(KeyCode::Enter)));
        type_text(&mut app, "second");
        assert_eq!(app.view, View::NewSession);
        assert_eq!(app.form.step, Step::Task);
        assert_eq!(app.form.input.text(), "quit now jk\nsecond");
        assert!(app.form.dirty);
    }

    #[test]
    fn tab_and_ctrl_enter_advance_while_shift_tab_and_escape_go_back() {
        let mut app = app();
        app.handle_action(Action::NewSession);
        type_text(&mut app, "task");
        assert!(!app.handle_key(key(KeyCode::Tab)));
        assert_eq!(app.form.step, Step::Attachments);
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)));
        assert_eq!(app.form.step, Step::Source);
        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert_eq!(app.form.step, Step::WorkingDirectory);
        assert!(!app.handle_key(key(KeyCode::BackTab)));
        assert_eq!(app.form.step, Step::Source);
        assert!(!app.handle_key(key(KeyCode::Esc)));
        assert_eq!(app.form.step, Step::Attachments);
        assert_eq!(app.view, View::NewSession);
        assert!(!app.handle_key(key(KeyCode::Esc)));
        assert_eq!(app.form.step, Step::Task);
        assert!(!app.handle_key(key(KeyCode::Esc)));
        assert_eq!(app.view, View::Sessions, "Esc at the first question leaves");
        assert!(!app.handle_key(key(KeyCode::Char('2'))));
        assert_eq!(app.view, View::NewSession);
        assert_eq!(app.form.input.text(), "task", "answers survive leaving");
        assert!(app.modal.is_none());
        assert!(app.registry.tasks_empty());
    }

    #[test]
    fn leaving_the_image_question_requires_a_task_or_an_image() {
        let mut app = app();
        app.handle_action(Action::NewSession);
        assert!(!app.handle_key(key(KeyCode::Tab)));
        assert_eq!(app.form.step, Step::Attachments);
        assert!(!app.handle_key(key(KeyCode::Tab)));
        assert_eq!(app.form.step, Step::Attachments);
        assert!(matches!(
            &app.modal,
            Some(Modal::Error(message))
                if message == "Task description or an image attachment is required"
        ));
        app.modal = None;
        type_text(&mut app, "shot.png");
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)));
        assert_eq!(app.form.step, Step::Source);
    }

    #[test]
    fn choosing_github_asks_for_a_repository_and_requires_one() {
        let mut app = app();
        app.handle_action(Action::NewSession);
        app.form.input.set_text("task");
        app.form.step = Step::Source;
        app.github_repositories = vec!["acme/cruise".to_string()];
        assert!(!app.handle_key(key(KeyCode::Char(' '))));
        assert_eq!(app.form.source, SourceKind::GitHub);
        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert_eq!(app.form.step, Step::Repository);
        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert_eq!(app.form.step, Step::Repository);
        assert!(matches!(
            &app.modal,
            Some(Modal::Error(message))
                if message == "Select a GitHub repository before creating a session"
        ));
        app.modal = None;
        type_text(&mut app, "acme/cruise");
        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert_eq!(app.form.step, Step::Config);
    }

    #[test]
    fn arrow_keys_recall_history_on_single_line_questions() {
        let mut app = app();
        app.handle_action(Action::NewSession);
        app.form.step = Step::WorkingDirectory;
        app.form.working_dir.set_text("");
        app.history_summary = Some(crate::application::NewSessionHistorySummary {
            recent_working_dirs: vec!["/tmp/one".to_string(), "/tmp/two".to_string()],
            last_requested_config_path: None,
            last_working_dir: None,
        });
        assert!(!app.handle_key(key(KeyCode::Down)));
        assert_eq!(app.form.working_dir.text(), "/tmp/one");
        assert!(!app.handle_key(key(KeyCode::Down)));
        assert_eq!(app.form.working_dir.text(), "/tmp/two");
        assert!(!app.handle_key(key(KeyCode::Up)));
        assert_eq!(app.form.working_dir.text(), "/tmp/one");

        app.form.step = Step::Repository;
        app.form.source = SourceKind::GitHub;
        app.github_repositories = vec!["acme/a".to_string(), "acme/b".to_string()];
        assert!(!app.handle_key(key(KeyCode::Char('k'))));
        assert_eq!(
            app.form.repository.text(),
            "k",
            "letters type even when they double as navigation keys"
        );
        assert!(!app.handle_key(key(KeyCode::Up)));
        assert_eq!(app.form.repository.text(), "acme/a");
    }

    #[test]
    fn config_candidates_follow_the_working_directory_when_entering_config_step() {
        let _lock = crate::test_support::lock_process();
        let fake_home = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let first_dir = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let second_dir = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        write_workflow(&first_dir.path().join("cruise.yaml"), "first");
        write_workflow(&second_dir.path().join("cruise.yaml"), "second");

        let mut app = app_without_lock();
        app.view = View::NewSession;
        app.form.config.set_text("");
        app.form
            .working_dir
            .set_text(&first_dir.path().to_string_lossy());
        app.form.step = Step::WorkingDirectory;
        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert_eq!(app.form.step, Step::Config);
        assert!(!app.handle_key(key(KeyCode::Down)));
        assert_eq!(
            app.form.config.text(),
            first_dir.path().join("cruise.yaml").to_string_lossy()
        );

        app.form.config.set_text("");
        app.form
            .working_dir
            .set_text(&second_dir.path().to_string_lossy());
        app.form.step = Step::WorkingDirectory;
        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert_eq!(app.form.step, Step::Config);
        assert!(!app.handle_key(key(KeyCode::Down)));
        assert_eq!(
            app.form.config.text(),
            second_dir.path().join("cruise.yaml").to_string_lossy()
        );
    }

    #[test]
    fn config_selection_refreshes_skipped_step_choices_immediately() {
        let _lock = crate::test_support::lock_process();
        let fake_home = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let repo_dir = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let cruise_dir = repo_dir.path().join(".cruise");
        std::fs::create_dir_all(&cruise_dir).unwrap_or_else(|error| panic!("{error}"));
        let first = repo_dir.path().join("cruise.yaml");
        let second = cruise_dir.join("second.yaml");
        write_workflow(&first, "first_step");
        write_workflow(&second, "second_step");

        let mut app = app_without_lock();
        app.view = View::NewSession;
        app.form.input.set_text("task");
        app.form
            .working_dir
            .set_text(&repo_dir.path().to_string_lossy());
        app.form.config.set_text(&first.to_string_lossy());
        app.form.step = Step::Config;
        app.refresh();
        assert!(
            app.skip_choices()
                .iter()
                .any(|(label, _)| label == "first_step")
        );

        assert!(!app.handle_key(key(KeyCode::Down)));
        assert_eq!(app.form.config.text(), second.to_string_lossy());
        assert!(
            app.skip_choices()
                .iter()
                .any(|(label, _)| label == "second_step")
        );
        assert!(
            !app.skip_choices()
                .iter()
                .any(|(label, _)| label == "first_step")
        );
    }

    #[test]
    fn github_auto_detect_waits_for_the_clone_before_loading_config_defaults() {
        let project = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        write_workflow(&project.path().join("cruise.yaml"), "local_only");

        let mut app = app();
        app.view = View::NewSession;
        app.form.input.set_text("task");
        app.form.source = SourceKind::GitHub;
        app.form.repository.set_text("acme/cruise");
        app.form
            .working_dir
            .set_text(&project.path().to_string_lossy());
        app.form.config.set_text("");
        app.form.step = Step::Config;
        app.refresh();

        assert!(
            !app.config_sources
                .iter()
                .any(crate::resolver::ConfigCandidate::is_local),
            "caller-local configs must not be offered for GitHub sessions"
        );
        assert!(
            app.config_defaults.is_none(),
            "auto-detect must wait until the repository clone exists"
        );
        assert!(app.skip_choices().is_empty());

        let request = app
            .session_request()
            .unwrap_or_else(|| panic!("expected a valid GitHub request"));
        assert!(
            request.skipped_steps.is_empty(),
            "auto-detect must not copy skip ids from the caller-local config"
        );
    }

    #[test]
    fn invalid_config_path_blocks_leaving_config_step_and_shows_error() {
        let mut app = app();
        app.view = View::NewSession;
        app.form.input.set_text("task");
        app.form.step = Step::Config;
        app.form.config.set_text("/path/that/does/not/exist.yaml");
        app.config_defaults = None;

        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert_eq!(app.form.step, Step::Config);
        assert!(matches!(
            &app.modal,
            Some(Modal::Error(message)) if message.contains("does/not/exist.yaml")
        ));
    }

    #[test]
    fn invalid_config_path_blocks_launch_shortcuts_from_config_step() {
        for shortcut in ['p', 'g', 'u', 's'] {
            let mut app = app();
            app.view = View::NewSession;
            app.form.input.set_text("task");
            app.form.step = Step::Config;
            app.form.config.set_text("/path/that/does/not/exist.yaml");

            assert!(!app.handle_key(KeyEvent::new(
                KeyCode::Char(shortcut),
                KeyModifiers::CONTROL,
            )));
            assert_eq!(app.form.step, Step::Config);
            assert!(matches!(
                &app.modal,
                Some(Modal::Error(message)) if message.contains("does/not/exist.yaml")
            ));
            assert!(app.registry.tasks_empty());
        }
    }

    #[test]
    fn valid_config_path_is_resolved_before_leaving_config_step() {
        let config_dir = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let config_path = config_dir.path().join("custom.yaml");
        write_workflow(&config_path, "custom_step");
        let expected_key =
            crate::new_session_history::resolved_config_key_for_session(&config_path);

        let mut app = app();
        app.view = View::NewSession;
        app.form.input.set_text("task");
        app.form.step = Step::Config;
        app.form.config.set_text(&config_path.to_string_lossy());
        app.config_defaults = None;

        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert_eq!(app.form.step, Step::SkippedSteps);
        assert_eq!(
            app.config_defaults
                .as_ref()
                .map(|defaults| defaults.resolved_config_key.as_str()),
            Some(expected_key.as_str())
        );
    }

    #[test]
    fn config_refresh_preserves_an_arbitrary_path_not_in_the_candidate_list() {
        let mut app = app();
        app.view = View::NewSession;
        app.form.step = Step::Config;
        app.form.config.set_text("custom/path/workflow.yaml");
        app.refresh();
        assert_eq!(app.form.config.text(), "custom/path/workflow.yaml");
    }

    #[test]
    fn j_and_k_remain_text_input_on_the_config_question() {
        let mut app = app();
        app.view = View::NewSession;
        app.form.step = Step::Config;
        app.form.config.set_text("");

        assert!(!app.handle_key(key(KeyCode::Char('j'))));
        assert!(!app.handle_key(key(KeyCode::Char('k'))));
        assert_eq!(app.form.config.text(), "jk");
    }

    #[test]
    fn invalid_cruise_config_is_reported_when_tui_refreshes_candidates() {
        let _lock = crate::test_support::lock_process();
        let fake_home = tempfile::TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let _home_guards = crate::test_support::set_fake_home(fake_home.path());
        let missing = fake_home.path().join("missing-cruise-config.yaml");
        let _env_guard = crate::test_support::EnvGuard::set("CRUISE_CONFIG", &missing);

        let app = app_without_lock();
        assert!(matches!(
            &app.modal,
            Some(Modal::Error(message)) if message.contains("missing-cruise-config.yaml")
        ));
    }

    #[test]
    fn skipped_step_question_toggles_choices_with_space() {
        let mut app = app();
        app.handle_action(Action::NewSession);
        let step = |id: &str| crate::workflow::SkippableStepNode {
            id: id.to_string(),
            expanded_step_ids: vec![id.to_string()],
            children: Vec::new(),
        };
        app.config_defaults = Some(crate::application::NewSessionConfigDefaults {
            steps: vec![step("build"), step("review")],
            after_pr_steps: Vec::new(),
            default_skipped_steps: Vec::new(),
            resolved_config_key: "test".to_string(),
        });
        app.form.step = Step::SkippedSteps;
        assert!(!app.handle_key(key(KeyCode::Char('j'))));
        assert_eq!(app.skip_cursor, 1);
        assert!(!app.handle_key(key(KeyCode::Char(' '))));
        assert_eq!(
            app.form.selected_skipped_steps(),
            vec!["review".to_string()]
        );
        assert!(app.form.skipped_explicit);
        assert!(!app.handle_key(key(KeyCode::Char(' '))));
        assert!(app.form.selected_skipped_steps().is_empty());
    }

    #[test]
    fn current_branch_workspace_adds_the_dirty_tree_question() {
        let mut app = app();
        app.handle_action(Action::NewSession);
        app.form.input.set_text("task");
        app.form.step = Step::Workspace;
        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert_eq!(app.form.step, Step::FormalSpec);
        assert!(!app.handle_key(key(KeyCode::BackTab)));
        assert!(!app.handle_key(key(KeyCode::Down)));
        assert_eq!(app.form.workspace_mode, WorkspaceMode::CurrentBranch);
        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert_eq!(app.form.step, Step::DirtyTree);
        assert!(!app.handle_key(key(KeyCode::Char(' '))));
        assert!(app.form.options.allow_dirty_working_tree);
        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert_eq!(app.form.step, Step::FormalSpec);
    }

    #[test]
    fn planning_shortcuts_select_mode_and_submit_from_any_question() {
        let cases = [
            ('p', Launch::Planning, (false, true, false)),
            ('g', Launch::Grill, (true, true, false)),
            ('u', Launch::InputPlan, (false, false, true)),
        ];
        for (key, launch, expected) in cases {
            let mut app = app();
            app.handle_action(Action::NewSession);
            app.form.step = Step::Config;
            let planning = &mut app.form.options.planning;
            planning.grill = true;
            planning.formal_spec = true;
            planning.skip_planning = true;

            assert!(!app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL,)));
            assert_eq!(app.form.launch, launch);
            let planning = &app.form.options.planning;
            assert_eq!(
                (planning.grill, planning.formal_spec, planning.skip_planning),
                expected
            );
            assert!(matches!(
                &app.modal,
                Some(Modal::Error(message))
                    if message == "Task description or an image attachment is required"
            ));
        }
    }

    #[test]
    fn launch_question_submits_the_highlighted_mode_with_enter() {
        let mut app = app();
        app.handle_action(Action::NewSession);
        app.form.step = Step::Launch;
        assert!(!app.handle_key(key(KeyCode::Down)));
        assert!(!app.handle_key(key(KeyCode::Down)));
        assert_eq!(app.form.launch, Launch::InputPlan);
        assert!(!app.handle_key(key(KeyCode::Enter)));
        assert!(app.form.options.planning.skip_planning);
        assert!(matches!(
            &app.modal,
            Some(Modal::Error(message))
                if message == "Task description or an image attachment is required"
        ));
        assert_eq!(
            app.form.step,
            Step::Launch,
            "a rejected launch stays on its question"
        );
    }

    #[test]
    fn planning_and_draft_shortcuts_do_not_edit_open_prompts() {
        let mut app = app();
        app.prompts.enqueue(pending_ask("ask-1").into());
        app.handle_action(Action::Open);
        for key in ['p', 'g', 'u', 's'] {
            assert!(!app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL,)));
            assert!(matches!(app.modal, Some(Modal::Prompt)));
            assert!(app.prompts.answer.text().is_empty());
        }
        assert!(!app.form.options.planning.grill);
    }

    #[test]
    fn bounded_log_keeps_latest_lines() {
        let mut buffer = VecDeque::new();
        for line in 0..SESSION_LOG_LIMIT + 5 {
            let line = line.to_string();
            push_bounded(&mut buffer, &line, SESSION_LOG_LIMIT);
        }
        assert_eq!(buffer.len(), SESSION_LOG_LIMIT);
        assert_eq!(buffer.front().map(String::as_str), Some("5"));
    }

    #[test]
    fn open_key_opens_next_queued_prompt() {
        let mut app = app();
        app.prompts.enqueue(pending_ask("ask-1").into());
        assert!(!app.handle_action(Action::Open));
        assert_eq!(
            app.prompts
                .active
                .as_ref()
                .map(|prompt| prompt.request_id.as_str()),
            Some("ask-1")
        );
        assert!(matches!(app.modal, Some(Modal::Prompt)));
    }
    #[tokio::test]
    async fn option_prompt_waits_for_run_all_until_open_key() {
        let mut app = app();
        let application = app.application.clone();
        let events = app.events.clone();
        let logs_sender = app.logs_sender.clone();
        assert!(app.registry.run_all(application, 1, events, logs_sender));
        app.apply_event(UiEvent::Control(ApplicationEvent::OptionRequired {
            session_id: "session".to_string(),
            request_id: "option-1".to_string(),
            prompt: "Choose a deployment target".to_string(),
            choices: vec![crate::application::OptionChoicePayload {
                label: "staging".to_string(),
                kind: crate::application::OptionChoiceKind::Selector,

                next_step: None,
            }],
        }));
        assert!(
            app.modal.is_none(),
            "Run All keeps option prompts queued instead of stealing focus"
        );
        app.handle_action(Action::Open);
        assert!(matches!(app.modal, Some(Modal::Prompt)));
        assert_eq!(
            app.prompts
                .active
                .as_ref()
                .map(|prompt| prompt.request_id.as_str()),
            Some("option-1")
        );
        app.registry.shutdown().await;
    }

    #[test]
    fn empty_prompt_answer_stays_queued_and_reopens_after_error() {
        let mut app = app();
        app.prompts.enqueue(pending_ask("ask-1").into());
        app.handle_action(Action::Open);

        assert!(!app.handle_action(Action::Enter));
        assert!(app.prompts.active.is_some());
        assert!(matches!(app.modal, Some(Modal::Error(_))));

        app.handle_action(Action::Enter);
        assert!(app.prompts.active.is_some());
        assert!(matches!(app.modal, Some(Modal::Prompt)));
    }

    #[test]
    fn resize_replaces_but_then_restores_prompt_modal() {
        let mut app = app();
        app.prompts.enqueue(pending_ask("ask-1").into());
        app.handle_action(Action::Open);
        app.prompts.answer.set_text("keep this answer");
        app.on_resize(79, 24);
        assert!(app.prompts.active.is_some());
        assert_eq!(app.prompts.answer.text(), "keep this answer");
        assert!(matches!(app.modal, Some(Modal::Resize)));
        app.on_resize(80, 24);
        assert!(matches!(app.modal, Some(Modal::Prompt)));
    }

    #[test]
    fn syncing_a_resolved_active_prompt_closes_every_prompt_ui_state() {
        let mut app = app();
        add_session(
            &mut app,
            "session",
            crate::session::SessionPhase::AwaitingInput,
        );
        app.prompts.enqueue(pending_ask("ask-1").into());
        app.handle_action(Action::Open);
        app.modal_state.prompt_modal_pending = true;

        app.sync_prompts();

        assert!(app.prompts.active.is_none());
        assert!(app.modal.is_none());
        assert!(!app.modal_state.prompt_modal_pending);
    }

    #[test]
    fn ctrl_c_with_active_work_opens_quit_confirmation_then_cancels() {
        let mut app = app();
        add_session(&mut app, "active", crate::session::SessionPhase::Running);
        let claim = app
            .application
            .runtime()
            .try_begin("active", crate::application::OperationKind::Run)
            .unwrap_or_else(|error| panic!("{error}"));

        assert!(!app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(matches!(
            app.modal,
            Some(Modal::Confirm {
                command: PendingCommand::Quit,
                ..
            })
        ));
        assert!(!app.operation_state.quit_requested);
        assert!(!app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(app.operation_state.quit_requested);
        drop(claim);
    }

    #[test]
    fn run_all_requires_confirmation() {
        let mut app = app();
        app.view = View::RunAll;
        app.handle_action(Action::Palette);
        assert!(matches!(
            app.modal,
            Some(Modal::Confirm {
                command: PendingCommand::RunAll,
                ..
            })
        ));
        app.handle_action(Action::Escape);
        assert!(app.modal.is_none());
    }

    #[test]
    fn run_all_rows_and_logs_keep_stable_ids_and_bounds() {
        let mut app = app();
        for index in 0..BATCH_LOG_LIMIT + 5 {
            app.append_log(
                Some("session-42".to_string()),
                crate::application::EventStream::Info,
                &format!("line-{index}"),
                true,
            );
        }
        assert_eq!(app.batch_logs.len(), BATCH_LOG_LIMIT);
        assert!(
            app.batch_logs
                .front()
                .is_some_and(|line| line.contains("line-5"))
        );
        assert!(
            app.batch_logs
                .iter()
                .all(|line| line.contains("[session-42]"))
        );

        app.batch_rows.push(BatchRow {
            id: "session-42".to_string(),
            title: "task".to_string(),
            phase: "Running".to_string(),
            finished: false,
        });
        let event = |phase: &str| {
            UiEvent::Control(ApplicationEvent::BatchSessionFinished {
                id: "session-42".to_string(),
                phase: phase.to_string(),
                error: None,
            })
        };
        app.apply_event(event("Completed"));
        app.apply_event(event("Completed"));
        assert_eq!(app.batch_finished, 1);
        assert_eq!(app.batch_rows.len(), 1);
        assert_eq!(app.batch_rows[0].id, "session-42");
        assert!(app.batch_rows[0].finished);
    }

    #[test]
    fn plan_and_log_navigation_controls_follow_state() {
        let mut app = app();
        add_session(&mut app, "session", crate::session::SessionPhase::Planned);
        app.plan_cache.insert(
            "session".to_string(),
            "line 1\nline 2\nline 3\nline 4".to_string(),
        );
        app.logs.insert(
            "session".to_string(),
            (0..10).map(|index| format!("line-{index}")).collect(),
        );

        app.tab = DetailTab::Plan;
        app.update_plan_scroll(-2);
        assert_eq!(app.plan_scroll, 0);
        app.handle_action(Action::End);
        assert_eq!(app.plan_scroll, 3);

        app.tab = DetailTab::Log;
        app.display.follow_log = true;
        app.update_log_scroll(-3);
        assert!(!app.display.follow_log);
        assert_eq!(app.log_scroll, 3);
        app.append_log(
            Some("session".to_string()),
            crate::application::EventStream::Stdout,
            "new",
            false,
        );
        assert_eq!(
            app.log_scroll, 3,
            "paused logs retain their scroll position"
        );
        app.handle_action(Action::Follow);
        app.append_log(
            Some("session".to_string()),
            crate::application::EventStream::Stdout,
            "latest",
            false,
        );
        assert!(app.display.follow_log);
        assert_eq!(app.log_scroll, 0);
    }
    #[test]
    fn visible_lines_keeps_only_viewport_and_ask_response() {
        let mut app = app();
        add_session(&mut app, "session", crate::session::SessionPhase::Planned);
        app.logs.insert(
            "session".to_string(),
            (0..10).map(|index| format!("log-{index}")).collect(),
        );
        app.ask_responses.insert(
            "session".to_string(),
            ["[info] answer".to_string()].into_iter().collect(),
        );
        app.log_scroll = 0;
        assert_eq!(
            app.visible_lines(2),
            vec!["log-9".to_string(), "[info] answer".to_string()]
        );
    }

    #[test]
    fn replan_opens_feedback_input() {
        let mut app = app();
        add_session(&mut app, "planned", crate::session::SessionPhase::Planned);
        app.apply_action(SessionAction::Replan);
        assert!(matches!(
            app.modal,
            Some(Modal::Input {
                command: PendingCommand::Session(SessionAction::Replan),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn replan_dispatches_feedback_work_without_running_llm() {
        let mut app = app();
        add_session(&mut app, "planned", crate::session::SessionPhase::Planned);
        app.apply_input_command(
            PendingCommand::Session(SessionAction::Replan),
            "change the deployment step".to_string(),
            false,
        );
        assert_eq!(app.status.as_deref(), Some("Replanning started"));
        assert!(
            app.registry.busy("planned"),
            "replan should reserve the session operation slot"
        );
        app.registry.shutdown().await;
    }

    #[test]
    fn new_session_rejects_empty_input_without_image() {
        let mut app = app();
        app.view = View::NewSession;
        app.form.input.set_text("  ");
        app.form.attachments.set_text("");
        app.create_session();
        assert!(
            matches!(app.modal, Some(Modal::Error(message)) if message == "Task description or an image attachment is required")
        );
    }

    #[test]
    fn replan_rejects_empty_feedback_before_dispatch() {
        let mut app = app();
        add_session(&mut app, "planned", crate::session::SessionPhase::Planned);
        app.apply_input_command(
            PendingCommand::Session(SessionAction::Replan),
            "  ".to_string(),
            false,
        );
        assert!(
            matches!(app.modal, Some(Modal::Error(message)) if message == "Replan feedback must not be empty")
        );
        assert_eq!(app.application.runtime().active_operation("planned"), None);
    }

    #[test]
    fn empty_session_quit_is_immediate() {
        let mut app = app();
        assert!(app.handle_action(Action::Quit));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn new_session_passes_formal_spec_only_to_its_initial_plan_request() {
        let _lock = crate::test_support::lock_process();
        let temp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let home = temp.path().join("home");
        std::fs::create_dir_all(&home).unwrap_or_else(|error| panic!("{error}"));
        let _home = crate::test_support::set_fake_home(&home);
        let config = temp.path().join("cruise.yaml");
        std::fs::write(&config, "command: [cat]\nsteps:\n  s1:\n    prompt: plan\n")
            .unwrap_or_else(|error| panic!("{error}"));

        let application = CruiseApplication::new(crate::session::SessionManager::new(
            temp.path().join("sessions"),
        ));
        let (events, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let (logs_sender, _) = tokio::sync::mpsc::channel(2);
        let mut app = TuiApp::new(application, events, logs_sender);
        app.view = View::NewSession;
        app.form.input.set_text("formal TUI task");
        app.form
            .working_dir
            .set_text(&temp.path().to_string_lossy());
        app.form.config.set_text(&config.to_string_lossy());
        app.form.options.planning.formal_spec = true;

        app.create_session();

        let event = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap_or_else(|error| panic!("timed out waiting for session creation: {error}"))
            .unwrap_or_else(|| panic!("session creation channel closed"));
        let UiEvent::SessionCreated { result, plan } = event else {
            panic!("expected SessionCreated event, got another event");
        };
        assert!(result.is_ok(), "session creation failed: {result:?}");
        assert!(plan.formal_spec);
        app.registry.shutdown().await;
    }

    fn take_one_notification(
        app: &mut TuiApp,
    ) -> crate::desktop_notifications::NotificationPayload {
        let mut notifications = app.take_notifications();
        assert_eq!(notifications.len(), 1);
        notifications
            .next()
            .unwrap_or_else(|| panic!("expected one queued notification"))
    }

    #[test]
    fn ask_user_required_enqueues_one_action_required_notification() {
        let mut app = app();
        add_session(
            &mut app,
            "session",
            crate::session::SessionPhase::AwaitingInput,
        );
        app.sessions[0].title = Some("Review deployment plan".to_string());

        app.apply_event(UiEvent::Control(ApplicationEvent::AskUserRequired {
            session_id: "session".to_string(),
            request_id: "ask-1".to_string(),
            question: "Which provider should be used?".to_string(),
        }));

        let notification = take_one_notification(&mut app);
        assert_eq!(
            notification.kind,
            crate::desktop_notifications::WorkflowNotificationKind::ActionRequired
        );
        assert!(notification.body.contains("Which provider should be used?"));
        assert!(notification.body.contains("Review deployment plan"));
    }

    #[test]
    fn option_required_enqueues_one_action_required_notification() {
        let mut app = app();
        add_session(&mut app, "session", crate::session::SessionPhase::Running);

        app.apply_event(UiEvent::Control(ApplicationEvent::OptionRequired {
            session_id: "session".to_string(),
            request_id: "option-1".to_string(),
            prompt: "Choose a deployment target".to_string(),
            choices: vec![],
        }));

        let notification = take_one_notification(&mut app);
        assert_eq!(
            notification.kind,
            crate::desktop_notifications::WorkflowNotificationKind::ActionRequired
        );
        assert!(notification.body.contains("Choose a deployment target"));
    }

    #[test]
    fn generate_plan_finished_awaiting_approval_enqueues_plan_ready() {
        let mut app = app();
        add_session(
            &mut app,
            "session",
            crate::session::SessionPhase::AwaitingApproval,
        );

        app.apply_event(UiEvent::Control(ApplicationEvent::PlanStarted {
            session_id: "session".to_string(),
            operation: crate::application::OperationKind::Generate,
        }));
        app.apply_event(UiEvent::Control(ApplicationEvent::PlanFinished {
            session_id: "session".to_string(),
            phase: "Awaiting Approval".to_string(),
        }));

        let notification = take_one_notification(&mut app);
        assert_eq!(
            notification.kind,
            crate::desktop_notifications::WorkflowNotificationKind::PlanReady
        );
    }

    #[test]
    fn fix_plan_finished_awaiting_approval_enqueues_plan_ready() {
        let mut app = app();
        add_session(
            &mut app,
            "session",
            crate::session::SessionPhase::AwaitingApproval,
        );

        app.apply_event(UiEvent::Control(ApplicationEvent::PlanStarted {
            session_id: "session".to_string(),
            operation: crate::application::OperationKind::Fix,
        }));
        app.apply_event(UiEvent::Control(ApplicationEvent::PlanFinished {
            session_id: "session".to_string(),
            phase: "Awaiting Approval".to_string(),
        }));

        let notification = take_one_notification(&mut app);
        assert_eq!(
            notification.kind,
            crate::desktop_notifications::WorkflowNotificationKind::PlanReady
        );
    }

    #[test]
    fn ask_plan_finished_awaiting_approval_does_not_enqueue_plan_ready() {
        let mut app = app();
        add_session(
            &mut app,
            "session",
            crate::session::SessionPhase::AwaitingApproval,
        );

        app.apply_event(UiEvent::Control(ApplicationEvent::PlanStarted {
            session_id: "session".to_string(),
            operation: crate::application::OperationKind::Ask,
        }));
        app.apply_event(UiEvent::Control(ApplicationEvent::PlanFinished {
            session_id: "session".to_string(),
            phase: "Awaiting Approval".to_string(),
        }));

        assert!(app.take_notifications().next().is_none());
    }

    #[test]
    fn ask_plan_finished_does_not_reuse_a_stale_previous_operation() {
        let mut app = app();
        add_session(
            &mut app,
            "session",
            crate::session::SessionPhase::AwaitingApproval,
        );

        app.apply_event(UiEvent::Control(ApplicationEvent::PlanStarted {
            session_id: "session".to_string(),
            operation: crate::application::OperationKind::Generate,
        }));
        // A worker panic reports a generic error rather than a plan terminal
        // event, so the next operation must replace the prior operation kind.
        app.apply_event(UiEvent::Error("planning worker panicked".to_string()));
        app.apply_event(UiEvent::Control(ApplicationEvent::PlanStarted {
            session_id: "session".to_string(),
            operation: crate::application::OperationKind::Ask,
        }));
        app.apply_event(UiEvent::Control(ApplicationEvent::PlanFinished {
            session_id: "session".to_string(),
            phase: "Awaiting Approval".to_string(),
        }));

        assert!(app.take_notifications().next().is_none());
    }

    #[test]
    fn replan_finished_awaiting_approval_enqueues_plan_ready() {
        let mut app = app();
        add_session(
            &mut app,
            "session",
            crate::session::SessionPhase::AwaitingApproval,
        );

        app.apply_event(UiEvent::Control(ApplicationEvent::PlanStarted {
            session_id: "session".to_string(),
            operation: crate::application::OperationKind::Replan,
        }));
        app.apply_event(UiEvent::Control(ApplicationEvent::PlanFinished {
            session_id: "session".to_string(),
            phase: "Awaiting Approval".to_string(),
        }));

        let notification = take_one_notification(&mut app);
        assert_eq!(
            notification.kind,
            crate::desktop_notifications::WorkflowNotificationKind::PlanReady
        );
    }

    #[test]
    fn approved_plan_refresh_enqueues_plan_ready_for_planned_phase() {
        let mut app = app();
        add_session(&mut app, "session", crate::session::SessionPhase::Planned);

        app.apply_event(UiEvent::Control(ApplicationEvent::PlanStarted {
            session_id: "session".to_string(),
            operation: crate::application::OperationKind::Replan,
        }));
        app.apply_event(UiEvent::Control(ApplicationEvent::PlanFinished {
            session_id: "session".to_string(),
            phase: "Planned".to_string(),
        }));

        let notification = take_one_notification(&mut app);
        assert_eq!(
            notification.kind,
            crate::desktop_notifications::WorkflowNotificationKind::PlanReady
        );
    }

    #[test]
    fn plan_failed_enqueues_failed_notification_with_error_detail() {
        let mut app = app();
        add_session(
            &mut app,
            "session",
            crate::session::SessionPhase::Failed("error".to_string()),
        );

        app.apply_event(UiEvent::Control(ApplicationEvent::PlanFailed {
            session_id: "session".to_string(),
            error: "backend unavailable".to_string(),
        }));

        let notification = take_one_notification(&mut app);
        assert_eq!(
            notification.kind,
            crate::desktop_notifications::WorkflowNotificationKind::Failed
        );
        assert!(notification.body.contains("backend unavailable"));
    }

    #[test]
    fn run_failed_enqueues_failed_notification_with_error_detail() {
        let mut app = app();
        add_session(
            &mut app,
            "session",
            crate::session::SessionPhase::Failed("error".to_string()),
        );

        app.apply_event(UiEvent::Control(ApplicationEvent::RunFailed {
            session_id: "session".to_string(),
            error: "command exited with status 1".to_string(),
        }));

        let notification = take_one_notification(&mut app);
        assert_eq!(
            notification.kind,
            crate::desktop_notifications::WorkflowNotificationKind::Failed
        );
        assert!(notification.body.contains("command exited with status 1"));
    }

    #[test]
    fn completed_run_enqueues_completed_notification() {
        let mut app = app();
        add_session(&mut app, "session", crate::session::SessionPhase::Completed);

        app.apply_event(UiEvent::Control(ApplicationEvent::RunFinished {
            session_id: "session".to_string(),
            phase: "Completed".to_string(),
        }));

        let notification = take_one_notification(&mut app);
        assert_eq!(
            notification.kind,
            crate::desktop_notifications::WorkflowNotificationKind::Completed
        );
    }

    #[test]
    fn cancelled_suspended_and_non_terminal_events_do_not_enqueue_notifications() {
        let cases = [
            UiEvent::Control(ApplicationEvent::PlanCancelled {
                session_id: "session".to_string(),
            }),
            UiEvent::Control(ApplicationEvent::RunCancelled {
                session_id: "session".to_string(),
            }),
            UiEvent::Control(ApplicationEvent::PlanFinished {
                session_id: "session".to_string(),
                phase: "Suspended".to_string(),
            }),
            UiEvent::Control(ApplicationEvent::RunFinished {
                session_id: "session".to_string(),
                phase: "Running".to_string(),
            }),
        ];

        for event in cases {
            let mut app = app();
            add_session(&mut app, "session", crate::session::SessionPhase::Running);
            app.apply_event(event);
            assert!(app.take_notifications().next().is_none());
        }
    }

    #[test]
    fn batch_session_finished_does_not_duplicate_worker_completion_notification() {
        let mut app = app();
        add_session(&mut app, "session", crate::session::SessionPhase::Completed);

        app.apply_event(UiEvent::Control(ApplicationEvent::RunFinished {
            session_id: "session".to_string(),
            phase: "Completed".to_string(),
        }));
        app.apply_event(UiEvent::Control(ApplicationEvent::BatchSessionFinished {
            id: "session".to_string(),
            phase: "Completed".to_string(),
            error: None,
        }));
        app.apply_event(UiEvent::Control(ApplicationEvent::BatchFinished {
            cancelled: false,
        }));

        let mut notifications = app.take_notifications();
        assert_eq!(notifications.len(), 1);
        assert_eq!(
            notifications.next().map(|notification| notification.kind),
            Some(crate::desktop_notifications::WorkflowNotificationKind::Completed)
        );
    }

    #[test]
    fn initialization_and_refresh_leave_notification_queue_empty() {
        let mut app = app();
        assert!(app.take_notifications().next().is_none());

        app.refresh();

        assert!(app.take_notifications().next().is_none());
    }
}
