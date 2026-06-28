use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use cruise::batch_run::run_all_with_dynamic_parallelism;
use cruise::new_session_draft::NewSessionDraft;
use cruise::new_session_history::{
    BUILTIN_CONFIG_KEY, NewSessionHistory, NewSessionHistoryEntry, expand_tilde,
    resolved_config_key_for_session,
};
use cruise::paths;
use cruise::session::{
    PLAN_VAR, SessionLogger, SessionManager, SessionPhase, SessionState, WorkspaceMode,
    current_iso8601,
};
use cruise::session_edit::CurrentStepUpdate;
use cruise::step::option::OptionResult;
use cruise::workspace::{prepare_execution_workspace, update_session_workspace};
use serde::{Deserialize, Serialize};

use cruise::planning::{
    ask_plan_template, fix_plan_template, initial_plan_template, plan_template, setup_plan_vars,
};

use crate::events::{PlanEvent, WorkflowEvent};
use crate::gui_option_handler::GuiOptionHandler;
use crate::state::{AppState, AskResponder};

const DEFAULT_RATE_LIMIT_RETRIES: usize = 5;

// --- DTOs ---------------------------------------------------------------------

/// Serializable representation of a session, sent to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub id: String,
    pub phase: String,
    /// Error message when `phase == "Failed"`.
    pub phase_error: Option<String>,
    pub config_source: String,
    pub config_path: Option<String>,
    pub base_dir: String,
    /// GitHub repository (`owner/repo`) backing this session, if any.
    pub repo: Option<String>,
    pub input: String,
    pub title: Option<String>,
    pub current_step: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub worktree_branch: Option<String>,
    pub pr_url: Option<String>,
    pub updated_at: Option<String>,
    pub awaiting_input: bool,
    pub workspace_mode: WorkspaceMode,
    /// Whether a valid (non-empty) `plan.md` exists for this session.
    pub plan_available: bool,
    /// Persisted planning ask_user question while phase == "Awaiting Input".
    pub pending_ask_question: Option<String>,
    /// True while a plan-fix request is in progress.
    pub fix_in_progress: bool,
    pub skipped_steps: Vec<String>,
}

impl SessionDto {
    /// Construct a [`SessionDto`] with filesystem-derived `plan_available` flag.
    ///
    /// Use this instead of `From<SessionState>` whenever you have access to a
    /// [`SessionManager`] so that `plan_available` is correctly populated.
    pub(crate) fn from_state(
        session: cruise::session::SessionState,
        manager: &SessionManager,
    ) -> Self {
        let plan_path = session.plan_path(&manager.sessions_dir());
        let plan_available = cruise::metadata::plan_markdown_available(&plan_path);
        let mut dto = Self::from(session);
        dto.plan_available = plan_available;
        dto
    }
}

impl From<cruise::session::SessionState> for SessionDto {
    fn from(s: cruise::session::SessionState) -> Self {
        let (phase_label, phase_error) = match &s.phase {
            SessionPhase::Failed(e) => ("Failed".to_string(), Some(e.clone())),
            other => (other.label().to_string(), None),
        };
        Self {
            id: s.id,
            phase: phase_label,
            phase_error,
            config_source: s.config_source,
            config_path: s.config_path.map(|p| p.to_string_lossy().into_owned()),
            base_dir: s.base_dir.to_string_lossy().into_owned(),
            repo: s.repo,
            input: s.input,
            title: s.title,
            current_step: s.current_step,
            created_at: s.created_at,
            completed_at: s.completed_at,
            worktree_branch: s.worktree_branch,
            pr_url: s.pr_url,
            updated_at: s.updated_at,
            awaiting_input: s.awaiting_input,
            workspace_mode: s.workspace_mode,
            plan_available: false,
            pending_ask_question: s.pending_ask_question,
            fix_in_progress: false, // populated from AppState in list_sessions / get_session
            skipped_steps: s.skipped_steps,
        }
    }
}

/// A directory entry returned by `list_directory`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryDto {
    pub name: String,
    pub path: String,
}

/// Result of a cleanup operation, returned to the frontend.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupResultDto {
    pub deleted: usize,
    pub skipped: usize,
}

/// Serializable DTO for update readiness, returned by [`get_update_readiness`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReadinessDto {
    pub can_auto_update: bool,
    /// `"translocated"` | `"mountedVolume"` | `"unknownBundlePath"` -- set when `can_auto_update` is false.
    pub reason: Option<String>,
    /// The resolved `.app` bundle path, for display in the UI.
    pub bundle_path: Option<String>,
    /// Human-readable remediation guidance.
    pub guidance: Option<String>,
}

/// Option result sent by the frontend when responding to an [`WorkflowEvent::OptionRequired`].
///
/// Mirrors [`OptionResult`] but derives [`Deserialize`] for IPC deserialization.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionResultDto {
    pub next_step: Option<String>,
    pub text_input: Option<String>,
}

// --- StateSavingEmitter --------------------------------------------------------

/// Wraps the Tauri IPC channel and intercepts `OptionRequired` events to update
/// the session's `awaiting_input` field in `state.json`.
struct StateSavingEmitter {
    inner: tauri::ipc::Channel<WorkflowEvent>,
    session_id: String,
}

impl StateSavingEmitter {
    fn new(inner: tauri::ipc::Channel<WorkflowEvent>, session_id: String) -> Self {
        Self { inner, session_id }
    }
}

impl crate::gui_option_handler::EventEmitter for StateSavingEmitter {
    fn emit(&self, event: WorkflowEvent) {
        if matches!(&event, WorkflowEvent::OptionRequired { .. }) {
            if let Ok(manager) = new_session_manager() {
                if let Ok(mut state) = manager.load(&self.session_id) {
                    state.awaiting_input = true;
                    let _ = manager.save(&state);
                }
            }
        }
        let _ = self.inner.send(event);
    }
}

// --- Helpers -------------------------------------------------------------------

/// RAII guard that clears the in-flight fix flag when dropped.
///
/// Ensures `stop_fixing` is called on every exit path of [`fix_session`]
/// without requiring a manual call at each return site.
struct FixingGuard<'a> {
    state: &'a AppState,
    session_id: String,
}

impl<'a> FixingGuard<'a> {
    fn new(state: &'a AppState, session_id: String) -> Self {
        state.start_fixing(&session_id);
        Self { state, session_id }
    }

    fn try_new(state: &'a AppState, session_id: String) -> Option<Self> {
        if state.try_start_fixing(&session_id) {
            Some(Self { state, session_id })
        } else {
            None
        }
    }
}

impl Drop for FixingGuard<'_> {
    fn drop(&mut self) {
        self.state.stop_fixing(&self.session_id);
    }
}

fn new_session_manager() -> std::result::Result<SessionManager, String> {
    let data_dir = paths::data_dir().map_err(|e| e.to_string())?;
    Ok(SessionManager::new(data_dir))
}

/// Resolve the base directory and workflow config for a new GUI session.
///
/// For repo-backed sessions (`repo = Some("owner/repo")`) the repository is
/// cloned into `<data_dir>/clones/{session_id}/`, which becomes the base
/// directory; the clone is removed again if config resolution fails.
fn resolve_new_session_workspace(
    manager: &SessionManager,
    session_id: &str,
    repo: Option<&str>,
    base_dir: &str,
    config_path: Option<&str>,
) -> std::result::Result<(PathBuf, String, cruise::resolver::ConfigSource), String> {
    let Some(spec) = repo else {
        return resolve_gui_session_paths(base_dir, config_path);
    };
    cruise::repo_clone::validate_repo_spec(spec).map_err(|e| e.to_string())?;
    cruise::worktree_pr::ensure_gh_available().map_err(|e| e.to_string())?;
    let clone_path = manager.clones_dir().join(session_id);
    cruise::repo_clone::clone_repo(spec, &clone_path).map_err(|e| e.to_string())?;
    match cruise::resolver::resolve_config_in_dir(config_path, &clone_path) {
        Ok((yaml, source)) => Ok((clone_path, yaml, source)),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&clone_path);
            Err(e.to_string())
        }
    }
}

/// Remove the temporary clone for `session_id` if one exists (best-effort).
fn remove_session_clone(manager: &SessionManager, session_id: &str) {
    let clone_path = manager.clones_dir().join(session_id);
    if clone_path.exists() {
        let _ = std::fs::remove_dir_all(&clone_path);
    }
}

fn persist_plan_failure(
    manager: &SessionManager,
    session: &mut SessionState,
    error: String,
) -> std::result::Result<(), String> {
    session.plan_error = Some(error);
    session.pending_ask_question = None;
    if matches!(session.phase, SessionPhase::AwaitingInput) {
        session.phase = SessionPhase::Draft;
    }
    manager.save(session).map_err(|e| e.to_string())
}

fn prepare_run_session(
    manager: &SessionManager,
    session: &mut SessionState,
    requested_workspace_mode: WorkspaceMode,
) -> cruise::error::Result<PathBuf> {
    let effective_workspace_mode = if session.current_step.is_none() {
        requested_workspace_mode
    } else {
        session.workspace_mode
    };

    // Preflight: verify `gh` is available before doing any workspace setup so
    // the session is never left in Running phase when `gh` is absent.
    if effective_workspace_mode == WorkspaceMode::Worktree {
        cruise::worktree_pr::ensure_gh_available()?;
    }

    session.workspace_mode = effective_workspace_mode;
    let execution_workspace =
        prepare_execution_workspace(manager, session, effective_workspace_mode)?;
    update_session_workspace(session, &execution_workspace);
    session.set_runner_to_current_process();
    session.phase = SessionPhase::Running;
    manager.save(session)?;

    Ok(execution_workspace.path().to_path_buf())
}

// --- Filesystem commands -------------------------------------------------------

/// List subdirectories of `path`, returning up to 50 entries sorted alphabetically.
///
/// `~` is expanded to `$HOME`. Hidden directories (`.`-prefixed) are excluded.
/// Non-existent paths return an empty Vec rather than an error.
#[tauri::command]
pub fn list_directory(path: String) -> std::result::Result<Vec<DirEntryDto>, String> {
    let expanded = expand_tilde(&path);

    let dir = std::path::Path::new(&expanded);
    if !dir.exists() {
        return Ok(vec![]);
    }

    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Ok(vec![]);
    };

    let mut entries: Vec<DirEntryDto> = read_dir
        .flatten()
        .filter(|e| {
            let ft = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if !ft {
                return false;
            }
            let name = e.file_name();
            let name_str = name.to_string_lossy();
            !name_str.starts_with('.')
        })
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let full_path = e.path().to_string_lossy().into_owned();
            DirEntryDto {
                name,
                path: full_path,
            }
        })
        .collect();

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries.truncate(50);
    Ok(entries)
}

/// List GitHub repositories (`owner/repo`) visible to the authenticated `gh`
/// user, most recently pushed first.
///
/// Used by the New Session form's repository picker. Returns an error if `gh`
/// is unavailable or not authenticated; the picker still accepts free-form
/// `owner/repo` input in that case.
#[tauri::command]
pub fn list_github_repos() -> std::result::Result<Vec<String>, String> {
    cruise::worktree_pr::ensure_gh_available().map_err(|e| e.to_string())?;
    let output = std::process::Command::new("gh")
        .args([
            "repo",
            "list",
            "--limit",
            "200",
            "--json",
            "nameWithOwner",
            "--jq",
            ".[].nameWithOwner",
        ])
        .output()
        .map_err(|e| format!("failed to run gh repo list: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "gh repo list failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToString::to_string)
        .collect())
}

// --- Read commands -------------------------------------------------------------

/// List all sessions, sorted oldest-first (internal implementation).
pub fn list_sessions_impl(manager: &SessionManager, state: &AppState) -> Vec<SessionDto> {
    let fixing = state.snapshot_fixing();
    manager
        .list()
        .map(|sessions| {
            sessions
                .into_iter()
                .map(|mut s| {
                    if matches!(s.phase, SessionPhase::Running) {
                        let active = state.is_session_active(&s.id);
                        let _ = manager.reconcile_running_phase(&mut s, active);
                    }
                    let mut dto = SessionDto::from_state(s, manager);
                    dto.fix_in_progress = fixing.contains(&dto.id);
                    dto
                })
                .collect()
        })
        .unwrap_or_else(|e| {
            eprintln!("warning: failed to list sessions: {e}");
            Vec::new()
        })
}

/// List all sessions, sorted oldest-first.
#[tauri::command]
pub fn list_sessions(
    state: tauri::State<'_, AppState>,
) -> std::result::Result<Vec<SessionDto>, String> {
    let manager = new_session_manager()?;
    Ok(list_sessions_impl(&manager, &state))
}

