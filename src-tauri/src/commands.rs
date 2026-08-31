use std::path::{Path, PathBuf};
use std::sync::Arc;

use cruise::application::{
    ApplicationEvent, ApplicationEventSink, CruiseApplication, CurrentStepUpdateDto, LogEvent,
    LogSink, NewSessionRequest, OperationKind, PlanRequest, RunRequest, SessionSettingsRequest,
};
use cruise::error::{CruiseError, Result as CruiseResult};
use cruise::new_session_draft::NewSessionDraft;
use cruise::session::{SessionPhase, SessionState, WorkspaceMode};
use cruise::step::option::OptionResult;
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;

use crate::state::AppState;

const BUILTIN_CONFIG_PATH: &str = cruise::new_session_history::BUILTIN_CONFIG_KEY;

fn error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn expanded_path(path: &Path) -> PathBuf {
    PathBuf::from(cruise::new_session_history::expand_tilde(
        &path.to_string_lossy(),
    ))
}

fn normalize_config_path(path: Option<String>) -> Option<String> {
    path.map(|path| cruise::new_session_history::expand_tilde(&path))
}
/// Serializable session representation used by the desktop client. Paths and
/// phase errors are flattened here so the web client never needs to understand
/// Rust-only domain types.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub id: String,
    pub phase: String,
    pub phase_error: Option<String>,
    pub config_source: String,
    pub config_path: Option<String>,
    pub base_dir: String,
    pub repo: Option<String>,
    pub input: String,
    pub title: Option<String>,
    pub current_step: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub worktree_branch: Option<String>,
    pub workspace_mode: WorkspaceMode,
    pub pr_url: Option<String>,
    pub updated_at: Option<String>,
    pub awaiting_input: bool,
    pub pending_ask_question: Option<String>,
    pub plan_error: Option<String>,
    pub exec: bool,
    pub plan_available: bool,
    pub fix_in_progress: bool,
    pub skipped_steps: Vec<String>,
}

fn session_plan_available(state: &SessionState) -> bool {
    let Ok(data_dir) = cruise::paths::data_dir() else {
        return false;
    };
    let sessions_dir = data_dir.join("sessions");
    cruise::metadata::plan_markdown_available(&state.plan_path(&sessions_dir))
}