/// Get a single session by ID.
#[tauri::command]
pub fn get_session(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<SessionDto, String> {
    let manager = new_session_manager()?;
    manager
        .load(&session_id)
        .map(|mut s| {
            if matches!(s.phase, SessionPhase::Running) {
                let active = state.is_session_active(&s.id);
                let _ = manager.reconcile_running_phase(&mut s, active);
            }
            let mut dto = SessionDto::from_state(s, &manager);
            dto.fix_in_progress = state.is_fixing(&dto.id);
            dto
        })
        .map_err(|e| e.to_string())
}

/// Return the plan markdown for a session.
#[tauri::command]
pub fn get_session_plan(session_id: String) -> std::result::Result<String, String> {
    let manager = new_session_manager()?;
    let session = manager.load(&session_id).map_err(|e| e.to_string())?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    std::fs::read_to_string(&plan_path)
        .map_err(|e| format!("failed to read plan {}: {}", plan_path.display(), e))
}

// --- Write commands ------------------------------------------------------------

/// Cancel the currently running workflow session.
#[tauri::command]
pub fn cancel_session(state: tauri::State<'_, AppState>) -> std::result::Result<(), String> {
    // TODO: accept a session_id parameter so the caller can cancel a specific session.
    state.cancel_all_sessions();
    Ok(())
}

/// Deliver the frontend's option-step response to the engine.
#[tauri::command]
pub fn respond_to_option(
    result: OptionResultDto,
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    let option_result = OptionResult {
        next_step: result.next_step,
        text_input: result.text_input,
    };
    let sent = state.respond_to_option(&session_id, option_result);
    if sent {
        // Best-effort: clear awaiting_input so the UI reflects the response immediately.
        // The engine will also clear this when the session finishes.
        if let Ok(manager) = new_session_manager() {
            if let Ok(mut s) = manager.load(&session_id) {
                s.awaiting_input = false;
                let _ = manager.save(&s);
            }
        }
        Ok(())
    } else {
        Err(format!("no pending option for session {session_id}"))
    }
}

/// Deliver the frontend's answer to an SDK `ask_user` dialog to the planning agent.
#[tauri::command]
pub fn respond_to_ask(
    answer: String,
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    respond_to_ask_impl(&state, &session_id, answer)
}

/// Testable core of `respond_to_ask`.
///
/// Ordering: take sender → send answer.
///
/// The sender is taken first so that a concurrent `respond_to_ask` call for the
/// same session cannot steal it.  Persisted ask state (`pending_ask_question`,
/// phase) is cleared by `GuiAskHandler::ask_user` once the blocking `recv()`
/// returns, keeping the entire load-modify-save on the agent thread and
/// eliminating the lost-update race that would otherwise occur if this function
/// and the agent thread both tried to write session state concurrently.
///
/// If `send` fails (the agent thread has already died) the persisted
/// `pending_ask_question` is left intact — the user will see the question again
/// on restart rather than entering a permanently-stuck state.
pub(crate) fn respond_to_ask_impl(
    state: &AppState,
    session_id: &str,
    answer: String,
) -> std::result::Result<(), String> {
    let sender = state
        .take_ask_sender(session_id)
        .ok_or_else(|| format!("no pending ask_user for session {session_id}"))?;

    sender
        .send(answer)
        .map_err(|_| format!("ask_user answer channel closed for session {session_id}"))?;

    Ok(())
}

/// Remove Completed sessions whose PR is closed or merged.
#[tauri::command]
pub async fn clean_sessions() -> std::result::Result<CleanupResultDto, String> {
    let manager = new_session_manager()?;
    tokio::task::spawn_blocking(move || {
        manager
            .cleanup_by_pr_status()
            .map(|r| CleanupResultDto {
                deleted: r.deleted,
                skipped: r.skipped,
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("cleanup task panicked: {e}"))?
}

/// Return the run log for a session as a plain-text string.
///
/// Returns an empty string when no log file exists yet (session never run).
#[tauri::command]
pub fn get_session_log(session_id: String) -> std::result::Result<String, String> {
    let manager = new_session_manager()?;
    let log_path = manager.run_log_path(&session_id);
    match std::fs::read_to_string(&log_path) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("failed to read log {}: {}", log_path.display(), e)),
    }
}

// --- Plan generation helpers ---------------------------------------------------

/// Create a logger for GUI planning output and write the operation start boundary.
fn plan_logger(manager: &SessionManager, session_id: &str, operation: &str) -> Arc<SessionLogger> {
    let logger = Arc::new(SessionLogger::new(manager.run_log_path(session_id)));
    logger.write(&format!("--- {operation} started ---"));
    logger
}

/// Persist one streamed planning output line to the session log.
fn log_plan_chunk(logger: &SessionLogger, line: &str) {
    logger.write(line);
}

/// Write a concise successful terminal marker for a planning operation.
fn log_plan_success(logger: &SessionLogger, message: &str) {
    logger.write(&format!("[OK] {message}"));
}

/// Write a concise failed terminal marker for a planning operation.
fn log_plan_failure(logger: &SessionLogger, message: &str) {
    logger.write(&format!("[FAIL] {message}"));
}

/// Build stdout/stderr callbacks that stream LLM output lines as [`PlanEvent::PlanChunk`]
/// events over the given IPC channel.
fn plan_chunk_callbacks(
    session_id: String,
    channel: tauri::ipc::Channel<PlanEvent>,
    logger: Option<Arc<SessionLogger>>,
) -> (
    Box<dyn Fn(&str) + Send + Sync>,
    Box<dyn Fn(&str) + Send + Sync>,
) {
    let make_callback = move |stream: &'static str| {
        let ch = channel.clone();
        let sid = session_id.clone();
        let log = logger.clone();
        Box::new(move |line: &str| {
            if let Some(logger) = log.as_deref() {
                log_plan_chunk(logger, line);
            }
            let _ = ch.send(PlanEvent::PlanChunk {
                session_id: sid.clone(),
                stream: stream.to_string(),
                line: line.to_string(),
            });
        }) as Box<dyn Fn(&str) + Send + Sync>
    };
    (make_callback("stdout"), make_callback("stderr"))
}

/// Guard that registers an SDK `ask_user` responder slot for a plan command and
/// removes it on drop, building the [`PlanPromptCtx`] the command runs with.
struct GuiPlanCtx {
    state: AppState,
    session_id: String,
    /// The exact responder slot this guard registered, so cleanup only removes
    /// its own slot (not a newer one a concurrent command may have installed).
    responder: AskResponder,
}

impl GuiPlanCtx {
    /// Register the ask responder and build a planning context whose `ask_user`
    /// tool routes questions to the frontend over `channel`.
    ///
    /// Note: GUI plan/fix/regenerate are independent one-shot commands with no
    /// in-process approve loop, so each passes `resume: &mut None` and starts a
    /// fresh seher session. Multi-turn `resume` (sharing one conversation across
    /// turns) is a CLI-only affordance; persisting the session id across GUI
    /// commands would require additional `AppState` plumbing.
    fn build<'a>(
        state: &AppState,
        manager: &SessionManager,
        session_id: &str,
        channel: &tauri::ipc::Channel<PlanEvent>,
        config: &'a cruise::config::WorkflowConfig,
        plan_path: &'a std::path::Path,
        working_dir: &'a std::path::Path,
        grill: bool,
    ) -> (Self, cruise::planning::PlanPromptCtx<'a>) {
        let responder = state.register_ask_responder(session_id);
        let ask: Arc<dyn cruise::ask_handler::AskHandler> =
            Arc::new(crate::gui_ask_handler::GuiAskHandler::new(
                channel.clone(),
                session_id.to_string(),
                manager.clone(),
                Arc::clone(&responder),
            ));
        let ctx = cruise::planning::PlanPromptCtx {
            config,
            ask,
            plan_path,
            interactive: true,
            rate_limit_retries: 5,
            working_dir: Some(working_dir),
            // Grill is opt-in per command: the initial-plan command (`create_session`)
            // may set it; regenerate/fix always pass `false`.
            grill,
            cancel_token: None,
        };
        (
            Self {
                state: state.clone(),
                session_id: session_id.to_string(),
                responder,
            },
            ctx,
        )
    }
}

impl Drop for GuiPlanCtx {
    fn drop(&mut self) {
        self.state
            .unregister_ask_responder(&self.session_id, &self.responder);
    }
}
// --- Session creation commands -------------------------------------------------

/// A discovered workflow config file, returned to the frontend.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEntryDto {
    pub path: String,
    pub name: String,
    pub description: Option<String>,
}

/// Summary of the latest GUI "New Session" selections.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionHistorySummaryDto {
    pub last_requested_config_path: Option<String>,
    pub last_working_dir: Option<String>,
    pub recent_working_dirs: Vec<String>,
}

/// Step list and default skip-step selections for the GUI form.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionConfigDefaultsDto {
    pub steps: Vec<cruise::workflow::SkippableStepNode>,
    pub after_pr_steps: Vec<cruise::workflow::SkippableStepNode>,
    pub default_skipped_steps: Vec<String>,
}

/// Serializable draft of the New Session form, sent over IPC.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionDraftDto {
    pub input: String,
    pub config_path: Option<String>,
    pub base_dir: String,
    #[serde(default)]
    pub repo: Option<String>,
    pub skipped_steps: Vec<String>,
    pub updated_at: Option<String>,
}

/// Enumerate `*.yaml` / `*.yml` files in `dir`, parse each for `description`, and return
/// sorted `ConfigEntryDto` entries. Files that fail to parse yield `description: None`.
fn read_configs_in(dir: &std::path::Path) -> Vec<ConfigEntryDto> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut configs: Vec<ConfigEntryDto> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file() && matches!(p.extension().and_then(|e| e.to_str()), Some("yaml" | "yml"))
        })
        .map(|p| {
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let description = std::fs::read_to_string(&p)
                .ok()
                .and_then(|yaml| cruise::yaml_metadata::extract_one_line_description(&yaml));
            ConfigEntryDto {
                name,
                path: p.to_string_lossy().into_owned(),
                description,
            }
        })
        .collect();
    configs.sort_by(|a, b| a.name.cmp(&b.name));
    configs
}

/// Return local config candidates from `base_dir` in resolver priority order:
/// 1. `cruise.yaml` / `cruise.yml` / `.cruise.yaml` / `.cruise.yml` at root (in that order).
/// 2. All YAML files inside `.cruise/` (ASCII-sorted).
///
/// Files that do not exist are silently skipped.
fn collect_local_configs(base_dir: &std::path::Path) -> Vec<ConfigEntryDto> {
    let mut configs = Vec::new();

    // 1. Root-level priority files in resolver order.
    for name in &["cruise.yaml", "cruise.yml", ".cruise.yaml", ".cruise.yml"] {
        let path = base_dir.join(name);
        if path.is_file() {
            let description = std::fs::read_to_string(&path)
                .ok()
                .and_then(|yaml| cruise::yaml_metadata::extract_one_line_description(&yaml));
            configs.push(ConfigEntryDto {
                name: (*name).to_string(),
                path: path.to_string_lossy().into_owned(),
                description,
            });
        }
    }

    // 2. Files inside `.cruise/` (ASCII-sorted via read_configs_in).
    let cruise_dir = base_dir.join(".cruise");
    if cruise_dir.is_dir() {
        configs.extend(read_configs_in(&cruise_dir));
    }

    configs
}

/// Collect configs for the GUI from all sources in priority order:
/// local (from `base_dir`) → user-dir.
///
/// When `is_repo_mode` is true or `base_dir` is `None`, only user-dir configs are returned.
/// Duplicate absolute paths (e.g., symlinked files) are removed, keeping the first occurrence.
fn collect_configs_for_gui(
    base_dir: Option<&std::path::Path>,
    is_repo_mode: bool,
    user_dir: &std::path::Path,
) -> Vec<ConfigEntryDto> {
    let local_entries: Vec<ConfigEntryDto> = if !is_repo_mode {
        base_dir.map(collect_local_configs).unwrap_or_default()
    } else {
        vec![]
    };
    let user_entries = read_configs_in(user_dir);

    let mut configs = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    for entry in local_entries.into_iter().chain(user_entries) {
        let canonical = std::path::Path::new(&entry.path)
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(&entry.path));
        if seen.insert(canonical) {
            configs.push(entry);
        }
    }

    configs
}

/// List available workflow config files.
///
/// When `base_dir` is provided and `repo` is `None`, local configs from the working directory
/// are included first (in resolver priority order), followed by user-dir configs.
/// When `base_dir` is absent, only user-dir configs are returned.
///
/// Note: `repo` is reserved for future repo-mode filtering but the current frontend always
/// passes `null`; `is_repo_mode` is therefore always `false` in practice.
#[tauri::command]
pub fn list_configs(
    base_dir: Option<String>,
    repo: Option<String>,
) -> std::result::Result<Vec<ConfigEntryDto>, String> {
    let user_config_dir = cruise::paths::config_dir().map_err(|e| e.to_string())?;
    let is_repo_mode = repo.is_some();

    let normalized_base_dir = base_dir
        .as_deref()
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(expand_tilde(d)));

    Ok(collect_configs_for_gui(
        normalized_base_dir.as_deref(),
        is_repo_mode,
        &user_config_dir,
    ))
}