fn session_dto(
    application: &CruiseApplication,
    state: SessionState,
    resolve_current_step: bool,
) -> SessionDto {
    let plan_available = session_plan_available(&state);
    let (phase, phase_error) = match &state.phase {
        SessionPhase::Failed(message) => ("Failed".to_string(), Some(message.clone())),
        phase => (phase.label().to_string(), None),
    };
    let config_path = state
        .config_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .or_else(|| {
            cruise::resolver::ConfigSource::is_builtin_source(&state.config_source)
                .then(|| BUILTIN_CONFIG_PATH.to_string())
        });
    let current_step = if resolve_current_step && state.current_step_is_node_id {
        state.current_step.as_deref().and_then(|node| {
            application
                .session_dag(&state.id)
                .ok()
                .flatten()
                .and_then(|dag| dag.step_name_for_node(node).map(str::to_owned))
        })
    } else {
        state.current_step.clone()
    };
    let fix_in_progress = matches!(
        application.runtime().active_operation(&state.id),
        Some(OperationKind::Fix)
    );
    SessionDto {
        id: state.id,
        phase,
        phase_error,
        config_source: state.config_source,
        config_path,
        base_dir: state.base_dir.to_string_lossy().into_owned(),
        repo: state.repo,
        input: state.input,
        title: state.title,
        current_step,
        created_at: state.created_at,
        completed_at: state.completed_at,
        worktree_branch: state.worktree_branch,
        workspace_mode: state.workspace_mode,
        pr_url: state.pr_url,
        updated_at: state.updated_at,
        awaiting_input: state.awaiting_input,
        pending_ask_question: state.pending_ask_question,
        plan_error: state.plan_error,
        exec: state.exec,
        plan_available,
        fix_in_progress,
        skipped_steps: state.skipped_steps,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DagDto {
    pub start_step: String,
    pub steps: Vec<DagStepDto>,
    pub edges: Vec<DagEdgeDto>,
    pub current_step: Option<String>,
}

/// Load the session's already-resolved workflow config only for selected DAG detail.
fn session_config(state: &SessionState) -> CruiseResult<cruise::config::WorkflowConfig> {
    let data_dir = cruise::paths::data_dir()?;
    let sessions_dir = data_dir.join("sessions");
    let config_path = state
        .config_path
        .clone()
        .unwrap_or_else(|| sessions_dir.join(&state.id).join("config.yaml"));
    if config_path.is_file() {
        return cruise::workflow_call::resolve_workflow_calls_from_path(config_path);
    }
    if cruise::resolver::ConfigSource::is_builtin_source(&state.config_source) {
        let (yaml, source) =
            cruise::resolver::resolve_config_in_dir(Some(BUILTIN_CONFIG_PATH), &state.base_dir)?;
        return cruise::resolver::resolve_workflow_config(&yaml, &source, &state.base_dir);
    }
    cruise::workflow_call::resolve_workflow_calls_from_path(config_path)
}

fn step_kind(config: &cruise::config::StepConfig) -> String {
    if config.prompt.is_some() {
        "prompt".to_string()
    } else if config.option.is_some() {
        "option".to_string()
    } else if config.command.is_some() {
        "command".to_string()
    } else {
        "unknown".to_string()
    }
}

fn transition_reason(reason: &cruise::dag::TransitionReason) -> (String, Option<String>) {
    use cruise::dag::TransitionReason;
    match reason {
        TransitionReason::Sequential => ("sequential".to_string(), None),
        TransitionReason::Next => ("next".to_string(), None),
        TransitionReason::IfFileChanged { target } => {
            ("ifFileChanged".to_string(), Some(target.clone()))
        }
        TransitionReason::IfNoFileChangesRetry => ("ifNoFileChangesRetry".to_string(), None),
        TransitionReason::IfNoFileChangesFail => ("ifNoFileChangesFail".to_string(), None),
        TransitionReason::IfFailGoto { target } => ("ifFail".to_string(), Some(target.clone())),
        TransitionReason::IfFailRetry => ("ifFailRetry".to_string(), None),
        TransitionReason::OptionChoice { selector } => {
            ("optionChoice".to_string(), Some(selector.clone()))
        }
        TransitionReason::GroupRetry { target } => ("groupRetry".to_string(), Some(target.clone())),
        TransitionReason::GroupRetryExhausted => ("groupRetryExhausted".to_string(), None),
        TransitionReason::SkipFallback => ("skipFallback".to_string(), None),
    }
}

fn build_dag_dto(
    compiled: &cruise::workflow::CompiledWorkflow,
    dag: &cruise::dag::ExecutionDag,
    current_step: Option<&str>,
    current_step_is_node_id: bool,
) -> std::result::Result<DagDto, String> {
    let start_step = dag
        .step_name_for_node(&dag.start)
        .ok_or_else(|| {
            format!(
                "start node '{}' does not map to any workflow step",
                dag.start
            )
        })?
        .to_string();
    let current_step = current_step.and_then(|step| {
        if current_step_is_node_id {
            dag.step_name_for_node(step).map(str::to_owned)
        } else {
            Some(step.to_string())
        }
    });
    let dag_step_names: std::collections::HashSet<String> = dag
        .nodes
        .values()
        .map(|node| node.step_name.clone())
        .collect();
    let mut steps = Vec::new();
    for (name, config) in &compiled.steps {
        if !dag_step_names.contains(name) {
            continue;
        }
        let is_terminal = dag
            .nodes
            .values()
            .filter(|node| node.step_name == *name)
            .any(|node| {
                node.successors
                    .iter()
                    .any(|successor| successor.target.is_none())
            });
        steps.push(DagStepDto {
            name: name.clone(),
            kind: step_kind(config),
            is_terminal,
        });
    }
    let mut seen_edges = std::collections::HashSet::new();
    let mut edges = Vec::new();
    for node in dag.nodes.values() {
        for successor in &node.successors {
            let to = successor
                .target
                .as_deref()
                .and_then(|id| dag.step_name_for_node(id))
                .map(str::to_owned);
            let (reason, selector) = transition_reason(&successor.reason);
            let key = (
                node.step_name.clone(),
                to.clone(),
                reason.clone(),
                selector.clone(),
            );
            if seen_edges.insert(key) {
                edges.push(DagEdgeDto {
                    from: node.step_name.clone(),
                    to,
                    reason,
                    selector,
                });
            }
        }
    }
    Ok(DagDto {
        start_step,
        steps,
        edges,
        current_step,
    })
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DagStepDto {
    pub name: String,
    pub kind: String,
    pub is_terminal: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct DagEdgeDto {
    pub from: String,
    pub to: Option<String>,
    pub reason: String,
    pub selector: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReadinessDto {
    pub can_auto_update: bool,
    pub reason: Option<String>,
    pub bundle_path: Option<String>,
    pub guidance: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigEntrySource {
    Local,
    User,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEntryDto {
    pub path: String,
    pub name: String,
    pub description: Option<String>,
    pub source: Option<ConfigEntrySource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionDraftDto {
    pub input: String,
    #[serde(default)]
    pub config_path: Option<String>,
    pub base_dir: String,
    #[serde(default)]
    pub repo: Option<String>,
    pub skipped_steps: Vec<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl From<NewSessionDraft> for NewSessionDraftDto {
    fn from(value: NewSessionDraft) -> Self {
        Self {
            input: value.input,
            config_path: value.requested_config_path,
            base_dir: value.working_dir,
            repo: value.repo,
            skipped_steps: value.skipped_steps,
            updated_at: Some(value.updated_at),
        }
    }
}

impl From<NewSessionDraftDto> for NewSessionDraft {
    fn from(value: NewSessionDraftDto) -> Self {
        Self {
            input: value.input,
            requested_config_path: value.config_path,
            working_dir: value.base_dir,
            repo: value.repo,
            skipped_steps: value.skipped_steps,
            updated_at: value.updated_at.unwrap_or_default(),
        }
    }
}

#[derive(Clone)]
struct TauriEventSink(Channel<ApplicationEvent>);

impl ApplicationEventSink for TauriEventSink {
    fn send(&self, event: ApplicationEvent) -> CruiseResult<()> {
        self.0
            .send(event)
            .map_err(|e| CruiseError::Other(format!("failed to deliver application event: {e}")))
    }
}

#[derive(Clone)]
struct TauriLogSink(Channel<ApplicationEvent>);

impl LogSink for TauriLogSink {
    fn try_send(&self, event: LogEvent) -> bool {
        self.0
            .send(ApplicationEvent::LogChunk {
                session_id: event.session_id,
                stream: event.stream,
                text: event.text,
                batch: event.batch,
            })
            .is_ok()
    }
}

fn sinks(channel: Channel<ApplicationEvent>) -> (Arc<dyn ApplicationEventSink>, Arc<dyn LogSink>) {
    (
        Arc::new(TauriEventSink(channel.clone())),
        Arc::new(TauriLogSink(channel)),
    )
}

#[tauri::command]
pub fn list_sessions(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Vec<SessionDto>, String> {
    state
        .application
        .list_sessions()
        .map(|sessions| {
            sessions
                .into_iter()
                .map(|session| {
                    state
                        .application
                        .reconcile_session(&session.id)
                        .unwrap_or(session)
                })
                .map(|session| session_dto(&state.application, session, false))
                .collect()
        })
        .map_err(error)
}

#[tauri::command]
pub fn get_session(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    state
        .application
        .reconcile_session(&session_id)
        .map(|session| session_dto(&state.application, session, true))
        .map_err(error)
}

#[tauri::command]
pub fn get_session_plan(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<String, String> {
    state.application.session_plan(&session_id).map_err(error)
}

#[tauri::command]
pub fn get_session_dag(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Option<DagDto>, String> {
    let session = state.application.read_session(&session_id).map_err(error)?;
    let compiled =
        cruise::workflow::compile(session_config(&session).map_err(error)?).map_err(error)?;
    let dag = state.application.session_dag(&session_id).map_err(error)?;
    dag.map(|dag| {
        build_dag_dto(
            &compiled,
            &dag,
            session.current_step.as_deref(),
            session.current_step_is_node_id,
        )
    })
    .transpose()
}

#[tauri::command]
pub fn get_session_log(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<String, String> {
    state
        .application
        .session_log(&session_id, None)
        .map_err(error)
}

#[tauri::command]
pub fn list_directory(
    path: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Vec<cruise::application::DirectoryEntry>, String> {
    Ok(state.application.list_directory(&path))
}

#[tauri::command]
pub async fn list_github_repos(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Vec<String>, String> {
    state
        .application
        .list_github_repositories()
        .await
        .map_err(error)
}

#[tauri::command]
pub fn cancel_session(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<bool, String> {
    Ok(state.application.cancel_session(&session_id))
}

#[tauri::command]
pub fn cancel_run_all(state: tauri::State<'_, AppState>) -> std::result::Result<bool, String> {
    Ok(state.application.cancel_run_all())
}

#[tauri::command]
pub fn respond_to_option(
    session_id: String,
    request_id: String,
    result: OptionResult,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    state
        .application
        .respond_to_option(&session_id, &request_id, result)
        .map_err(error)
}

#[tauri::command]
pub fn respond_to_ask(
    session_id: String,
    request_id: String,
    answer: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    state
        .application
        .respond_to_ask(&session_id, &request_id, answer)
        .map_err(error)
}

#[tauri::command]
pub fn pending_prompts(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Vec<cruise::application::PendingPrompt> {
    state.application.pending_prompts(&session_id)
}

#[tauri::command]
pub async fn clean_sessions(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<cruise::session::CleanupReport, String> {
    let application = state.application.clone();
    tokio::task::spawn_blocking(move || application.clean())
        .await
        .map_err(error)?
        .map_err(error)
}

#[tauri::command]
pub fn get_app_config(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<cruise::app_config::AppConfig, String> {
    state.application.app_config().map_err(error)
}

#[tauri::command]
pub fn update_app_config(
    config: cruise::app_config::AppConfig,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    state.application.save_app_config(&config).map_err(error)
}

#[tauri::command]
pub fn get_new_session_history_summary(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<cruise::application::NewSessionHistorySummary, String> {
    state
        .application
        .new_session_history_summary()
        .map_err(error)
}

#[tauri::command]
pub fn get_new_session_config_defaults(
    base_dir: String,
    config_path: Option<String>,
    repo: Option<String>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<cruise::application::NewSessionConfigDefaults, String> {
    state
        .application
        .new_session_config_defaults(
            &PathBuf::from(cruise::new_session_history::expand_tilde(&base_dir)),
            config_path.as_deref(),
            repo.as_deref(),
        )
        .map_err(error)
}

#[tauri::command]
pub fn get_new_session_draft(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Option<NewSessionDraftDto>, String> {
    state
        .application
        .draft()
        .map(|draft| draft.map(Into::into))
        .map_err(error)
}

#[tauri::command]
pub fn save_new_session_draft(
    draft: NewSessionDraftDto,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    state.application.save_draft(&draft.into()).map_err(error)
}
#[tauri::command]
pub fn clear_new_session_draft(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    state.application.clear_draft().map_err(error)
}

#[tauri::command]
pub fn list_configs(
    base_dir: Option<String>,
    repo: Option<String>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Vec<ConfigEntryDto>, String> {
    let is_repo = repo.is_some();
    let entries = if is_repo {
        state.application.discover_configs()
    } else {
        let base = expanded_path(Path::new(base_dir.as_deref().unwrap_or(".")));
        state.application.discover_config_sources(&base)
    };
    let user_dir = cruise::paths::workflows_dir()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok());
    let mut seen = std::collections::HashSet::new();
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            if entry.path == BUILTIN_CONFIG_PATH {
                return None;
            }
            let path = PathBuf::from(&entry.path);
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if !seen.insert(canonical.clone()) {
                return None;
            }
            let source = if is_repo
                || user_dir
                    .as_ref()
                    .is_some_and(|dir| canonical.starts_with(dir))
            {
                ConfigEntrySource::User
            } else {
                ConfigEntrySource::Local
            };
            Some(ConfigEntryDto {
                source: Some(source),
                path: entry.path,
                name: entry.name,
                description: entry.description,
            })
        })
        .collect())
}

#[tauri::command]
pub fn create_session(
    mut request: NewSessionRequest,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    request.base_dir = expanded_path(&request.base_dir);
    request.config_path = request.config_path.map(|path| expanded_path(&path));
    request.attachments = request
        .attachments
        .into_iter()
        .map(|path| expanded_path(&path))
        .collect();
    state
        .application
        .create_session(request)
        .map(|session| session_dto(&state.application, session, true))
        .map_err(error)
}

#[tauri::command]
pub fn approve_session(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    state
        .application
        .approve(&session_id)
        .map(|session| session_dto(&state.application, session, true))
        .map_err(error)
}

#[tauri::command]
pub fn use_input_as_plan(
    session_id: String,
    channel: Channel<ApplicationEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    let (sink, _) = sinks(channel);
    state
        .application
        .use_input_as_plan(&session_id, &*sink)
        .map(|session| session_dto(&state.application, session, true))
        .map_err(error)
}

#[tauri::command]
pub async fn generate_plan_for_draft(
    session_id: String,
    request: PlanRequest,
    channel: Channel<ApplicationEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    let (sink, _) = sinks(channel);
    state
        .application
        .generate(&session_id, request, sink)
        .await
        .map(|session| session_dto(&state.application, session, true))
        .map_err(error)
}

#[tauri::command]
pub async fn regenerate_session_plan(
    session_id: String,
    request: PlanRequest,
    channel: Channel<ApplicationEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    let (sink, _) = sinks(channel);
    state
        .application
        .replan(&session_id, request, sink)
        .await
        .map(|session| session_dto(&state.application, session, true))
        .map_err(error)
}

#[tauri::command]
pub async fn fix_session(
    session_id: String,
    feedback: String,
    channel: Channel<ApplicationEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    let (sink, _) = sinks(channel);
    state
        .application
        .fix(&session_id, feedback, sink)
        .await
        .map(|session| session_dto(&state.application, session, true))
        .map_err(error)
}

#[tauri::command]
pub async fn ask_session(
    session_id: String,
    question: String,
    channel: Channel<ApplicationEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    let (sink, _) = sinks(channel);
    state
        .application
        .ask(&session_id, question, sink)
        .await
        .map(|session| session_dto(&state.application, session, true))
        .map_err(error)
}

#[tauri::command]
pub fn discard_session(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    state
        .application
        .discard_session(&session_id)
        .map_err(error)
}

#[tauri::command]
pub fn delete_session(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    state.application.delete_session(&session_id).map_err(error)
}

#[tauri::command]
pub fn reset_session(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    state
        .application
        .reset_to_planned(&session_id)
        .map(|session| session_dto(&state.application, session, true))
        .map_err(error)
}

#[tauri::command]
pub fn update_session(
    session_id: String,
    mut request: SessionSettingsRequest,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    request.config_path = normalize_config_path(request.config_path);
    state
        .application
        .update_settings(&session_id, request)
        .map(|session| session_dto(&state.application, session, true))
        .map_err(error)
}

#[tauri::command]
pub fn edit_current_step(
    session_id: String,
    update: CurrentStepUpdateDto,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    state
        .application
        .edit_current_step(&session_id, update)
        .map(|session| session_dto(&state.application, session, true))
        .map_err(error)
}

#[tauri::command]
pub fn publish_plan_issue(
    session_id: String,
    trigger_cruise: bool,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<cruise::issue_publish::PublishedIssue, String> {
    state
        .application
        .publish(&session_id, trigger_cruise)
        .map_err(error)
}

#[tauri::command]
pub async fn run_session(
    session_id: String,
    request: RunRequest,
    channel: Channel<ApplicationEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    let (sink, log_sink) = sinks(channel);
    match state
        .application
        .run_with_log_sink(&session_id, request, sink, Some(log_sink))
        .await
    {
        Ok(session) => Ok(session_dto(&state.application, session, true)),
        Err(CruiseError::Interrupted) => state
            .application
            .reconcile_session(&session_id)
            .map(|session| session_dto(&state.application, session, true))
            .map_err(error),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
pub async fn run_all_sessions(
    parallelism: Option<usize>,
    channel: Channel<ApplicationEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    let application = state.application.clone();
    let parallelism_provider = move || match parallelism {
        Some(value) => Ok(value),
        None => application
            .app_config()
            .map(|config| config.run_all_parallelism),
    };
    let (sink, log_sink) = sinks(channel);
    state
        .application
        .run_all_with_parallelism_provider(parallelism_provider, sink, Some(log_sink))
        .await
        .map(|_| ())
        .map_err(error)
}

#[tauri::command]
pub fn get_update_readiness() -> UpdateReadinessDto {
    std::env::current_exe().map_or_else(
        |_| UpdateReadinessDto {
            can_auto_update: false,
            reason: Some("unknownBundlePath".to_string()),
            bundle_path: None,
            guidance: None,
        },
        |path| check_update_readiness_for_path(&path),
    )
}

pub fn check_update_readiness_for_path(exe_path: &Path) -> UpdateReadinessDto {
    let bundle_path = std::iter::successors(Some(exe_path), |path| path.parent())
        .find(|path| path.to_str().is_some_and(|value| value.ends_with(".app")))
        .map(|path| path.to_string_lossy().into_owned());
    let path = exe_path.to_string_lossy();
    if path.contains("/AppTranslocation/") {
        return UpdateReadinessDto {
            can_auto_update: false,
            reason: Some("translocated".to_string()),
            bundle_path,
            guidance: Some(
                "Move cruise.app to /Applications, then relaunch before updating.".to_string(),
            ),
        };
    }
    if path.starts_with("/Volumes/") {
        return UpdateReadinessDto {
            can_auto_update: false,
            reason: Some("mountedVolume".to_string()),
            bundle_path,
            guidance: Some(
                "Copy cruise.app to /Applications before using auto-update.".to_string(),
            ),
        };
    }
    if bundle_path.is_none() {
        return UpdateReadinessDto {
            can_auto_update: false,
            reason: Some("unknownBundlePath".to_string()),
            bundle_path: None,
            guidance: None,
        };
    }
    UpdateReadinessDto {
        can_auto_update: true,
        reason: None,
        bundle_path,
        guidance: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_dto_serializes_nullable_fields_and_camel_case() {
        let dto = SessionDto {
            id: "session-1".to_string(),
            phase: "Planned".to_string(),
            phase_error: None,
            config_source: "config: (builtin default)".to_string(),
            config_path: Some(BUILTIN_CONFIG_PATH.to_string()),
            base_dir: "/tmp/project".to_string(),
            repo: None,
            input: "task".to_string(),
            title: None,
            current_step: Some("build".to_string()),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            completed_at: None,
            worktree_branch: None,
            workspace_mode: WorkspaceMode::Worktree,
            pr_url: None,
            updated_at: None,
            awaiting_input: false,
            pending_ask_question: None,
            plan_error: None,
            exec: false,
            plan_available: true,
            fix_in_progress: false,
            skipped_steps: vec![],
        };
        let value = serde_json::to_value(dto).expect("session DTO serializes");
        assert_eq!(value["configPath"], BUILTIN_CONFIG_PATH);
        assert!(value["planError"].is_null());
        assert_eq!(value["planAvailable"], true);
        assert_eq!(value["fixInProgress"], false);
        assert_eq!(value["currentStep"], "build");
    }

    #[test]
    fn auto_config_values_preserve_explicit_auto_selection() {
        assert_eq!(
            normalize_config_path(Some(String::new())),
            Some(String::new())
        );
        assert_eq!(normalize_config_path(None), None);
        assert_eq!(
            normalize_config_path(Some(BUILTIN_CONFIG_PATH.to_string())),
            Some(BUILTIN_CONFIG_PATH.to_string())
        );
    }
}