/// Create a new session and generate a plan, streaming [`PlanEvent`]s over `channel`.
///
/// Returns the new session ID on success.  The session is left in "Awaiting Approval"
/// phase so the frontend can show the plan and let the user approve or discard it.
#[tauri::command]
pub async fn create_session(
    input: String,
    config_path: Option<String>,
    base_dir: String,
    repo: Option<String>,
    skipped_steps: Vec<String>,
    use_input_as_plan: bool,
    grill: bool,
    no_interactive_planning: bool,
    image_attachments: Vec<String>,
    channel: tauri::ipc::Channel<PlanEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<String, String> {
    use cruise::config::{WorkflowConfig, validate_config};
    use cruise::session::{SessionManager, SessionState};

    let repo = repo.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    let manager = new_session_manager()?;
    let session_id = SessionManager::new_session_id();
    let (base, yaml, source) = resolve_new_session_workspace(
        &manager,
        &session_id,
        repo.as_deref(),
        &base_dir,
        config_path.as_deref(),
    )?;
    let mut config = match WorkflowConfig::from_yaml(&yaml) {
        Ok(config) => config,
        Err(e) => {
            remove_session_clone(&manager, &session_id);
            return Err(format!("config parse error: {e}"));
        }
    };
    if let Err(e) = validate_config(&config) {
        remove_session_clone(&manager, &session_id);
        return Err(e.to_string());
    }

    if no_interactive_planning {
        config.interactive_planning = false;
    }

    // Grill mode interviews the user via the SDK `ask_user` tool, which is only
    // registered in the interactive tool-based planning flow. Reject before
    // creating the session when the SDK backend is absent or interactive
    // planning is disabled.
    if grill && !cruise::planning::sdk_plan_tools_enabled(&config) {
        remove_session_clone(&manager, &session_id);
        return Err(
            "grill mode requires the SDK backend with interactive planning enabled \
             (`sdk:` must be set and `interactive_planning` must not be disabled)"
                .to_string(),
        );
    }

    let mut session = SessionState::new(
        session_id.clone(),
        base.clone(),
        source.display_string(),
        input.trim().to_string(),
    );
    session.repo = repo.clone();
    // Configs that live inside the temporary clone are copied into the session
    // directory below so they stay readable after the clone is removed.
    session.config_path = if repo.is_some() {
        cruise::repo_clone::persistent_config_path(&source, &base)
    } else {
        source.path().cloned()
    };
    session.skipped_steps = skipped_steps;
    if !use_input_as_plan {
        session.phase = SessionPhase::Draft;
    }
    if let Err(e) = manager.create(&session) {
        remove_session_clone(&manager, &session_id);
        return Err(e.to_string());
    }

    let mut history = NewSessionHistory::load_best_effort();
    history.record_selection(NewSessionHistoryEntry {
        selected_at: current_iso8601(),
        input: session.input.clone(),
        requested_config_path: config_path,
        // Clone paths are temporary; never offer them as recent directories.
        working_dir: if repo.is_some() {
            String::new()
        } else {
            base.to_string_lossy().into_owned()
        },
        repo: repo.clone(),
        resolved_config_key: session.config_path.as_deref().map_or_else(
            || BUILTIN_CONFIG_KEY.to_string(),
            resolved_config_key_for_session,
        ),
        skipped_steps: session.skipped_steps.clone(),
    });
    history.save_best_effort();

    let session_dir = manager.sessions_dir().join(&session_id);
    if session.config_path.is_none() {
        std::fs::write(session_dir.join("config.yaml"), &yaml)
            .map_err(|e| format!("failed to write session config: {e}"))?;
    }

    // Copy attached images into the session dir and rewrite session.input so the
    // planning prompt references them (the agent reads images via its Read tool).
    if !image_attachments.is_empty() {
        let sources: Vec<PathBuf> = image_attachments.iter().map(PathBuf::from).collect();
        match cruise::attachments::copy_images_into_session(&session_dir, &sources) {
            Ok(stored) => {
                session.attachments = stored;
                if let Err(e) = manager.save(&session) {
                    let _ = manager.delete(&session_id);
                    remove_session_clone(&manager, &session_id);
                    return Err(e.to_string());
                }
            }
            Err(e) => {
                let _ = manager.delete(&session_id);
                remove_session_clone(&manager, &session_id);
                return Err(e.to_string());
            }
        }
    }

    let plan_logger = plan_logger(
        &manager,
        &session_id,
        if use_input_as_plan {
            "planning (input as plan)"
        } else {
            "planning"
        },
    );

    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = setup_plan_vars(session.input_with_attachments(), plan_path.clone(), &config);

    if use_input_as_plan {
        // Emit SessionCreated only after successful write so that form error handling
        // still works if the write fails (formReleased stays false in the frontend).
        // input_with_attachments() so the plan content references images that
        // executed steps can read.
        let plan_content = session.input_with_attachments();
        match cruise::planning::write_input_as_plan(&plan_path, &plan_content) {
            Ok(content) => {
                cruise::metadata::refresh_session_title_from_plan(&mut session, &content);
                session.plan_error = None;
                // Transition directly to Planned (same as CLI --skip-planning).
                session.approve();
                cruise::repo_clone::cleanup_after_approval(&manager, &mut session);
                if let Err(e) = manager.save(&session) {
                    log_plan_failure(&plan_logger, &format!("planning failed: {e}"));
                    let _ = manager.delete(&session_id);
                    remove_session_clone(&manager, &session_id);
                    return Err(e.to_string());
                }
                log_plan_success(&plan_logger, "input saved as plan");
                let _ = channel.send(PlanEvent::SessionCreated {
                    session_id: session_id.clone(),
                });
                let _ = channel.send(PlanEvent::PlanGenerating);
                let _ = channel.send(PlanEvent::PlanGenerated {
                    session_id: session_id.clone(),
                    content,
                });
                return Ok(session_id);
            }
            Err(e) => {
                log_plan_failure(&plan_logger, &format!("planning failed: {e}"));
                let _ = manager.delete(&session_id);
                remove_session_clone(&manager, &session_id);
                return Err(e.to_string());
            }
        }
    }

    let _fixing_guard = FixingGuard::new(&state, session_id.clone());
    let _ = channel.send(PlanEvent::SessionCreated {
        session_id: session_id.clone(),
    });
    let _ = channel.send(PlanEvent::PlanGenerating);

    let (on_stdout, on_stderr) = plan_chunk_callbacks(
        session_id.clone(),
        channel.clone(),
        Some(plan_logger.clone()),
    );
    let stream_callbacks = cruise::step::prompt::StreamCallbacks {
        on_stdout: Some(on_stdout.as_ref()),
        on_stderr: Some(on_stderr.as_ref()),
    };

    let (_ask_guard, ctx) = GuiPlanCtx::build(
        &state,
        &manager,
        &session_id,
        &channel,
        &config,
        &plan_path,
        &base,
        grill,
    );
    match cruise::planning::run_plan_prompt_template(
        &ctx,
        &mut vars,
        initial_plan_template(&config, grill),
        "[plan] creating plan...",
        Some(&stream_callbacks),
        &mut None,
        true,
    )
    .await
    .map_err(|e| e.to_string())
    {
        Ok(result) => {
            let content = match cruise::metadata::resolve_plan_content(
                &plan_path,
                &result.output,
                &result.stderr,
            ) {
                Ok(c) => c,
                Err(e) => {
                    let msg = e.to_string();
                    log_plan_failure(&plan_logger, &format!("planning failed: {msg}"));
                    let _ = persist_plan_failure(&manager, &mut session, msg.clone());
                    let _ = channel.send(PlanEvent::PlanFailed {
                        session_id: session_id.clone(),
                        error: msg.clone(),
                    });
                    return Err(msg);
                }
            };
            log_plan_success(&plan_logger, "plan generated");
            session.phase = SessionPhase::AwaitingApproval;
            session.plan_error = None;
            session.pending_ask_question = None;
            if let Err(e) = manager.save(&session) {
                let msg = e.to_string();
                log_plan_failure(
                    &plan_logger,
                    &format!("phase transition failed after successful plan: {msg}"),
                );
                let _ = channel.send(PlanEvent::PlanFailed {
                    session_id: session_id.clone(),
                    error: msg.clone(),
                });
                return Err(msg);
            }
            drop(_fixing_guard);
            let _ = channel.send(PlanEvent::PlanGenerated {
                session_id: session_id.clone(),
                content: content.clone(),
            });
            Ok(session_id)
        }
        Err(msg) => {
            log_plan_failure(&plan_logger, &format!("planning failed: {msg}"));
            let _ = persist_plan_failure(&manager, &mut session, msg.clone());
            let _ = channel.send(PlanEvent::PlanFailed {
                session_id: session_id.clone(),
                error: msg.clone(),
            });
            Err(msg)
        }
    }
}

/// Create a new session in `Draft` phase without generating a plan
/// (internal implementation).
///
/// Mirrors the CLI `cruise draft` command: the session is persisted with no
/// `plan.md`, ready for the user to generate a plan later via
/// [`generate_plan_for_draft`]. Returns the new session ID.
///
/// Extracted for unit-testability: callers can supply any [`SessionManager`]
/// (including one backed by a `TempDir`).
pub(crate) fn create_draft_session_impl(
    manager: &SessionManager,
    input: String,
    config_path: Option<String>,
    base_dir: String,
    repo: Option<String>,
    skipped_steps: Vec<String>,
    image_attachments: Vec<String>,
) -> std::result::Result<String, String> {
    use cruise::config::{WorkflowConfig, validate_config};

    let repo = repo.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    let session_id = SessionManager::new_session_id();
    let (base, yaml, source) = resolve_new_session_workspace(
        manager,
        &session_id,
        repo.as_deref(),
        &base_dir,
        config_path.as_deref(),
    )?;
    let config = match WorkflowConfig::from_yaml(&yaml) {
        Ok(config) => config,
        Err(e) => {
            remove_session_clone(manager, &session_id);
            return Err(format!("config parse error: {e}"));
        }
    };
    if let Err(e) = validate_config(&config) {
        remove_session_clone(manager, &session_id);
        return Err(e.to_string());
    }

    let mut session = SessionState::new(
        session_id.clone(),
        base.clone(),
        source.display_string(),
        input.trim().to_string(),
    );
    session.repo = repo.clone();
    // Configs that live inside the temporary clone are copied into the session
    // directory below so they stay readable after the clone is removed.
    session.config_path = if repo.is_some() {
        cruise::repo_clone::persistent_config_path(&source, &base)
    } else {
        source.path().cloned()
    };
    session.skipped_steps = skipped_steps;
    session.phase = SessionPhase::Draft;
    if let Err(e) = manager.create(&session) {
        remove_session_clone(manager, &session_id);
        return Err(e.to_string());
    }

    let mut history = NewSessionHistory::load_best_effort();
    history.record_selection(NewSessionHistoryEntry {
        selected_at: current_iso8601(),
        input: session.input.clone(),
        requested_config_path: config_path,
        // Clone paths are temporary; never offer them as recent directories.
        working_dir: if repo.is_some() {
            String::new()
        } else {
            base.to_string_lossy().into_owned()
        },
        repo: repo.clone(),
        resolved_config_key: session.config_path.as_deref().map_or_else(
            || BUILTIN_CONFIG_KEY.to_string(),
            resolved_config_key_for_session,
        ),
        skipped_steps: session.skipped_steps.clone(),
    });
    history.save_best_effort();

    let session_dir = manager.sessions_dir().join(&session_id);
    if session.config_path.is_none()
        && let Err(e) = std::fs::write(session_dir.join("config.yaml"), &yaml)
    {
        let _ = manager.delete(&session_id);
        remove_session_clone(manager, &session_id);
        return Err(format!("failed to write session config: {e}"));
    }

    if !image_attachments.is_empty() {
        let sources: Vec<PathBuf> = image_attachments.iter().map(PathBuf::from).collect();
        match cruise::attachments::copy_images_into_session(&session_dir, &sources) {
            Ok(stored) => {
                session.attachments = stored;
                if let Err(e) = manager.save(&session) {
                    let _ = manager.delete(&session_id);
                    remove_session_clone(manager, &session_id);
                    return Err(e.to_string());
                }
            }
            Err(e) => {
                let _ = manager.delete(&session_id);
                remove_session_clone(manager, &session_id);
                return Err(e.to_string());
            }
        }
    }

    Ok(session_id)
}

/// Create a new session in `Draft` phase without generating a plan.
///
/// The session is left in `Draft` phase so the frontend can later generate a
/// plan via [`generate_plan_for_draft`]. Returns the new session ID.
#[tauri::command]
pub fn create_draft_session(
    input: String,
    config_path: Option<String>,
    base_dir: String,
    repo: Option<String>,
    skipped_steps: Vec<String>,
    image_attachments: Vec<String>,
) -> std::result::Result<String, String> {
    let manager = new_session_manager()?;
    create_draft_session_impl(
        &manager,
        input,
        config_path,
        base_dir,
        repo,
        skipped_steps,
        image_attachments,
    )
}

/// Return the latest persisted New Session selections and recent working directories.
#[tauri::command]
pub fn get_new_session_history_summary() -> std::result::Result<NewSessionHistorySummaryDto, String>
{
    let history = NewSessionHistory::load_best_effort();
    let mut seen = HashSet::new();
    let mut recent_working_dirs = Vec::new();
    let mut last_requested_config_path = None;
    let mut last_working_dir = None;
    for entry in &history.entries {
        if entry.working_dir.is_empty() {
            continue;
        }
        if cruise::new_session_history::is_temp_working_dir(&entry.working_dir) {
            continue;
        }
        if last_working_dir.is_none() {
            last_requested_config_path = entry.requested_config_path.clone();
            last_working_dir = Some(entry.working_dir.clone());
        }
        if seen.insert(entry.working_dir.clone()) && recent_working_dirs.len() < 5 {
            recent_working_dirs.push(entry.working_dir.clone());
        }
    }
    Ok(NewSessionHistorySummaryDto {
        last_requested_config_path,
        last_working_dir,
        recent_working_dirs,
    })
}

/// Resolve the effective config for the New Session form and return the skippable-step
/// tree together with history-backed default skip selections.
#[tauri::command]
pub fn get_new_session_config_defaults(
    base_dir: String,
    config_path: Option<String>,
    repo: Option<String>,
) -> std::result::Result<NewSessionConfigDefaultsDto, String> {
    let (_, yaml, source) = resolve_gui_session_paths(&base_dir, config_path.as_deref())?;
    let config = cruise::config::WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| format!("Failed to parse config: {e}"))?;
    cruise::config::validate_config(&config)
        .map_err(|e| format!("Failed to validate config: {e}"))?;
    let steps = cruise::workflow::list_skippable_steps(&config)
        .map_err(|e| format!("Failed to list skippable steps: {e}"))?;
    let after_pr_steps = cruise::workflow::list_skippable_after_pr_steps(&config)
        .map_err(|e| format!("Failed to list skippable after-pr steps: {e}"))?;
    let resolved_config_key = source.path().map_or_else(
        || BUILTIN_CONFIG_KEY.to_string(),
        |p| resolved_config_key_for_session(p),
    );
    let history = NewSessionHistory::load_best_effort();
    let scope = repo
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(cruise::new_session_history::HistoryScope::Repo)
        .or_else(|| {
            if base_dir.is_empty() {
                None
            } else {
                Some(cruise::new_session_history::HistoryScope::Directory(
                    &base_dir,
                ))
            }
        });
    let default_skipped_steps = scope
        .and_then(|s| history.latest_entry_for_scope(s, &resolved_config_key))
        .map(|entry| entry.skipped_steps.clone())
        .unwrap_or_default();
    Ok(NewSessionConfigDefaultsDto {
        steps,
        after_pr_steps,
        default_skipped_steps,
    })
}

/// Update session settings (config and/or skipped steps) for a session in
/// `Awaiting Approval` or `Planned` phase.
///
/// Returns the updated `SessionDto` on success.
pub fn update_session_settings(
    manager: &SessionManager,
    session_id: &str,
    config_path: Option<String>,
    skipped_steps: Vec<String>,
    current_step_update: CurrentStepUpdate,
) -> std::result::Result<SessionDto, String> {
    use cruise::session::SessionPhase;

    let mut session = manager.load(session_id).map_err(|e| e.to_string())?;

    let is_failed_or_suspended = matches!(
        &session.phase,
        SessionPhase::Failed(_) | SessionPhase::Suspended
    );

    match &session.phase {
        SessionPhase::Draft
        | SessionPhase::AwaitingApproval
        | SessionPhase::Planned
        | SessionPhase::Failed(_)
        | SessionPhase::Suspended => {}
        other => {
            return Err(format!(
                "Cannot edit session in '{}' phase. Only 'Draft', 'Awaiting Approval', 'Planned', 'Failed' and 'Suspended' sessions are editable.",
                other.label()
            ));
        }
    }

    let old_config_path = session
        .config_path
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());

    // Failed/Suspended: config path must stay the same
    if is_failed_or_suspended && old_config_path != config_path {
        return Err(
            "Cannot change config for a Failed or Suspended session. Only skip steps and current step can be edited.".to_string(),
        );
    }

    // current_step can only be set for Failed/Suspended
    if !matches!(current_step_update, CurrentStepUpdate::Unchanged) && !is_failed_or_suspended {
        return Err(
            "Cannot update current step for a session that is not Failed or Suspended.".to_string(),
        );
    }

    let (base, yaml, source) =
        resolve_gui_session_paths(&session.base_dir.to_string_lossy(), config_path.as_deref())?;
    let config = cruise::config::WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| format!("config parse error: {e}"))?;
    cruise::config::validate_config(&config).map_err(|e| e.to_string())?;

    match current_step_update {
        CurrentStepUpdate::Unchanged => {}
        CurrentStepUpdate::Clear => session.current_step = None,
        CurrentStepUpdate::Set(step_name) => {
            let nodes = cruise::workflow::list_skippable_steps(&config)
                .map_err(|e| format!("step expansion error: {e}"))?;
            let valid_ids: std::collections::HashSet<&str> = nodes
                .iter()
                .flat_map(|n| n.expanded_step_ids.iter().map(String::as_str))
                .collect();
            if !valid_ids.contains(step_name.as_str()) {
                return Err(format!(
                    "Step '{step_name}' does not exist in the workflow config."
                ));
            }
            if skipped_steps.contains(&step_name) {
                return Err(format!(
                    "Cannot set current_step to '{step_name}' because it is in skipped_steps."
                ));
            }
            session.current_step = Some(step_name);
        }
    }

    session.config_source = source.display_string();
    session.config_path = if config_path.is_some() {
        source.path().cloned()
    } else {
        None
    };
    session.skipped_steps = skipped_steps;
    session.plan_error = None;
    session.updated_at = Some(current_iso8601());

    manager.save(&session).map_err(|e| e.to_string())?;

    let session_dir = manager.sessions_dir().join(session_id);
    if session.config_path.is_none() {
        std::fs::write(session_dir.join("config.yaml"), &yaml)
            .map_err(|e| format!("failed to write session config: {e}"))?;
    }

    if !is_failed_or_suspended {
        let resolved_config_key = source.path().map_or_else(
            || BUILTIN_CONFIG_KEY.to_string(),
            |p| resolved_config_key_for_session(p),
        );
        let mut history = NewSessionHistory::load_best_effort();
        history.record_selection(NewSessionHistoryEntry {
            selected_at: current_iso8601(),
            input: session.input.clone(),
            requested_config_path: config_path,
            // Repo sessions use a temporary clone as base_dir; never expose it as a
            // recent directory (mirrors the create_session behaviour).
            working_dir: if session.repo.is_some() {
                String::new()
            } else {
                base.to_string_lossy().into_owned()
            },
            repo: session.repo.clone(),
            resolved_config_key,
            skipped_steps: session.skipped_steps.clone(),
        });
        history.save_best_effort();
    }

    Ok(SessionDto::from_state(session, manager))
}

/// Approve a session, transitioning it from "Awaiting Approval" to "Planned".
#[tauri::command]
pub fn approve_session(session_id: String) -> std::result::Result<(), String> {
    let manager = new_session_manager()?;
    let mut session = manager.load(&session_id).map_err(|e| e.to_string())?;
    if matches!(session.phase, SessionPhase::Draft) {
        return Err("Cannot approve a Draft session; generate a plan first.".to_string());
    }
    if let Err(err) = cruise::metadata::refresh_session_title_from_session(&manager, &mut session) {
        eprintln!("warning: failed to refresh session title: {err}");
    }
    session.approve();
    // Repo-backed sessions: the temporary clone is no longer needed once the
    // plan is approved; execution re-clones into a fresh directory.
    cruise::repo_clone::cleanup_after_approval(&manager, &mut session);
    manager.save(&session).map_err(|e| e.to_string())?;
    Ok(())
}

/// Reset a session to "Planned" phase regardless of its current phase.
#[tauri::command]
pub fn reset_session(session_id: String) -> std::result::Result<SessionDto, String> {
    let manager = new_session_manager()?;
    let mut session = manager.load(&session_id).map_err(|e| e.to_string())?;
    if matches!(session.phase, SessionPhase::Draft) {
        return Err("Cannot reset a Draft session; it has no plan to reset to.".to_string());
    }
    session.reset_to_planned();
    manager.save(&session).map_err(|e| e.to_string())?;
    Ok(SessionDto::from_state(session, &manager))
}

/// Update session settings (config, skipped steps, and/or current_step) for an editable session.
#[tauri::command]
pub fn update_session(
    session_id: String,
    config_path: Option<String>,
    skipped_steps: Vec<String>,
    current_step: Option<String>,
    set_current_step: bool,
) -> std::result::Result<SessionDto, String> {
    let manager = new_session_manager()?;
    let current_step_update = if !set_current_step {
        CurrentStepUpdate::Unchanged
    } else if let Some(step) = current_step {
        CurrentStepUpdate::Set(step)
    } else {
        CurrentStepUpdate::Clear
    };
    update_session_settings(
        &manager,
        &session_id,
        config_path,
        skipped_steps,
        current_step_update,
    )
}

/// Regenerate the plan for a session using its current config,
/// streaming [`PlanEvent`]s over `channel`.
#[tauri::command]
pub async fn regenerate_session_plan(
    session_id: String,
    channel: tauri::ipc::Channel<PlanEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<String, String> {
    let manager = new_session_manager()?;

    let _fixing_guard = FixingGuard::new(&state, session_id.clone());
    regenerate_plan(&manager, &session_id, &channel, &state).await
}

/// Generate the initial plan for a session in `Draft` phase,
/// streaming [`PlanEvent`]s over `channel`.
/// Transitions the session to `AwaitingApproval` on success.
#[tauri::command]
pub async fn generate_plan_for_draft(
    session_id: String,
    channel: tauri::ipc::Channel<PlanEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<String, String> {
    let manager = new_session_manager()?;
    // Claim the fixing slot before the phase check to close the TOCTOU window:
    // if two requests race, only one gets the guard; the other fails immediately.
    let _fixing_guard = FixingGuard::try_new(&state, session_id.clone())
        .ok_or_else(|| "plan generation already in progress for this session".to_string())?;
    {
        let session = manager.load(&session_id).map_err(|e| e.to_string())?;
        if !matches!(
            session.phase,
            SessionPhase::Draft | SessionPhase::AwaitingInput
        ) {
            return Err(format!(
                "expected Draft phase, got {}",
                session.phase.label()
            ));
        }
    }
    regenerate_plan(&manager, &session_id, &channel, &state).await
}

async fn regenerate_plan(
    manager: &SessionManager,
    session_id: &str,
    channel: &tauri::ipc::Channel<PlanEvent>,
    state: &AppState,
) -> std::result::Result<String, String> {
    let mut session = manager.load(session_id).map_err(|e| e.to_string())?;
    let operation = if matches!(session.phase, SessionPhase::Draft) {
        "planning"
    } else {
        "regenerate-plan"
    };
    let plan_logger = plan_logger(manager, session_id, operation);
    // Repo-backed sessions plan inside the temporary clone; re-create it if missing.
    match cruise::repo_clone::ensure_repo_session_workspace(manager, &mut session) {
        Ok(true) => {
            let _ = manager.save(&session);
        }
        Ok(false) => {}
        Err(e) => {
            let msg = e.to_string();
            log_plan_failure(&plan_logger, &format!("{operation} failed: {msg}"));
            return Err(msg);
        }
    }
    let config = match manager.load_config(&session) {
        Ok(config) => config,
        Err(e) => {
            let msg = e.to_string();
            log_plan_failure(&plan_logger, &format!("{operation} failed: {msg}"));
            return Err(msg);
        }
    };
    let _ = channel.send(PlanEvent::PlanGenerating);
    let plan_path = session.plan_path(&manager.sessions_dir());
    let base = session.base_dir.clone();
    let mut vars = setup_plan_vars(session.input_with_attachments(), plan_path.clone(), &config);

    let (on_stdout, on_stderr) = plan_chunk_callbacks(
        session_id.to_string(),
        channel.clone(),
        Some(plan_logger.clone()),
    );
    let stream_callbacks = cruise::step::prompt::StreamCallbacks {
        on_stdout: Some(on_stdout.as_ref()),
        on_stderr: Some(on_stderr.as_ref()),
    };

    let (_ask_guard, ctx) = GuiPlanCtx::build(
        state, manager, session_id, channel, &config, &plan_path, &base, false,
    );
    match cruise::planning::run_plan_prompt_template(
        &ctx,
        &mut vars,
        plan_template(&config),
        "[plan] regenerating plan...",
        Some(&stream_callbacks),
        &mut None,
        true,
    )
    .await
    .map_err(|e| e.to_string())
    {
        Ok(result) => {
            let content = match cruise::metadata::resolve_plan_content(
                &plan_path,
                &result.output,
                &result.stderr,
            ) {
                Ok(c) => c,
                Err(e) => {
                    let msg = e.to_string();
                    log_plan_failure(&plan_logger, &format!("planning failed: {msg}"));
                    let _ = channel.send(PlanEvent::PlanFailed {
                        session_id: session_id.to_string(),
                        error: msg.clone(),
                    });
                    let _ = persist_plan_failure(manager, &mut session, msg.clone());
                    return Err(msg);
                }
            };
            cruise::metadata::refresh_session_title_from_plan(&mut session, &content);
            session.plan_error = None;
            // Set AwaitingApproval before sending PlanGenerated so that any
            // immediate refreshSession() call in the UI sees the correct phase.
            if matches!(
                session.phase,
                SessionPhase::Draft | SessionPhase::AwaitingInput
            ) {
                session.phase = SessionPhase::AwaitingApproval;
            }
            session.pending_ask_question = None;
            if let Err(e) = manager.save(&session) {
                let msg = e.to_string();
                log_plan_failure(&plan_logger, &format!("{operation} failed: {msg}"));
                return Err(msg);
            }

            if matches!(operation, "planning") {
                log_plan_success(&plan_logger, "plan generated");
            } else {
                log_plan_success(&plan_logger, "plan regenerated");
            }
            let _ = channel.send(PlanEvent::PlanGenerated {
                session_id: session_id.to_string(),
                content: content.clone(),
            });
            Ok(content)
        }
        Err(msg) => {
            log_plan_failure(&plan_logger, &format!("planning failed: {msg}"));
            let _ = channel.send(PlanEvent::PlanFailed {
                session_id: session_id.to_string(),
                error: msg.clone(),
            });
            let _ = persist_plan_failure(manager, &mut session, msg.clone());
            Err(msg)
        }
    }
}

/// Delete a session that is still in "Awaiting Approval" phase (discard).
#[tauri::command]
pub fn discard_session(session_id: String) -> std::result::Result<(), String> {
    let manager = new_session_manager()?;
    if let Ok(session) = manager.load(&session_id)
        && session.repo.is_some()
    {
        cruise::repo_clone::cleanup_session_workspace(&manager, &session);
    }
    manager.delete(&session_id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a session and clean up its git worktree if one exists.
///
/// Running sessions cannot be deleted -- cancel them first.
#[tauri::command]
pub fn delete_session(session_id: String) -> std::result::Result<(), String> {
    let manager = new_session_manager()?;
    let session = manager.load(&session_id).map_err(|e| e.to_string())?;

    if matches!(session.phase, SessionPhase::Running) {
        return Err("Cannot delete a running session. Cancel it first.".to_string());
    }

    if session.repo.is_some() {
        cruise::repo_clone::cleanup_session_workspace(&manager, &session);
    } else if let Some(ctx) = session.worktree_context()
        && let Err(e) = cruise::worktree::cleanup_worktree(&ctx)
    {
        eprintln!(
            "warning: failed to remove worktree for {}: {}",
            session_id, e
        );
    }

    manager.delete(&session_id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Re-generate the plan for an existing session, streaming [`PlanEvent`]s over `channel`.
///
/// Returns the updated plan markdown on success.
#[tauri::command]
pub async fn fix_session(
    session_id: String,
    feedback: String,
    channel: tauri::ipc::Channel<PlanEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<String, String> {
    let manager = new_session_manager()?;
    let mut session = manager.load(&session_id).map_err(|e| e.to_string())?;
    let plan_logger = plan_logger(&manager, &session_id, "fix-plan");

    let _fixing_guard = FixingGuard::new(&state, session_id.clone());
    let _ = channel.send(PlanEvent::PlanGenerating);

    // Repo-backed sessions plan inside the temporary clone; re-create it if missing.
    match cruise::repo_clone::ensure_repo_session_workspace(&manager, &mut session) {
        Ok(true) => {
            let _ = manager.save(&session);
        }
        Ok(false) => {}
        Err(e) => {
            let msg = e.to_string();
            log_plan_failure(&plan_logger, &format!("fix-plan failed: {msg}"));
            return Err(msg);
        }
    }
    let config = match manager.load_config(&session) {
        Ok(config) => config,
        Err(e) => {
            let msg = e.to_string();
            log_plan_failure(&plan_logger, &format!("fix-plan failed: {msg}"));
            return Err(msg);
        }
    };
    let plan_path = session.plan_path(&manager.sessions_dir());
    let base = session.base_dir.clone();
    let mut vars = setup_plan_vars(session.input_with_attachments(), plan_path.clone(), &config);
    vars.set_prev_input(Some(feedback));

    let (on_stdout, on_stderr) = plan_chunk_callbacks(
        session_id.clone(),
        channel.clone(),
        Some(plan_logger.clone()),
    );
    let stream_callbacks = cruise::step::prompt::StreamCallbacks {
        on_stdout: Some(on_stdout.as_ref()),
        on_stderr: Some(on_stderr.as_ref()),
    };

    let (_ask_guard, ctx) = GuiPlanCtx::build(
        &state,
        &manager,
        &session_id,
        &channel,
        &config,
        &plan_path,
        &base,
        false,
    );
    match cruise::planning::run_plan_prompt_template(
        &ctx,
        &mut vars,
        fix_plan_template(&config),
        "[fix-plan] applying fixes...",
        Some(&stream_callbacks),
        &mut None,
        true,
    )
    .await
    .map_err(|e| e.to_string())
    {
        Ok(result) => {
            let content = match cruise::metadata::resolve_plan_content(
                &plan_path,
                &result.output,
                &result.stderr,
            ) {
                Ok(c) => c,
                Err(e) => {
                    let msg = e.to_string();
                    log_plan_failure(&plan_logger, &format!("fix-plan failed: {msg}"));
                    let _ = channel.send(PlanEvent::PlanFailed {
                        session_id: session_id.clone(),
                        error: msg.clone(),
                    });
                    return Err(msg);
                }
            };
            cruise::metadata::refresh_session_title_from_plan(&mut session, &content);
            // Re-save to update updated_at timestamp
            if let Err(e) = manager.save(&session) {
                let msg = e.to_string();
                log_plan_failure(&plan_logger, &format!("fix-plan failed: {msg}"));
                return Err(msg);
            }

            log_plan_success(&plan_logger, "plan fixed");
            let _ = channel.send(PlanEvent::PlanGenerated {
                session_id: session_id.clone(),
                content: content.clone(),
            });
            Ok(content)
        }
        Err(msg) => {
            log_plan_failure(&plan_logger, &format!("fix-plan failed: {msg}"));
            let _ = channel.send(PlanEvent::PlanFailed {
                session_id: session_id.clone(),
                error: msg.clone(),
            });
            Err(msg)
        }
    }
}

/// Ask a question about an existing session's plan without modifying it.
///
/// Extracted for unit-testability: callers can supply any `SessionManager`
/// (including one backed by a `TempDir`) and any config with a short-circuit
/// command (e.g. `["echo"]`) to exercise the logic without invoking the real LLM.
pub(crate) async fn do_ask_session(
    manager: &cruise::session::SessionManager,
    session_id: &str,
    question: String,
) -> std::result::Result<String, String> {
    let mut session = manager.load(session_id).map_err(|e| e.to_string())?;
    // Repo-backed sessions answer questions inside the temporary clone.
    if cruise::repo_clone::ensure_repo_session_workspace(manager, &mut session)
        .map_err(|e| e.to_string())?
    {
        let _ = manager.save(&session);
    }
    let config = manager.load_config(&session).map_err(|e| e.to_string())?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = setup_plan_vars(session.input_with_attachments(), plan_path.clone(), &config);
    vars.set_prev_input(Some(question));

    // The Ask flow is request/response (no streaming channel), so `ask_user`
    // cannot surface a question to the user: run non-interactively.
    let ask: Arc<dyn cruise::ask_handler::AskHandler> =
        Arc::new(cruise::ask_handler::NoninteractiveAskHandler);
    let ctx = cruise::planning::PlanPromptCtx {
        config: &config,
        ask,
        plan_path: &plan_path,
        interactive: false,
        rate_limit_retries: 5,
        working_dir: Some(&session.base_dir),
        // Grill mode is a CLI-only flag; the GUI uses the standard plan flow.
        grill: false,
        cancel_token: None,
    };
    let result = cruise::planning::run_plan_prompt_template(
        &ctx,
        &mut vars,
        ask_plan_template(&config),
        "[ask-plan] answering question...",
        None,
        &mut None,
        // Read-only: never register plan-writing tools for the Ask flow.
        false,
    )
    .await
    .map_err(|e| e.to_string())?;

    // Return raw output only -- do NOT call resolve_plan_content(), which would
    // overwrite plan.md with the answer and corrupt the saved plan.
    Ok(result.output)
}

/// Tauri command wrapper around [`do_ask_session`].
#[tauri::command]
pub async fn ask_session(
    session_id: String,
    question: String,
) -> std::result::Result<String, String> {
    let manager = new_session_manager()?;
    do_ask_session(&manager, &session_id, question).await
}

// --- run_session / run_all_sessions --------------------------------------------

/// Core session execution logic shared by [`run_session`] and [`run_all_sessions`].
///
/// Loads the session, runs the workflow on a dedicated blocking thread, saves the
/// final phase, and emits the terminal [`WorkflowEvent`].  Returns the final
/// [`SessionPhase`] so callers can decide how to proceed (e.g. break a batch loop
/// on `Suspended`).
///
/// Infrastructure errors (mutex poisoned, session not found, ...) are returned as
/// `Err(String)`.  Workflow-level errors (step failure) are returned as
/// `Ok(SessionPhase::Failed(msg))` so that `run_all_sessions` can log them and
/// continue to the next session instead of aborting the batch.
#[expect(clippy::too_many_lines)]
async fn execute_single_session(
    session_id: &str,
    workspace_mode: WorkspaceMode,
    channel: &tauri::ipc::Channel<WorkflowEvent>,
    state: &AppState,
    manager: &SessionManager,
    cancel_token: cruise::cancellation::CancellationToken,
) -> std::result::Result<SessionPhase, String> {
    let mut session = manager.load(session_id).map_err(|e| e.to_string())?;

    if !session.phase.is_runnable() {
        return Err(format!(
            "Session {} is in '{}' phase and cannot be run",
            session_id,
            session.phase.label()
        ));
    }

    // Repo-backed sessions always run in a worktree on a fresh clone so a PR
    // is always created; current-branch (no-PR) mode is not available.
    let workspace_mode = if session.repo.is_some() {
        WorkspaceMode::Worktree
    } else {
        workspace_mode
    };
    if cruise::repo_clone::ensure_repo_session_workspace(manager, &mut session)
        .map_err(|e| e.to_string())?
    {
        let _ = manager.save(&session);
    }

    let config = manager.load_config(&session).map_err(|e| e.to_string())?;
    let compiled = cruise::workflow::compile(config).map_err(|e| e.to_string())?;

    let mut dag = cruise::dag::build_dag(&compiled, 10).map_err(|e| e.to_string())?;
    let start_node = session.current_step.clone().map_or_else(
        || Ok(dag.start.clone()),
        |step| {
            if session.current_step_is_node_id {
                if dag.nodes.contains_key(&step) {
                    Ok(step)
                } else {
                    Ok(dag.start.clone())
                }
            } else {
                dag.first_node_for_step(&step)
                    .cloned()
                    .ok_or_else(|| format!("step '{step}' not found in workflow"))
            }
        },
    )?;

    let exec_root =
        prepare_run_session(&manager, &mut session, workspace_mode).map_err(|e| e.to_string())?;

    // Build WorktreeContext from session fields set by prepare_run_session, so the
    // blocking closure can call handle_worktree_pr without holding the session reference.
    let worktree_ctx_for_pr = if session.workspace_mode == WorkspaceMode::Worktree {
        session
            .worktree_path
            .as_ref()
            .zip(session.worktree_branch.as_ref())
            .map(|(path, branch)| cruise::worktree::WorktreeContext {
                path: path.clone(),
                branch: branch.clone(),
                original_dir: session.base_dir.clone(),
            })
    } else {
        None
    };
    let sid_for_pr = session_id.to_string();

    let option_responder =
        state.register_session_with_token(sid_for_pr.clone(), cancel_token.clone());
    let sessions_dir = manager.sessions_dir();
    let plan_path = session.plan_path(&sessions_dir);
    let input = session.input.clone();
    let skipped_steps = session.skipped_steps.clone();
    let token_for_task = cancel_token.clone();
    let channel_for_step = channel.clone();
    let channel_for_emitter = channel.clone();
    let channel_for_log = channel.clone();
    let sid_for_cleanup = sid_for_pr.clone();
    let log_path = manager.run_log_path(session_id);

    let join_result = tokio::task::spawn_blocking(
        move || -> cruise::error::Result<cruise::engine::ExecutionResult> {
            use cruise::dag::ExecutionDag;
            use cruise::engine::{ExecutionContext, NodeCheckpoint, execute_steps_with_dag};
            use cruise::file_tracker::FileTracker;
            use cruise::variable::VariableStore;

            let logger = std::sync::Arc::new(SessionLogger::new(log_path));
            logger.write("--- run started ---");

            // Temporarily change the working directory for command steps.
            let original_dir = std::env::current_dir().ok();
            std::env::set_current_dir(&exec_root).map_err(|e| {
                cruise::error::CruiseError::Other(format!("failed to set working dir: {e}"))
            })?;

            let logger_for_start = logger.clone();
            let on_step_start = |step: &str| -> cruise::error::Result<()> {
                logger_for_start.write(step);
                let _ = channel_for_step.send(WorkflowEvent::StepStarted {
                    session_id: sid_for_pr.clone(),
                    step: step.to_string(),
                });
                if let Ok(mgr) = new_session_manager() {
                    if let Ok(mut s) = mgr.load(&sid_for_pr) {
                        s.current_step = Some(step.to_string());
                        let _ = mgr.save(&s);
                    }
                }
                Ok(())
            };

            let logger_for_log = logger.clone();
            let on_step_log = |stream: &str, line: &str| {
                logger_for_log.write(line);
                let _ = channel_for_log.send(WorkflowEvent::LogChunk {
                    session_id: sid_for_pr.clone(),
                    stream: stream.to_string(),
                    line: line.to_string(),
                });
            };

            let emitter = Arc::new(StateSavingEmitter::new(
                channel_for_emitter,
                sid_for_pr.clone(),
            ));
            let handler = GuiOptionHandler::new(emitter, sid_for_pr.clone(), option_responder);

            let on_node_start =
                |cp: &NodeCheckpoint<'_>, _dag: &ExecutionDag| -> cruise::error::Result<()> {
                    if let Ok(mgr) = new_session_manager() {
                        if let Ok(mut s) = mgr.load(&sid_for_pr) {
                            s.current_step = Some(cp.node_id.to_string());
                            s.current_step_is_node_id = true;
                            s.has_dag = true;
                            let _ = mgr.save(&s);
                        }
                    }
                    Ok(())
                };

            let mut vars = VariableStore::new(input);
            vars.set_named_file(PLAN_VAR, plan_path);
            let exec_root_path = exec_root.clone();
            let mut tracker = FileTracker::with_root(exec_root);

            let ctx = ExecutionContext {
                compiled: &compiled,
                max_retries: 10,
                rate_limit_retries: DEFAULT_RATE_LIMIT_RETRIES,
                on_step_start: &on_step_start,
                on_step_log: Some(&on_step_log),
                cancel_token: Some(&token_for_task),
                option_handler: &handler,
                config_reloader: None,
                working_dir: Some(&exec_root_path),
                skipped_steps: &skipped_steps,
            };

            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(execute_steps_with_dag(
                &ctx,
                &mut vars,
                &mut tracker,
                &mut dag,
                &start_node,
                &on_node_start,
            ));

            match &result {
                Ok(exec) => logger.write(&format!(
                    "[OK] completed -- run: {}, skipped: {}, failed: {}",
                    exec.run, exec.skipped, exec.failed
                )),
                Err(cruise::error::CruiseError::Interrupted) => {
                    logger.write("[paused] cancelled");
                }
                Err(e) => logger.write(&format!("[FAIL] failed: {}", e.detailed_message())),
            }

            if let Some(dir) = original_dir {
                let _ = std::env::set_current_dir(dir);
            }

            let exec = result?;
            if let Some(ref ctx) = worktree_ctx_for_pr {
                let manager_for_pr =
                    new_session_manager().map_err(cruise::error::CruiseError::Other)?;
                let mut session_for_pr = manager_for_pr
                    .load(&sid_for_pr)
                    .map_err(|e| cruise::error::CruiseError::Other(e.to_string()))?;
                let skipped_steps_for_pr = session_for_pr.skipped_steps.clone();
                let pr_result = handle.block_on(cruise::worktree_pr::handle_worktree_pr(
                    ctx,
                    &compiled,
                    &mut vars,
                    &mut tracker,
                    &mut session_for_pr,
                    5,
                    10,
                    &skipped_steps_for_pr,
                    None,
                ));
                if pr_result.is_ok() {
                    let _ = manager_for_pr.save(&session_for_pr);
                }
                pr_result.map(|()| exec)
            } else {
                Ok(exec)
            }
        },
    )
    .await;

    // Unregister the session from AppState regardless of panic/error.
    state.unregister_session(sid_for_cleanup.as_str());

    let exec_result = join_result.map_err(|e| format!("execution task panicked: {e}"))?;

    // Reload session to pick up any intermediate saves, then apply the final phase.
    let mut final_session = manager.load(session_id).unwrap_or(session);
    final_session.awaiting_input = false;

    match exec_result {
        Ok(exec) => {
            final_session.clear_runner();
            final_session.phase = SessionPhase::Completed;
            final_session.completed_at = Some(current_iso8601());
            // Repo-backed sessions: drop the temporary clone (and its
            // worktree) once the PR has been created.
            if final_session.repo.is_some() && final_session.pr_url.is_some() {
                cruise::repo_clone::cleanup_session_workspace(manager, &final_session);
                final_session.worktree_path = None;
            }
            let _ = channel.send(WorkflowEvent::WorkflowCompleted {
                session_id: sid_for_cleanup.clone(),
                run: exec.run,
                skipped: exec.skipped,
                failed: exec.failed,
            });
            manager.save(&final_session).map_err(|e| e.to_string())?;
            Ok(SessionPhase::Completed)
        }
        Err(cruise::error::CruiseError::Interrupted) => {
            final_session.clear_runner();
            final_session.phase = SessionPhase::Suspended;
            let _ = channel.send(WorkflowEvent::WorkflowCancelled {
                session_id: sid_for_cleanup.clone(),
            });
            manager.save(&final_session).map_err(|e| e.to_string())?;
            Ok(SessionPhase::Suspended)
        }
        Err(e) => {
            let msg = e.to_string();
            final_session.clear_runner();
            final_session.phase = SessionPhase::Failed(msg.clone());
            final_session.completed_at = Some(current_iso8601());
            let _ = channel.send(WorkflowEvent::WorkflowFailed {
                session_id: sid_for_cleanup.clone(),
                error: msg.clone(),
            });
            // Ignore save errors so the original workflow error is preserved.
            let _ = manager.save(&final_session);
            Ok(SessionPhase::Failed(msg))
        }
    }
}

/// Execute a session's workflow, streaming [`WorkflowEvent`]s over `channel`.
///
/// Delegates to [`execute_single_session`] and converts the terminal phase into
/// the return value expected by the Tauri IPC layer (`Ok(())` for Completed /
/// Suspended, `Err(msg)` for Failed).
#[tauri::command]
pub async fn run_session(
    session_id: String,
    workspace_mode: WorkspaceMode,
    channel: tauri::ipc::Channel<WorkflowEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    let manager = new_session_manager()?;
    match execute_single_session(
        &session_id,
        workspace_mode,
        &channel,
        &state,
        &manager,
        cruise::cancellation::CancellationToken::new(),
    )
    .await?
    {
        SessionPhase::Failed(msg) => Err(msg),
        _ => Ok(()),
    }
}

/// Execute all Planned / Suspended sessions with bounded concurrency, streaming batch-level
/// [`WorkflowEvent`]s (plus the per-session events from each run) over `channel`.
///
/// Individual session failures are logged and the batch continues. Cancelling any
/// active Run All session propagates through the shared batch token so no new
/// sessions are scheduled.
#[tauri::command]
pub async fn run_all_sessions(
    channel: tauri::ipc::Channel<WorkflowEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    let manager = Arc::new(new_session_manager()?);
    let parallelism = cruise::app_config::AppConfig::load()
        .map_err(|e| e.to_string())?
        .run_all_parallelism;
    let total = manager
        .run_all_remaining(&std::collections::HashSet::new())
        .map_err(|e| e.to_string())?
        .len();
    let _ = channel.send(WorkflowEvent::RunAllStarted { total, parallelism });

    let app_state = state.inner().clone();
    app_state.set_run_all_parallelism(parallelism);
    let batch_cancel_token = cruise::cancellation::CancellationToken::new();
    app_state.register_batch_cancel_token(batch_cancel_token.clone());
    let channel_for_batch = channel.clone();
    let manager_for_batch = Arc::clone(&manager);
    let results = run_all_with_dynamic_parallelism(
        &manager,
        {
            let app_state = app_state.clone();
            move || app_state.get_run_all_parallelism()
        },
        batch_cancel_token,
        move |session: SessionState, cancel_token| {
            let channel = channel_for_batch.clone();
            let manager = Arc::clone(&manager_for_batch);
            let state = app_state.clone();
            async move {
                let session_id = session.id.clone();
                let input = session.input.clone();
                let workspace_mode = session.workspace_mode;

                let _ = channel.send(WorkflowEvent::RunAllSessionStarted {
                    session_id: session_id.clone(),
                    input: input.clone(),
                });

                let phase = execute_single_session(
                    &session_id,
                    workspace_mode,
                    &channel,
                    &state,
                    &manager,
                    cancel_token,
                )
                .await
                .unwrap_or_else(SessionPhase::Failed);

                let error = match &phase {
                    SessionPhase::Failed(msg) => Some(msg.clone()),
                    _ => None,
                };

                let _ = channel.send(WorkflowEvent::RunAllSessionFinished {
                    session_id,
                    input,
                    phase: phase.label().to_string(),
                    error,
                });

                match phase {
                    SessionPhase::Suspended => Err(cruise::error::CruiseError::Interrupted),
                    _ => Ok(()),
                }
            }
        },
    )
    .await;
    state.clear_batch_cancel_token();
    let results = results.map_err(|e| e.to_string())?;

    let cancelled = results
        .iter()
        .filter(|result| matches!(result.outcome, Err(cruise::error::CruiseError::Interrupted)))
        .count();

    let _ = channel.send(WorkflowEvent::RunAllCompleted { cancelled });

    Ok(())
}

/// Normalize a raw GUI `base_dir` string (expand leading `~`) and resolve the
/// workflow config relative to that directory.
///
/// Returns `(normalized_base_dir, yaml_content, config_source)`.
pub(crate) fn resolve_gui_session_paths(
    base_dir_raw: &str,
    explicit_config: Option<&str>,
) -> std::result::Result<(PathBuf, String, cruise::resolver::ConfigSource), String> {
    let normalized = PathBuf::from(expand_tilde(base_dir_raw));

    let (yaml, source) = cruise::resolver::resolve_config_in_dir(explicit_config, &normalized)
        .map_err(|e| e.to_string())?;

    Ok((normalized, yaml, source))
}

/// Determine whether the current launch context supports automatic in-place update.
///
/// The Tauri updater on macOS replaces the `.app` bundle in-place using the path
/// derived from `current_exe()`.  If the app is running from App Translocation or
/// a mounted DMG volume the replacement targets a temporary copy and the update
/// appears to revert on next launch.
///
/// Extracted from [`get_update_readiness`] for unit-testability.
/// On non-macOS platforms this always returns `can_auto_update = true`.
pub fn check_update_readiness_for_path(exe_path: &std::path::Path) -> UpdateReadinessDto {
    // Walk ancestor components to find the nearest .app bundle root.
    let bundle_path = {
        let mut result = None;
        let mut current = exe_path;
        loop {
            if current.to_str().is_some_and(|s| s.ends_with(".app")) {
                result = Some(current.to_string_lossy().into_owned());
                break;
            }
            match current.parent() {
                Some(p) if p != current => current = p,
                _ => break,
            }
        }
        result
    };

    let path_str = exe_path.to_string_lossy();

    if path_str.contains("/AppTranslocation/") {
        return UpdateReadinessDto {
            can_auto_update: false,
            reason: Some("translocated".to_string()),
            bundle_path,
            guidance: Some(
                "Move cruise.app to /Applications, then relaunch before updating.".to_string(),
            ),
        };
    }

    if path_str.starts_with("/Volumes/") {
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

/// Return the current application-level configuration from `~/.config/cruise/config.json`.
///
/// If the file does not exist, returns the default config (`run_all_parallelism: 1`).
#[tauri::command]
pub fn get_app_config() -> std::result::Result<cruise::app_config::AppConfig, String> {
    cruise::app_config::AppConfig::load().map_err(|e| e.to_string())
}

/// Persist an updated application-level configuration to `~/.config/cruise/config.json`.
///
/// Validates the config before writing (e.g. `run_all_parallelism` must be >= 1).
/// On success, also updates the in-memory runtime parallelism so that any active
/// Run All batch picks up the new value at its next scheduling boundary.
#[tauri::command]
pub fn update_app_config(
    config: cruise::app_config::AppConfig,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    config.save().map_err(|e| e.to_string())?;
    state.set_run_all_parallelism(config.run_all_parallelism);
    Ok(())
}

/// Return whether the current launch context supports automatic in-place update.
///
/// On macOS the updater replaces the `.app` bundle in-place.  If the app is
/// running from App Translocation or a mounted DMG the replacement targets a
/// temporary copy, causing the update to appear to revert on next launch.
#[tauri::command]
pub fn get_update_readiness() -> UpdateReadinessDto {
    match std::env::current_exe() {
        Ok(path) => check_update_readiness_for_path(&path),
        Err(_) => UpdateReadinessDto {
            can_auto_update: false,
            reason: Some("unknownBundlePath".to_string()),
            bundle_path: None,
            guidance: None,
        },
    }
}

/// Return the latest New Session form draft, or `None` if no draft exists.
#[tauri::command]
pub fn get_new_session_draft() -> std::result::Result<Option<NewSessionDraftDto>, String> {
    Ok(
        NewSessionDraft::load_best_effort().map(|draft| NewSessionDraftDto {
            input: draft.input,
            config_path: draft.requested_config_path,
            base_dir: draft.working_dir,
            repo: draft.repo,
            skipped_steps: draft.skipped_steps,
            updated_at: Some(draft.updated_at),
        }),
    )
}

/// Persist the current New Session form state so it survives restarts.
#[tauri::command]
pub fn save_new_session_draft(draft: NewSessionDraftDto) -> std::result::Result<(), String> {
    let entry = NewSessionDraft {
        input: draft.input,
        requested_config_path: draft.config_path,
        working_dir: draft.base_dir,
        repo: draft.repo,
        skipped_steps: draft.skipped_steps,
        updated_at: cruise::session::current_iso8601(),
    };
    entry
        .save()
        .map_err(|e| format!("failed to save new session draft: {e}"))
}

/// Delete the New Session form draft.
#[tauri::command]
pub fn clear_new_session_draft() -> std::result::Result<(), String> {
    NewSessionDraft::clear().map_err(|e| format!("failed to clear new session draft: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cruise::new_session_history::{NewSessionHistory, NewSessionHistoryEntry};
    use cruise::test_support::{init_git_repo, make_session};

    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Polls `pending` until a sender is available, or panics after 5 seconds.
    fn wait_for_pending(pending: &Arc<Mutex<Option<std::sync::mpsc::Sender<OptionResult>>>>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let guard = pending.lock().unwrap_or_else(|e| panic!("{e}"));
            if guard.is_some() {
                return;
            }
            drop(guard);
            if std::time::Instant::now() >= deadline {
                panic!("wait_for_pending timed out after 5 seconds");
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    #[test]
    fn test_plan_logger_writes_start_boundary_to_run_log() {
        // Given: a persisted session managed by a temp SessionManager
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session_id = "20260613000000".to_string();
        let session = cruise::session::SessionState::new(
            session_id.clone(),
            tmp.path().join("repo"),
            "cruise.yaml".to_string(),
            "add planning log persistence".to_string(),
        );
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: planning logging starts
        let _logger = plan_logger(&manager, &session_id, "planning");

        // Then: the canonical run.log contains a planning start boundary
        let content = std::fs::read_to_string(manager.run_log_path(&session_id))
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            content.contains("--- planning started ---"),
            "run.log should contain planning start boundary, got: {content:?}"
        );
    }

    #[test]
    fn test_plan_chunk_logging_appends_streamed_lines_in_order() {
        // Given: a persisted session with planning logging started
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session_id = "20260613000001".to_string();
        let session = cruise::session::SessionState::new(
            session_id.clone(),
            tmp.path().join("repo"),
            "cruise.yaml".to_string(),
            "stream planner chunks".to_string(),
        );
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        let logger = plan_logger(&manager, &session_id, "planning");

        // When: streamed plan chunks are persisted
        log_plan_chunk(&logger, "first streamed line");
        log_plan_chunk(&logger, "second streamed line");

        // Then: the chunk lines are appended to run.log in callback order
        let content = std::fs::read_to_string(manager.run_log_path(&session_id))
            .unwrap_or_else(|e| panic!("{e:?}"));
        let first = content
            .find("first streamed line")
            .unwrap_or_else(|| panic!("missing first chunk in {content:?}"));
        let second = content
            .find("second streamed line")
            .unwrap_or_else(|| panic!("missing second chunk in {content:?}"));
        assert!(
            first < second,
            "streamed lines should preserve order: {content:?}"
        );
    }

    #[test]
    fn test_plan_success_and_failure_helpers_append_terminal_lines() {
        // Given: a persisted session with planning logging started
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session_id = "20260613000002".to_string();
        let session = cruise::session::SessionState::new(
            session_id.clone(),
            tmp.path().join("repo"),
            "cruise.yaml".to_string(),
            "record terminal planning status".to_string(),
        );
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        let logger = plan_logger(&manager, &session_id, "planning");

        // When: terminal statuses are logged
        log_plan_success(&logger, "plan generated");
        log_plan_failure(&logger, "planning failed: boom");

        // Then: run.log contains concise OK/FAIL markers
        let content = std::fs::read_to_string(manager.run_log_path(&session_id))
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            content.contains("[OK] plan generated"),
            "missing success marker: {content:?}"
        );
        assert!(
            content.contains("[FAIL] planning failed: boom"),
            "missing failure marker: {content:?}"
        );
    }

    #[test]
    fn test_session_dto_from_session_includes_title() {
        // Given: a session with a generated title
        let mut session = cruise::session::SessionState::new(
            "20260321120000".to_string(),
            std::path::PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "raw input".to_string(),
        );
        session.title = Some("Readable session title".to_string());

        // When: converting to the IPC DTO
        let dto = SessionDto::from(session);

        // Then: title is preserved for the frontend
        assert_eq!(dto.title.as_deref(), Some("Readable session title"));
        assert_eq!(dto.input, "raw input");
    }

    #[test]
    fn test_session_dto_from_session_title_is_none_when_not_yet_generated() {
        // Given: a session without a generated title
        let session = cruise::session::SessionState::new(
            "20260321120001".to_string(),
            std::path::PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "raw input".to_string(),
        );

        // When: converting to the IPC DTO
        let dto = SessionDto::from(session);

        // Then: title remains absent and the raw input is still available
        assert_eq!(dto.title, None);
        assert_eq!(dto.input, "raw input");
    }

    #[test]
    fn test_session_dto_fix_in_progress_defaults_to_false() {
        // Given: a freshly created session
        let session = cruise::session::SessionState::new(
            "20260407000000".to_string(),
            std::path::PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "test input".to_string(),
        );

        // When: converting to the IPC DTO (no AppState, so fix_in_progress is not set)
        let dto = SessionDto::from(session);

        // Then: fix_in_progress is false; it is populated from AppState in list_sessions / get_session
        assert!(!dto.fix_in_progress);
    }

    #[test]
    fn test_get_new_session_history_summary_prefers_latest_gui_entry_even_when_auto() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());

        let mut history = NewSessionHistory::default();
        history.record_selection(NewSessionHistoryEntry {
            selected_at: String::new(),
            input: String::new(),
            requested_config_path: Some("/Users/takumi/.cruise/team.yaml".to_string()),
            working_dir: "/Users/takumi/projects/demo".to_string(),
            repo: None,
            resolved_config_key: "/Users/takumi/.cruise/team.yaml".to_string(),
            skipped_steps: vec!["review".to_string()],
        });
        history.record_selection(NewSessionHistoryEntry {
            selected_at: String::new(),
            input: String::new(),
            requested_config_path: None,
            working_dir: "/Users/takumi/projects/another-repo".to_string(),
            repo: None,
            resolved_config_key: BUILTIN_CONFIG_KEY.to_string(),
            skipped_steps: vec!["write-tests".to_string()],
        });
        history.save().unwrap_or_else(|e| panic!("{e}"));

        let summary = get_new_session_history_summary().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(summary.last_requested_config_path, None);
        assert_eq!(
            summary.last_working_dir.as_deref(),
            Some("/Users/takumi/projects/another-repo")
        );
        assert_eq!(
            summary.recent_working_dirs,
            vec![
                "/Users/takumi/projects/another-repo".to_string(),
                "/Users/takumi/projects/demo".to_string(),
            ]
        );
    }

    #[test]
    fn test_get_new_session_history_summary_filters_temp_dir_entries() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());

        let mut history = NewSessionHistory::default();
        // Normal entry should appear
        history.record_selection(NewSessionHistoryEntry {
            selected_at: String::new(),
            input: String::new(),
            requested_config_path: None,
            working_dir: "/Users/takumi/projects/cruise".to_string(),
            repo: None,
            resolved_config_key: BUILTIN_CONFIG_KEY.to_string(),
            skipped_steps: vec![],
        });
        // Temp dir entry saved directly (bypass record_selection's filter to simulate legacy data)
        history.entries.insert(
            0,
            NewSessionHistoryEntry {
                selected_at: "2026-01-01T00:00:00Z".to_string(),
                input: String::new(),
                requested_config_path: None,
                working_dir: "/var/folders/4r/cb2pswws7fsctl8ksr1xpk100000gn/T/.tmpXYZ/repo"
                    .to_string(),
                repo: None,
                resolved_config_key: BUILTIN_CONFIG_KEY.to_string(),
                skipped_steps: vec![],
            },
        );
        history.save().unwrap_or_else(|e| panic!("{e}"));

        let summary = get_new_session_history_summary().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(
            summary.last_working_dir.as_deref(),
            Some("/Users/takumi/projects/cruise"),
            "temp dir should be filtered from last_working_dir"
        );
        assert_eq!(
            summary.recent_working_dirs,
            vec!["/Users/takumi/projects/cruise".to_string()],
            "temp dir should not appear in recent_working_dirs"
        );
    }

    #[test]
    fn test_prepare_run_session_uses_requested_workspace_mode_for_fresh_runs() {
        // Given: a fresh planned session and a current-branch run request from the GUI
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session_id = "20260321121000";
        let session = make_session(session_id, &repo);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        let mut loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));

        // When: the backend prepares the run before spawning execution
        let exec_root = prepare_run_session(&manager, &mut loaded, WorkspaceMode::CurrentBranch)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the requested mode is persisted and the run targets the base repository
        assert_eq!(exec_root, repo);
        let saved = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(saved.phase, SessionPhase::Running);
        assert_eq!(saved.workspace_mode, WorkspaceMode::CurrentBranch);
        assert_eq!(saved.target_branch.as_deref(), Some("main"));
        assert!(saved.worktree_path.is_none());
        assert!(saved.worktree_branch.is_none());
    }

    #[test]
    fn test_prepare_run_session_resumes_with_saved_workspace_mode() {
        // Given: a resume/retry session already pinned to current-branch mode
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        fs::write(repo.join("resume-dirty.txt"), "dirty").unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session_id = "20260321121001";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::Suspended;
        session.current_step = Some("edit".to_string());
        session.workspace_mode = WorkspaceMode::CurrentBranch;
        session.target_branch = Some("main".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        let mut loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));

        // When: the GUI asks to rerun with a different mode
        let exec_root = prepare_run_session(&manager, &mut loaded, WorkspaceMode::Worktree)
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the saved workspace mode wins and no worktree is created mid-session
        assert_eq!(exec_root, repo);
        let saved = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(saved.phase, SessionPhase::Running);
        assert_eq!(saved.workspace_mode, WorkspaceMode::CurrentBranch);
        assert!(saved.worktree_path.is_none());
        assert!(saved.worktree_branch.is_none());
        assert!(
            !manager.worktrees_dir().join(session_id).exists(),
            "resume should not switch to a newly created worktree"
        );
    }

    #[test]
    fn test_prepare_run_session_does_not_persist_running_phase_when_workspace_setup_fails() {
        // Given: a fresh current-branch run request against a dirty repository
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        // Modify an already-tracked file to make the working tree dirty
        // (is_working_tree_dirty uses --untracked-files=no, so new files are ignored)
        fs::write(repo.join("README.md"), "dirty modification").unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session_id = "20260321121002";
        let session = make_session(session_id, &repo);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        let mut loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));

        // When: workspace preparation fails before execution starts
        let error = prepare_run_session(&manager, &mut loaded, WorkspaceMode::CurrentBranch)
            .map_or_else(|e| e, |_| panic!("expected workspace preparation to fail"));

        // Then: the session remains runnable instead of being left in Running phase
        assert!(
            error.to_string().contains("dirty"),
            "unexpected error: {error}"
        );
        let saved = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(saved.phase, SessionPhase::Planned);
    }

    // --- Integration: full option-selection round-trip ------------------------
    //
    // Data flow:
    //   GuiOptionHandler::select_option (engine thread)
    //     -> stores sender in shared pending_response slot
    //     -> emits WorkflowEvent::OptionRequired
    //   test thread: extracts sender from slot and sends OptionResult
    //   GuiOptionHandler::select_option (engine thread)
    //     -> blocking_recv returns OptionResult
    //
    // Modules covered: events, gui_option_handler, state, commands
    //
    #[test]
    fn test_option_flow_integration_select_and_respond_round_trip() {
        use crate::events::WorkflowEvent;
        use crate::gui_option_handler::{EventEmitter, GuiOptionHandler};
        use cruise::option_handler::OptionHandler;
        use cruise::step::OptionChoice;

        /// Minimal emitter that records the last emitted event.
        struct CapturingEmitter {
            last: Mutex<Option<WorkflowEvent>>,
        }
        impl CapturingEmitter {
            fn new() -> Self {
                Self {
                    last: Mutex::new(None),
                }
            }
        }
        impl EventEmitter for CapturingEmitter {
            fn emit(&self, event: WorkflowEvent) {
                *self.last.lock().unwrap_or_else(|e| panic!("{e}")) = Some(event);
            }
        }

        // Given: a GuiOptionHandler wired to a shared pending_response slot
        let emitter = Arc::new(CapturingEmitter::new());
        let pending: Arc<Mutex<Option<std::sync::mpsc::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(None));
        let handler = GuiOptionHandler::new(
            Arc::clone(&emitter),
            "integration-req".to_string(),
            Arc::clone(&pending),
        );
        let choices = vec![OptionChoice::Selector {
            label: "Proceed".to_string(),
            next: Some("finalize".to_string()),
        }];

        // When: the engine thread calls select_option (blocks until response)
        let pending_for_cmd = Arc::clone(&pending);
        let engine_thread =
            std::thread::spawn(move || handler.select_option(&choices, Some("plan text")));

        // And: the IPC command thread responds once the sender is populated
        wait_for_pending(&pending_for_cmd);
        let sender = pending_for_cmd
            .lock()
            .unwrap_or_else(|e| panic!("{e}"))
            .take()
            .expect("sender should be present after wait_for_pending");
        sender
            .send(OptionResult {
                next_step: Some("finalize".to_string()),
                text_input: None,
            })
            .unwrap_or_else(|_| panic!("respond_to_option: receiver dropped"));

        // Then: the engine thread receives the OptionResult
        let result = engine_thread
            .join()
            .unwrap_or_else(|e| panic!("engine thread panicked: {e:?}"))
            .unwrap_or_else(|e| panic!("select_option failed: {e}"));
        assert_eq!(result.next_step, Some("finalize".to_string()));
        assert_eq!(result.text_input, None);

        // And: the OptionRequired event was emitted with the correct data
        let emitted = emitter.last.lock().unwrap_or_else(|e| panic!("{e}")).take();
        match emitted {
            Some(WorkflowEvent::OptionRequired {
                request_id,
                plan,
                choices,
            }) => {
                assert_eq!(request_id, "integration-req");
                assert_eq!(plan.as_deref(), Some("plan text"));
                assert_eq!(choices.len(), 1);
                assert_eq!(choices[0].label, "Proceed");
            }
            other => panic!("expected OptionRequired event, got: {other:?}"),
        }
    }

    // --- check_update_readiness_for_path -------------------------------------

    #[test]
    fn test_readiness_normal_applications_path_allows_update() {
        // Given: exe is inside a normal /Applications/ .app bundle
        let exe = Path::new("/Applications/cruise.app/Contents/MacOS/cruise");
        // When: readiness is checked
        let r = check_update_readiness_for_path(exe);
        // Then: update is allowed and no reason is set
        assert!(r.can_auto_update);
        assert!(r.reason.is_none());
    }

    #[test]
    fn test_readiness_app_translocation_path_blocks_update() {
        // Given: exe is in an App Translocation sandbox created by macOS Gatekeeper
        let exe = Path::new(
            "/private/var/folders/xx/yyy/T/AppTranslocation/AABBCCDD/d/cruise.app/Contents/MacOS/cruise",
        );
        // When: readiness is checked
        let r = check_update_readiness_for_path(exe);
        // Then: update is blocked with reason "translocated"
        assert!(!r.can_auto_update);
        assert_eq!(r.reason.as_deref(), Some("translocated"));
    }

    #[test]
    fn test_readiness_mounted_dmg_volume_blocks_update() {
        // Given: exe is running directly from a mounted DMG volume
        let exe = Path::new("/Volumes/cruise 0.1.24/cruise.app/Contents/MacOS/cruise");
        // When: readiness is checked
        let r = check_update_readiness_for_path(exe);
        // Then: update is blocked with reason "mountedVolume"
        assert!(!r.can_auto_update);
        assert_eq!(r.reason.as_deref(), Some("mountedVolume"));
    }

    #[test]
    fn test_readiness_path_without_app_bundle_returns_unknown() {
        // Given: exe path has no .app ancestor component (e.g. a bare binary)
        let exe = Path::new("/usr/local/bin/cruise");
        // When: readiness is checked
        let r = check_update_readiness_for_path(exe);
        // Then: update is blocked with reason "unknownBundlePath"
        assert!(!r.can_auto_update);
        assert_eq!(r.reason.as_deref(), Some("unknownBundlePath"));
    }

    #[test]
    fn test_readiness_translocated_path_reports_bundle_path() {
        // Given: exe inside an App Translocation .app
        let exe = Path::new(
            "/private/var/folders/xx/yyy/T/AppTranslocation/AABBCCDD/d/cruise.app/Contents/MacOS/cruise",
        );
        // When: readiness is checked
        let r = check_update_readiness_for_path(exe);
        // Then: bundle_path ends with ".app" so the UI can display it
        let bundle_path = r.bundle_path.unwrap_or_default();
        assert!(
            bundle_path.ends_with(".app"),
            "expected bundle_path to end with '.app', got: {bundle_path}"
        );
    }

    #[test]
    fn test_readiness_translocated_path_includes_applications_guidance() {
        // Given: exe inside an App Translocation .app
        let exe = Path::new(
            "/private/var/folders/xx/yyy/T/AppTranslocation/AABBCCDD/d/cruise.app/Contents/MacOS/cruise",
        );
        // When: readiness is checked
        let r = check_update_readiness_for_path(exe);
        // Then: guidance mentions /Applications so the user knows where to move the app
        let guidance = r.guidance.unwrap_or_default();
        assert!(
            guidance.contains("/Applications"),
            "expected guidance to mention '/Applications', got: {guidance}"
        );
    }

    #[test]
    fn test_readiness_mounted_volume_includes_applications_guidance() {
        // Given: exe running from a mounted DMG volume
        let exe = Path::new("/Volumes/cruise 0.1.24/cruise.app/Contents/MacOS/cruise");
        // When: readiness is checked
        let r = check_update_readiness_for_path(exe);
        // Then: guidance mentions /Applications so the user knows to copy the app first
        let guidance = r.guidance.unwrap_or_default();
        assert!(
            guidance.contains("/Applications"),
            "expected guidance to mention '/Applications', got: {guidance}"
        );
    }

    #[test]
    fn test_readiness_nested_volumes_subpath_blocks_update() {
        // Given: exe path that starts with /Volumes/ but is nested deeper
        let exe = Path::new("/Volumes/ExternalDisk/apps/cruise.app/Contents/MacOS/cruise");
        // When: readiness is checked
        let r = check_update_readiness_for_path(exe);
        // Then: still blocked as mountedVolume
        assert!(!r.can_auto_update);
        assert_eq!(r.reason.as_deref(), Some("mountedVolume"));
    }

    // --- do_ask_session -------------------------------------------------------

    /// Write a minimal `config.yaml` that uses the given shell command as the LLM.
    ///
    /// The command must read stdin (or ignore it) and write to stdout; it does
    /// not need to be an actual language model.
    fn write_test_config(session_dir: &std::path::Path, shell_command: &str) {
        let yaml = format!("command:\n  - bash\n  - -c\n  - \"{shell_command}\"\nsteps: {{}}\n");
        fs::write(session_dir.join("config.yaml"), yaml).unwrap_or_else(|e| panic!("{e}"));
    }

    /// Create a temporary SessionManager with a session that has `plan.md` and `config.yaml`.
    /// Returns `(TempDir, SessionManager)` -- callers must keep `_tmp` alive for the test duration.
    fn setup_ask_session(
        session_id: &str,
        plan_content: &str,
        shell_command: &str,
    ) -> (TempDir, SessionManager) {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session = cruise::session::SessionState::new(
            session_id.to_string(),
            repo,
            "cruise.yaml".to_string(),
            "test task".to_string(),
        );
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        let session_dir = manager.sessions_dir().join(session_id);
        fs::write(session_dir.join("plan.md"), plan_content).unwrap_or_else(|e| panic!("{e}"));
        write_test_config(&session_dir, shell_command);
        (tmp, manager)
    }

    #[test]
    fn test_persist_plan_failure_keeps_session_retryable() {
        // Given: plan generation is waiting for input on an already-persisted Draft session
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = cruise::session::SessionState::new(
            "20260625190000".to_string(),
            tmp.path().join("repo"),
            "cruise.yaml".to_string(),
            "test task".to_string(),
        );
        session.phase = SessionPhase::AwaitingInput;
        session.pending_ask_question = Some("Question?".to_string());
        manager
            .create(&session)
            .unwrap_or_else(|e| panic!("create failed: {e:?}"));

        // When: planning fails
        persist_plan_failure(&manager, &mut session, "model error".to_string())
            .unwrap_or_else(|e| panic!("persist failed: {e}"));

        // Then: the session directory is still present and can be retried as Draft
        assert!(
            manager.sessions_dir().join("20260625190000").exists(),
            "plan failure must not delete the session"
        );
        let saved = manager
            .load("20260625190000")
            .unwrap_or_else(|e| panic!("load failed: {e:?}"));
        assert_eq!(saved.phase, SessionPhase::Draft);
        assert_eq!(saved.plan_error.as_deref(), Some("model error"));
        assert!(saved.pending_ask_question.is_none());
    }

    #[tokio::test]
    async fn test_ask_session_returns_llm_output() {
        // Given: a session with a plan and a config that echoes a fixed answer
        let (_tmp, manager) =
            setup_ask_session("20260326130000", "# Original Plan", "echo ask-answer");

        // Re-load so config_path is correct (config.yaml is in session dir)
        let session = manager
            .load("20260326130000")
            .unwrap_or_else(|e| panic!("{e:?}"));
        manager.save(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: ask_session is called with a question
        let result =
            do_ask_session(&manager, "20260326130000", "What does this do?".to_string()).await;

        // Then: returns Ok (the LLM command ran successfully)
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
        let answer = result.unwrap_or_else(|e| panic!("{e}"));
        assert!(
            answer.contains("ask-answer"),
            "expected answer to contain 'ask-answer', got: {answer}"
        );
    }

    #[tokio::test]
    async fn test_ask_session_does_not_modify_plan_md() {
        // Given: a session with known plan.md content
        let original_plan = "# Original Plan\nDo the thing.";
        let (_tmp, manager) = setup_ask_session(
            "20260326130001",
            original_plan,
            "echo ask-answer; cat > /dev/null",
        );

        // When: ask_session is called
        let _ = do_ask_session(&manager, "20260326130001", "A question?".to_string()).await;

        // Then: plan.md is unchanged
        let session_dir = manager.sessions_dir().join("20260326130001");
        let plan_after =
            fs::read_to_string(session_dir.join("plan.md")).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            plan_after, original_plan,
            "ask_session must not modify plan.md"
        );
    }

    #[tokio::test]
    async fn test_ask_session_does_not_change_session_phase() {
        // Given: a session in AwaitingApproval phase (the default for SessionState::new)
        let (_tmp, manager) = setup_ask_session("20260326130002", "# Plan", "echo answer");

        // When: ask_session is called
        let _ = do_ask_session(&manager, "20260326130002", "A question?".to_string()).await;

        // Then: session phase is still AwaitingApproval
        let saved = manager
            .load("20260326130002")
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            matches!(saved.phase, SessionPhase::AwaitingApproval),
            "ask_session must not mutate session phase, got: {:?}",
            saved.phase
        );
    }

    #[tokio::test]
    async fn test_ask_session_returns_error_when_session_not_found() {
        // Given: no session with the given ID exists
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        // sessions dir doesn't even exist -- load will fail immediately

        // When: ask_session is called with a nonexistent ID
        let result =
            do_ask_session(&manager, "nonexistent-session-id", "Question?".to_string()).await;

        // Then: returns an error
        assert!(result.is_err(), "expected Err for missing session, got Ok");
    }

    // --- resolve_gui_session_paths -------------------------------------------

    #[test]
    fn test_resolve_gui_session_paths_local_config_beats_user_dir() {
        // Given: base_dir contains cruise.yaml; ~/.cruise/default.yaml also exists
        // (Regression: GUI used to resolve config from process cwd, picking user-dir default
        //  instead of the repo-local file.)
        let repo_dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo_dir.path().join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let fake_home = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_home = fake_home.path().join(".cruise");
        fs::create_dir_all(&cruise_home).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            cruise_home.join("default.yaml"),
            "command: [userdir]\nsteps:\n  s:\n    command: userdir",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let _lock = cruise::test_support::lock_process();
        let _home_guard = cruise::test_support::EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _env_guard = cruise::test_support::EnvGuard::remove("CRUISE_CONFIG");

        // When: GUI session paths are resolved for the repo base_dir
        let (base, yaml, source) = resolve_gui_session_paths(
            repo_dir
                .path()
                .to_str()
                .unwrap_or_else(|| panic!("unexpected None")),
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        // Then: the local config is selected (not the user-dir default)
        assert!(
            yaml.contains("local"),
            "expected local config to be selected, got: {yaml}"
        );
        if let cruise::resolver::ConfigSource::Local(p) = &source {
            assert_eq!(
                p,
                &repo_dir.path().join("cruise.yaml"),
                "config_path must be <repo>/cruise.yaml"
            );
        } else {
            panic!("expected ConfigSource::Local, got: {source:?}");
        }
        // And the returned base_dir matches the input
        assert_eq!(base, repo_dir.path());
    }

    #[test]
    fn test_resolve_gui_session_paths_expands_tilde_in_base_dir() {
        // Given: base_dir starts with ~
        let fake_home = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let target = fake_home.path().join("myrepo");
        fs::create_dir_all(&target).unwrap_or_else(|e| panic!("{e:?}"));

        let _lock = cruise::test_support::lock_process();
        let _home_guard = cruise::test_support::EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _env_guard = cruise::test_support::EnvGuard::remove("CRUISE_CONFIG");

        // When: base_dir with tilde is resolved
        let (base, _yaml, _source) =
            resolve_gui_session_paths("~/myrepo", None).unwrap_or_else(|e| panic!("{e}"));

        // Then: the returned base path is absolute (tilde expanded)
        assert!(
            base.is_absolute(),
            "normalized base_dir must be absolute, got: {}",
            base.display()
        );
        assert_eq!(base, target, "tilde must expand to home + suffix");
    }

    #[test]
    fn test_resolve_gui_session_paths_explicit_config_wins_over_local() {
        // Given: base_dir has cruise.yaml, and an explicit config path is also provided
        let repo_dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo_dir.path().join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let explicit_file = tempfile::NamedTempFile::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            explicit_file.path(),
            "command: [explicit]\nsteps:\n  s:\n    command: explicit",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let explicit_path = explicit_file
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("unexpected None"))
            .to_string();

        let _lock = cruise::test_support::lock_process();
        let _env_guard = cruise::test_support::EnvGuard::remove("CRUISE_CONFIG");

        // When: explicit config is specified
        let (_, yaml, source) = resolve_gui_session_paths(
            repo_dir
                .path()
                .to_str()
                .unwrap_or_else(|| panic!("unexpected None")),
            Some(&explicit_path),
        )
        .unwrap_or_else(|e| panic!("{e}"));

        // Then: explicit config wins over local repo config
        assert!(
            yaml.contains("explicit"),
            "expected explicit config, got: {yaml}"
        );
        assert!(
            matches!(source, cruise::resolver::ConfigSource::Explicit(_)),
            "expected ConfigSource::Explicit, got: {source:?}"
        );
    }

    #[test]
    fn test_resolve_gui_session_paths_normalized_base_matches_absolute_input() {
        // Given: base_dir is already an absolute path (no tilde)
        let repo_dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));

        let _lock = cruise::test_support::lock_process();
        let _home_guard = cruise::test_support::EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _env_guard = cruise::test_support::EnvGuard::remove("CRUISE_CONFIG");

        let raw = repo_dir
            .path()
            .to_str()
            .unwrap_or_else(|| panic!("unexpected None"));

        // When: resolved without tilde
        let (base, _yaml, _source) =
            resolve_gui_session_paths(raw, None).unwrap_or_else(|e| panic!("{e}"));

        // Then: the returned base_dir equals the input path exactly
        assert_eq!(base, repo_dir.path());
    }

    // --- prepare_run_session: worktree gh preflight --------------------------

    /// Given: worktree-mode session, no `gh` in PATH (empty bin directory)
    /// When:  prepare_run_session is called with WorkspaceMode::Worktree
    /// Then:  fails with a gh-related error before saving the Running phase
    ///
    /// This test verifies the new preflight behaviour required by the plan:
    /// the GUI must check that `gh` is available **before** committing the
    /// session to `Running`, just as the CLI already does in `run_cmd.rs`.
    #[cfg(unix)]
    #[test]
    fn test_prepare_run_session_worktree_fails_before_running_when_gh_not_available() {
        // Given: a fresh worktree-mode session (WorkspaceMode::Worktree is the
        // default created by make_session / SessionState::new)
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session_id = "20260406100000";
        let session = make_session(session_id, &repo);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        let mut loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));

        // Replace PATH with an empty directory so gh (and git) cannot be found.
        // The gh preflight must fire before any git worktree operations.
        let empty_bin = tmp.path().join("empty_bin");
        fs::create_dir_all(&empty_bin).unwrap_or_else(|e| panic!("{e:?}"));
        let _lock = cruise::test_support::lock_process();
        let _path_guard = cruise::test_support::EnvGuard::set("PATH", empty_bin.as_os_str());

        // When: prepare_run_session is called with worktree mode
        let result = prepare_run_session(&manager, &mut loaded, WorkspaceMode::Worktree);

        // Then: fails with an error that mentions "gh"
        assert!(
            result.is_err(),
            "expected prepare_run_session to fail when gh is absent"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.to_lowercase().contains("gh"),
            "error should mention gh: {err}"
        );

        // And: the session is NOT saved in Running phase (preflight fired early)
        let saved = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            saved.phase,
            SessionPhase::Planned,
            "session should remain Planned when gh preflight fails"
        );
    }

    /// Given: worktree-mode session, a fake `gh` is available in PATH
    /// When:  prepare_run_session is called with WorkspaceMode::Worktree
    /// Then:  the session is saved as Running and a workspace is returned
    ///
    /// Positive counterpart of the gh-preflight test: proves that a
    /// correctly installed `gh` does not block the run.
    #[cfg(unix)]
    #[test]
    fn test_prepare_run_session_worktree_succeeds_when_gh_available() {
        // Given: a fresh worktree-mode session with a fake gh that passes --version
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let session_id = "20260406100001";
        let session = make_session(session_id, &repo);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        let mut loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));

        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap_or_else(|e| panic!("{e:?}"));
        cruise::test_support::install_version_only_gh(&bin_dir);

        let _lock = cruise::test_support::lock_process();
        let _path_guard = cruise::test_support::prepend_to_path(&bin_dir);

        // When: prepare_run_session is called with worktree mode
        let result = prepare_run_session(&manager, &mut loaded, WorkspaceMode::Worktree);

        // Then: succeeds and the session is saved as Running
        assert!(
            result.is_ok(),
            "expected prepare_run_session to succeed when gh is available: {result:?}"
        );
        let saved = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(saved.phase, SessionPhase::Running);
        assert_eq!(saved.workspace_mode, WorkspaceMode::Worktree);
        assert!(
            saved.worktree_branch.is_some(),
            "worktree branch should be set"
        );
    }

    // --- Post-plan session editing tests -----------------------------------------

    #[test]
    fn test_session_dto_includes_config_path_and_skipped_steps() {
        let mut session = cruise::session::SessionState::new(
            "20260410000000".to_string(),
            std::path::PathBuf::from("/repo"),
            "cruise.yaml".to_string(),
            "test input".to_string(),
        );
        session.skipped_steps = vec!["build".to_string(), "test".to_string()];

        let dto = SessionDto::from(session.clone());

        assert_eq!(dto.config_source, "cruise.yaml");
        assert!(dto.config_path.is_none());
        assert_eq!(
            dto.skipped_steps,
            vec!["build".to_string(), "test".to_string()]
        );
    }

    #[test]
    fn test_update_session_settings_succeeds_for_awaiting_approval_session() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo.join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260410000001";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::AwaitingApproval;
        session.skipped_steps = vec![];
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        let result = update_session_settings(
            &manager,
            session_id,
            None,
            vec!["build".to_string()],
            CurrentStepUpdate::Unchanged,
        );

        assert!(
            result.is_ok(),
            "update_session_settings should succeed for AwaitingApproval"
        );
        let updated = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(updated.skipped_steps, vec!["build"]);
    }

    #[test]
    fn test_update_session_settings_succeeds_for_planned_session() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo.join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260410000002";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::Planned;
        session.skipped_steps = vec![];
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        let result = update_session_settings(
            &manager,
            session_id,
            None,
            vec!["build".to_string()],
            CurrentStepUpdate::Unchanged,
        );

        assert!(
            result.is_ok(),
            "update_session_settings should succeed for Planned"
        );
        let updated = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(updated.skipped_steps, vec!["build"]);
    }

    #[test]
    fn test_update_session_settings_fails_for_running_session() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260410000003";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::Running;
        session.skipped_steps = vec![];
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        let result = update_session_settings(
            &manager,
            session_id,
            None,
            vec!["build".to_string()],
            CurrentStepUpdate::Unchanged,
        );

        assert!(
            result.is_err(),
            "update_session_settings should fail for Running"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Running"),
            "error should mention phase restriction: {err_msg}"
        );
    }

    #[test]
    fn test_update_session_settings_succeeds_for_suspended_session() {
        // Given: a Suspended session with a config file (function now reaches config loading)
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo.join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260410000004";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::Suspended;
        session.skipped_steps = vec![];
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: skip-only edit, no current_step change
        let result = update_session_settings(
            &manager,
            session_id,
            None,
            vec!["s".to_string()],
            CurrentStepUpdate::Unchanged,
        );

        // Then: Suspended should now be an allowed phase for skip edits
        assert!(
            result.is_ok(),
            "update_session_settings should succeed for Suspended: {:?}",
            result.err()
        );
        let updated = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(updated.skipped_steps, vec!["s"]);
    }

    #[test]
    fn test_update_session_settings_succeeds_for_failed_session() {
        // Given: a Failed session with a config file (function now reaches config loading)
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo.join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260410000005";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::Failed("test error".to_string());
        session.skipped_steps = vec![];
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: skip-only edit, no current_step change
        let result = update_session_settings(
            &manager,
            session_id,
            None,
            vec!["s".to_string()],
            CurrentStepUpdate::Unchanged,
        );

        // Then: Failed should now be an allowed phase for skip edits
        assert!(
            result.is_ok(),
            "update_session_settings should succeed for Failed: {:?}",
            result.err()
        );
        let updated = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(updated.skipped_steps, vec!["s"]);
    }

    #[test]
    fn test_update_session_settings_fails_for_completed_session() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260410000006";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::Completed;
        session.skipped_steps = vec![];
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        let result = update_session_settings(
            &manager,
            session_id,
            None,
            vec!["build".to_string()],
            CurrentStepUpdate::Unchanged,
        );

        assert!(
            result.is_err(),
            "update_session_settings should fail for Completed"
        );
    }

    #[test]
    fn test_update_session_settings_builtin_config_writes_config_yaml() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        // Use an empty fake HOME so the resolver falls through to the built-in default
        // (no local cruise.yaml in repo, no CRUISE_CONFIG, no ~/.cruise/*.yaml files).
        let _home_guard = cruise::test_support::EnvGuard::set("HOME", tmp.path().as_os_str());
        let _env_guard = cruise::test_support::EnvGuard::remove("CRUISE_CONFIG");

        let session_id = "20260410000007";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::AwaitingApproval;
        session.config_source = BUILTIN_CONFIG_KEY.to_string();
        session.config_path = None;
        session.skipped_steps = vec![];
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        let result = update_session_settings(
            &manager,
            session_id,
            None,
            vec!["build".to_string()],
            CurrentStepUpdate::Unchanged,
        );

        assert!(result.is_ok());
        let config_yaml_path = manager.sessions_dir().join(session_id).join("config.yaml");
        assert!(
            config_yaml_path.exists(),
            "builtin config switch should write config.yaml to session dir"
        );
    }

    #[test]
    fn test_create_draft_session_creates_draft_phase_without_plan() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());
        let _env_guard = cruise::test_support::EnvGuard::remove("CRUISE_CONFIG");
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = create_draft_session_impl(
            &manager,
            "do the thing".to_string(),
            None,
            repo.to_string_lossy().into_owned(),
            None,
            vec!["build".to_string()],
            vec![],
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let session = manager
            .load(&session_id)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(matches!(session.phase, SessionPhase::Draft));
        assert_eq!(session.input, "do the thing");
        assert_eq!(session.skipped_steps, vec!["build".to_string()]);

        // A draft must NOT have a plan.md.
        let plan_path = session.plan_path(&manager.sessions_dir());
        assert!(
            !plan_path.exists(),
            "draft session should not have a plan.md"
        );
        // Built-in config switch writes config.yaml to the session dir.
        let config_yaml_path = manager.sessions_dir().join(&session_id).join("config.yaml");
        assert!(
            config_yaml_path.exists(),
            "builtin config should write config.yaml to session dir"
        );
    }

    #[test]
    fn test_create_draft_session_trims_input() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());
        let _env_guard = cruise::test_support::EnvGuard::remove("CRUISE_CONFIG");
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = create_draft_session_impl(
            &manager,
            "  padded input  ".to_string(),
            None,
            repo.to_string_lossy().into_owned(),
            None,
            vec![],
            vec![],
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let session = manager
            .load(&session_id)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(session.input, "padded input");
    }

    #[test]
    fn test_update_session_settings_external_config_updates_paths() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260410000008";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::AwaitingApproval;
        session.config_source = BUILTIN_CONFIG_KEY.to_string();
        session.config_path = None;
        session.skipped_steps = vec![];
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        let custom_config = tmp.path().join("custom.yaml");
        fs::write(
            &custom_config,
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let external_config = custom_config.to_string_lossy().to_string();

        let result = update_session_settings(
            &manager,
            session_id,
            Some(external_config.clone()),
            vec![],
            CurrentStepUpdate::Unchanged,
        );

        assert!(result.is_ok());
        let updated = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(updated.config_source, format!("config: {external_config}"));
        assert!(updated.config_path.is_some());
    }

    #[test]
    fn test_update_session_settings_failed_updates_current_step() {
        // Given: a Failed session with config
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo.join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260620000001";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::Failed("s failed".to_string());
        session.current_step = None;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: set current_step to "s" (exists in compiled workflow)
        let result = update_session_settings(
            &manager,
            session_id,
            None,
            vec![],
            CurrentStepUpdate::Set("s".to_string()),
        );

        // Then: succeeds and current_step is persisted
        assert!(
            result.is_ok(),
            "Failed phase should allow current_step update: {:?}",
            result.err()
        );
        let updated = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            updated.current_step,
            Some("s".to_string()),
            "current_step should be set to 's'"
        );
    }

    #[test]
    fn test_update_session_settings_suspended_clears_current_step() {
        // Given: a Suspended session with current_step set
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo.join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260620000002";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::Suspended;
        session.current_step = Some("s".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: clear current_step (Some(None) = from beginning)
        let result =
            update_session_settings(&manager, session_id, None, vec![], CurrentStepUpdate::Clear);

        // Then: succeeds and current_step is cleared
        assert!(
            result.is_ok(),
            "Suspended phase should allow current_step clear: {:?}",
            result.err()
        );
        let updated = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(updated.current_step, None, "current_step should be cleared");
    }

    #[test]
    fn test_update_session_settings_failed_rejects_config_swap() {
        // Given: a Failed session with its current config
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo.join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let alt_config = tmp.path().join("alt.yaml");
        fs::write(
            &alt_config,
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260620000003";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::Failed("err".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: try to change config_path on a Failed session
        let result = update_session_settings(
            &manager,
            session_id,
            Some(alt_config.to_string_lossy().into_owned()),
            vec![],
            CurrentStepUpdate::Unchanged,
        );

        // Then: must reject — config swapping is prohibited for Failed/Suspended
        assert!(
            result.is_err(),
            "Failed phase should reject config_path swap"
        );
    }

    #[test]
    fn test_update_session_settings_planned_rejects_current_step_update() {
        // Given: a Planned session (current_step editing only valid for Failed/Suspended)
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());
        let home = tmp.path();
        let manager = SessionManager::new(home.join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo.join("cruise.yaml"),
            "command: [local]\nsteps:\n  s:\n    command: local",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260620000004";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::Planned;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: try to set current_step on a Planned session
        let result = update_session_settings(
            &manager,
            session_id,
            None,
            vec![],
            CurrentStepUpdate::Set("s".to_string()),
        );

        // Then: must reject — Planned never has an in-progress step to resume from
        assert!(
            result.is_err(),
            "Planned phase should reject current_step update"
        );
    }

    #[test]
    fn test_on_step_start_persists_current_step_to_state_json() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guard = cruise::test_support::set_fake_home(tmp.path());
        let manager =
            SessionManager::new(cruise::paths::data_dir().unwrap_or_else(|e| panic!("{e:?}")));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);

        let session_id = "20260418000001";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::Planned;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        let sid = session_id.to_string();
        let on_step_start = move |step: &str| -> cruise::error::Result<()> {
            if let Ok(mgr) = new_session_manager() {
                if let Ok(mut s) = mgr.load(&sid) {
                    s.current_step = Some(step.to_string());
                    let _ = mgr.save(&s);
                }
            }
            Ok(())
        };

        on_step_start("implement").unwrap_or_else(|e| panic!("{e:?}"));

        let saved = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(saved.current_step, Some("implement".to_string()));
    }

    #[test]
    fn test_get_session_returns_config_path_and_skipped_steps() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260410000009";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::AwaitingApproval;
        session.skipped_steps = vec!["build".to_string()];
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        let dto = SessionDto::from_state(session, &manager);

        assert!(dto.config_path.is_some() || dto.config_source == "cruise.yaml");
        assert_eq!(dto.skipped_steps, vec!["build"]);
    }

    #[test]
    fn test_list_sessions_returns_config_path_and_skipped_steps() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));

        let session_id = "20260410000010";
        let mut session = make_session(session_id, &repo);
        session.phase = SessionPhase::AwaitingApproval;
        session.skipped_steps = vec!["test".to_string()];
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        let sessions = list_sessions_impl(&manager, &AppState::new());
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].skipped_steps, vec!["test"]);
    }

    #[test]
    fn test_new_session_draft_save_and_get_round_trip() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());

        let draft_dto = NewSessionDraftDto {
            input: "test task".to_string(),
            config_path: Some("/tmp/cruise.yaml".to_string()),
            base_dir: "/tmp/project".to_string(),
            repo: None,
            skipped_steps: vec!["review".to_string()],
            updated_at: None,
        };
        save_new_session_draft(draft_dto).unwrap_or_else(|e| panic!("save failed: {e}"));

        let loaded = get_new_session_draft()
            .unwrap_or_else(|e| panic!("get failed: {e}"))
            .unwrap_or_else(|| panic!("expected Some, got None"));
        assert_eq!(loaded.input, "test task");
        assert_eq!(loaded.config_path.as_deref(), Some("/tmp/cruise.yaml"));
        assert_eq!(loaded.base_dir, "/tmp/project");
        assert_eq!(loaded.skipped_steps, vec!["review"]);
    }

    #[test]
    fn test_new_session_draft_get_returns_none_when_no_draft() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());

        let draft = get_new_session_draft().unwrap_or_else(|e| panic!("get failed: {e}"));
        assert!(draft.is_none(), "expected None when no draft exists");
    }

    #[test]
    fn test_new_session_draft_clear_removes_draft() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());

        save_new_session_draft(NewSessionDraftDto {
            input: "temp".to_string(),
            config_path: None,
            base_dir: "/tmp".to_string(),
            repo: None,
            skipped_steps: vec![],
            updated_at: None,
        })
        .unwrap_or_else(|e| panic!("save failed: {e}"));

        clear_new_session_draft().unwrap_or_else(|e| panic!("clear failed: {e}"));

        let draft = get_new_session_draft().unwrap_or_else(|e| panic!("get failed: {e}"));
        assert!(draft.is_none(), "draft should be cleared");
    }

    // ---- read_configs_in ----

    #[test]
    fn test_read_configs_in_with_description() {
        // Given: a YAML file that includes a description
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            dir.path().join("team.yaml"),
            "command: [claude, -p]\ndescription: team-shared\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: reading configs
        let configs = read_configs_in(dir.path());

        // Then: description field is populated
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "team.yaml");
        assert_eq!(configs[0].description, Some("team-shared".to_string()));
    }

    #[test]
    fn test_read_configs_in_without_description() {
        // Given: a YAML file without description
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            dir.path().join("simple.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: reading configs
        let configs = read_configs_in(dir.path());

        // Then: description is None
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "simple.yaml");
        assert_eq!(configs[0].description, None);
    }

    #[test]
    fn test_read_configs_in_broken_yaml_falls_back_to_none() {
        // Given: a file with malformed YAML
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            dir.path().join("broken.yaml"),
            "not: valid: yaml: [unclosed",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: reading configs
        let configs = read_configs_in(dir.path());

        // Then: entry still appears with description = None (no panic, no missing entry)
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "broken.yaml");
        assert_eq!(configs[0].description, None);
    }

    #[test]
    fn test_read_configs_in_empty_dir() {
        // Given: an empty directory
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));

        // When: reading configs
        let configs = read_configs_in(dir.path());

        // Then: empty result
        assert!(configs.is_empty());
    }

    #[test]
    fn test_read_configs_in_excludes_non_yaml() {
        // Given: a directory with YAML and non-YAML files
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            dir.path().join("config.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(dir.path().join("notes.txt"), "some text").unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(dir.path().join("config.json"), "{}").unwrap_or_else(|e| panic!("{e:?}"));

        // When: reading configs
        let configs = read_configs_in(dir.path());

        // Then: only the YAML file is included
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "config.yaml");
    }

    #[test]
    fn test_read_configs_in_sorts_alphabetically() {
        // Given: multiple YAML files in non-alphabetical order on disk
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        for name in &["zebra.yaml", "alpha.yaml", "middle.yml"] {
            fs::write(
                dir.path().join(name),
                "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
            )
            .unwrap_or_else(|e| panic!("{e:?}"));
        }

        // When: reading configs
        let configs = read_configs_in(dir.path());

        // Then: results are sorted alphabetically by name
        assert_eq!(configs.len(), 3);
        assert_eq!(configs[0].name, "alpha.yaml");
        assert_eq!(configs[1].name, "middle.yml");
        assert_eq!(configs[2].name, "zebra.yaml");
    }

    // ---- collect_local_configs ----

    #[test]
    fn test_collect_local_configs_includes_cruise_yaml_at_root() {
        // Given: base_dir contains cruise.yaml
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            dir.path().join("cruise.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: collecting local configs
        let configs = collect_local_configs(dir.path());

        // Then: cruise.yaml is included
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "cruise.yaml");
    }

    #[test]
    fn test_collect_local_configs_includes_cruise_yml_at_root() {
        // Given: base_dir contains cruise.yml (alternative extension)
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            dir.path().join("cruise.yml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: collecting local configs
        let configs = collect_local_configs(dir.path());

        // Then: cruise.yml is included
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "cruise.yml");
    }

    #[test]
    fn test_collect_local_configs_includes_files_in_dot_cruise_subdir() {
        // Given: base_dir has .cruise/foo.yaml
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = dir.path().join(".cruise");
        fs::create_dir(&cruise_dir).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            cruise_dir.join("foo.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: collecting local configs
        let configs = collect_local_configs(dir.path());

        // Then: foo.yaml from .cruise/ is included
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "foo.yaml");
    }

    #[test]
    fn test_collect_local_configs_ordering_root_file_before_cruise_subdir() {
        // Given: base_dir has both cruise.yaml at root and .cruise/bar.yaml
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            dir.path().join("cruise.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = dir.path().join(".cruise");
        fs::create_dir(&cruise_dir).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            cruise_dir.join("bar.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: collecting local configs
        let configs = collect_local_configs(dir.path());

        // Then: root cruise.yaml comes before .cruise/bar.yaml
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].name, "cruise.yaml");
        assert_eq!(configs[1].name, "bar.yaml");
    }

    #[test]
    fn test_collect_local_configs_cruise_subdir_files_sorted_alphabetically() {
        // Given: .cruise/ contains multiple files in non-alphabetical order
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_dir = dir.path().join(".cruise");
        fs::create_dir(&cruise_dir).unwrap_or_else(|e| panic!("{e:?}"));
        for name in &["zzz.yaml", "aaa.yaml", "mmm.yml"] {
            fs::write(
                cruise_dir.join(name),
                "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
            )
            .unwrap_or_else(|e| panic!("{e:?}"));
        }

        // When: collecting local configs
        let configs = collect_local_configs(dir.path());

        // Then: files inside .cruise/ are sorted alphabetically
        assert_eq!(configs.len(), 3);
        assert_eq!(configs[0].name, "aaa.yaml");
        assert_eq!(configs[1].name, "mmm.yml");
        assert_eq!(configs[2].name, "zzz.yaml");
    }

    #[test]
    fn test_collect_local_configs_empty_base_dir_returns_empty() {
        // Given: base_dir has no relevant files
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));

        // When: collecting local configs
        let configs = collect_local_configs(dir.path());

        // Then: empty result
        assert!(configs.is_empty());
    }

    // ---- collect_configs_for_gui ----

    #[test]
    fn test_collect_configs_for_gui_local_configs_come_before_user_dir() {
        // Given: base_dir has cruise.yaml, user_dir has user.yaml
        let base = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let user = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            base.path().join("cruise.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            user.path().join("user.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: collecting all configs (non-repo mode)
        let configs = collect_configs_for_gui(Some(base.path()), false, user.path());

        // Then: local cruise.yaml comes first, then user.yaml
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].name, "cruise.yaml");
        assert_eq!(configs[1].name, "user.yaml");
    }

    #[test]
    fn test_collect_configs_for_gui_repo_mode_skips_local_configs() {
        // Given: base_dir has cruise.yaml, user_dir has user.yaml, is_repo_mode=true
        let base = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let user = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            base.path().join("cruise.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            user.path().join("user.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: collecting configs in repo mode
        let configs = collect_configs_for_gui(Some(base.path()), true, user.path());

        // Then: only user.yaml is returned (no local configs)
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "user.yaml");
    }

    #[test]
    fn test_collect_configs_for_gui_no_base_dir_returns_only_user_dir() {
        // Given: no base_dir, user_dir has user.yaml
        let user = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            user.path().join("user.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: collecting configs without a base_dir
        let configs = collect_configs_for_gui(None, false, user.path());

        // Then: only user.yaml is returned
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "user.yaml");
    }

    #[test]
    fn test_collect_configs_for_gui_deduplicates_same_absolute_path() {
        // Given: base_dir and user_dir are the same directory, containing cruise.yaml
        let shared = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            shared.path().join("cruise.yaml"),
            "command: [claude, -p]\nsteps:\n  s1:\n    command: echo\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        // When: base_dir == user_dir (simulates symlinked user config in project)
        let configs = collect_configs_for_gui(Some(shared.path()), false, shared.path());

        // Then: cruise.yaml appears only once despite being reachable from both paths
        let count = configs.iter().filter(|c| c.name == "cruise.yaml").count();
        let names: Vec<_> = configs.iter().map(|c| &c.name).collect();
        assert_eq!(count, 1, "duplicate found: {names:?}");
    }

    // --- respond_to_ask_impl --------------------------------------------------

    /// Helper: temp SessionManager with a session in AwaitingInput + pending question.
    fn setup_awaiting_input_session(session_id: &str) -> (TempDir, SessionManager) {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = cruise::session::SessionState::new(
            session_id.to_string(),
            repo,
            "cruise.yaml".to_string(),
            "test task".to_string(),
        );
        session.phase = SessionPhase::AwaitingInput;
        session.pending_ask_question = Some("Which DB?".into());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        (tmp, manager)
    }

    #[test]
    fn test_respond_to_ask_impl_delivers_answer() {
        // Given: session in AwaitingInput with a registered ask slot and pending sender
        let (_tmp, _manager) = setup_awaiting_input_session("20260618010000");
        let state = AppState::new();
        let responder = state.register_ask_responder("20260618010000");
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        *responder.lock().unwrap_or_else(|e| panic!("{e}")) = Some(tx);

        // When: respond_to_ask_impl delivers the answer
        let result = respond_to_ask_impl(&state, "20260618010000", "Postgres".to_string());

        // Then: the call succeeds and the answer arrives at the waiting agent
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        let received = rx
            .try_recv()
            .unwrap_or_else(|e| panic!("answer not delivered: {e}"));
        assert_eq!(received, "Postgres");
        // Note: clearing pending_ask_question is GuiAskHandler::ask_user's responsibility,
        // tested separately in gui_ask_handler tests.
    }

    #[test]
    fn test_respond_to_ask_impl_returns_error_when_no_pending_sender() {
        // Given: session in AwaitingInput, responder slot registered but no sender installed
        let (_tmp, _manager) = setup_awaiting_input_session("20260618010001");
        let state = AppState::new();
        state.register_ask_responder("20260618010001"); // slot empty — no sender installed

        // When: respond_to_ask_impl is called without a waiting agent
        let result = respond_to_ask_impl(&state, "20260618010001", "Postgres".to_string());

        // Then: returns an error
        assert!(
            result.is_err(),
            "expected Err when no pending sender, got: {result:?}"
        );
    }
}
