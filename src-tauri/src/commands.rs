use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cruise::cancellation::CancellationToken;
use cruise::session::{
    SessionLogger, SessionManager, SessionPhase, SessionState, WorkspaceMode, current_iso8601,
    get_cruise_home,
};
use cruise::step::option::OptionResult;
use cruise::workspace::{prepare_execution_workspace, update_session_workspace};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::events::{PlanEvent, WorkflowEvent};
use crate::gui_option_handler::GuiOptionHandler;
use crate::state::AppState;

// ─── DTOs ─────────────────────────────────────────────────────────────────────

/// Serializable representation of a session, sent to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub id: String,
    pub phase: String,
    /// Error message when `phase == "Failed"`.
    pub phase_error: Option<String>,
    pub config_source: String,
    pub base_dir: String,
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
        let plan_available = std::fs::read_to_string(&plan_path)
            .map(|c| !c.trim().is_empty())
            .unwrap_or(false);
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
            base_dir: s.base_dir.to_string_lossy().into_owned(),
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
    /// `"translocated"` | `"mountedVolume"` | `"unknownBundlePath"` — set when `can_auto_update` is false.
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

// ─── StateSavingEmitter ────────────────────────────────────────────────────────

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

// ─── Helpers ───────────────────────────────────────────────────────────────────

fn new_session_manager() -> std::result::Result<SessionManager, String> {
    let cruise_home = get_cruise_home().map_err(|e| e.to_string())?;
    Ok(SessionManager::new(cruise_home))
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

    session.workspace_mode = effective_workspace_mode;
    let execution_workspace =
        prepare_execution_workspace(manager, session, effective_workspace_mode)?;
    update_session_workspace(session, &execution_workspace);
    session.phase = SessionPhase::Running;
    manager.save(session)?;

    Ok(execution_workspace.path().to_path_buf())
}

// ─── Filesystem commands ───────────────────────────────────────────────────────

/// Expand a leading `~` to the home directory. Returns the path unchanged if it does
/// not start with `~`.
fn expand_tilde(path: &str) -> String {
    if path.starts_with('~') {
        let home = home::home_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("{}{}", home, &path[1..])
    } else {
        path.to_string()
    }
}

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

// ─── Read commands ─────────────────────────────────────────────────────────────

/// List all sessions, sorted oldest-first.
#[tauri::command]
pub fn list_sessions() -> std::result::Result<Vec<SessionDto>, String> {
    let manager = new_session_manager()?;
    manager
        .list()
        .map(|sessions| {
            sessions
                .into_iter()
                .map(|s| SessionDto::from_state(s, &manager))
                .collect()
        })
        .map_err(|e| e.to_string())
}

/// Get a single session by ID.
#[tauri::command]
pub fn get_session(session_id: String) -> std::result::Result<SessionDto, String> {
    let manager = new_session_manager()?;
    manager
        .load(&session_id)
        .map(|s| SessionDto::from_state(s, &manager))
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

// ─── Write commands ────────────────────────────────────────────────────────────

/// Cancel the currently running workflow session.
#[tauri::command]
pub fn cancel_session(state: tauri::State<'_, AppState>) -> std::result::Result<(), String> {
    do_cancel_session(&state.cancel_token, &state.option_responder)
}

/// Deliver the frontend's option-step response to the engine.
#[tauri::command]
pub fn respond_to_option(
    result: OptionResultDto,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    // Clear awaiting_input on the active session before unblocking the engine.
    let session_id = {
        let guard = state
            .active_session_id
            .lock()
            .map_err(|e| format!("lock poisoned: {e}"))?;
        guard.clone()
    };
    if let Some(id) = session_id {
        if let Ok(manager) = new_session_manager() {
            if let Ok(mut s) = manager.load(&id) {
                s.awaiting_input = false;
                let _ = manager.save(&s);
            }
        }
    }

    let option_result = OptionResult {
        next_step: result.next_step,
        text_input: result.text_input,
    };
    do_respond_to_option(&state.option_responder, option_result)
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

// ─── Plan generation helpers ───────────────────────────────────────────────────

/// Plan generation prompt templates, embedded at compile-time.
const PLAN_PROMPT_TEMPLATE: &str = include_str!("../../prompts/plan.md");
const FIX_PLAN_PROMPT_TEMPLATE: &str = include_str!("../../prompts/fix-plan.md");
const ASK_PLAN_PROMPT_TEMPLATE: &str = include_str!("../../prompts/ask-plan.md");
const PLAN_VAR: &str = "plan";

/// Invoke the LLM to generate/fix a plan using `template`, writing output to the
/// path stored in `vars` under the `"plan"` variable.
async fn run_plan_prompt_template(
    config: &cruise::config::WorkflowConfig,
    vars: &mut cruise::variable::VariableStore,
    template: &str,
    rate_limit_retries: usize,
    cwd: Option<&std::path::Path>,
) -> std::result::Result<cruise::step::prompt::PromptResult, String> {
    let plan_model = config.plan_model.clone().or_else(|| config.model.clone());
    let prompt = vars
        .resolve(template)
        .map_err(|e: cruise::error::CruiseError| e.to_string())?;
    let effective_model = plan_model.as_deref();
    let has_placeholder = config.command.iter().any(|s| s.contains("{model}"));
    let (resolved_command, model_arg) = if has_placeholder {
        (
            cruise::engine::resolve_command_with_model(&config.command, effective_model),
            None,
        )
    } else {
        (config.command.clone(), effective_model.map(str::to_string))
    };
    cruise::step::prompt::run_prompt(
        &resolved_command,
        model_arg.as_deref(),
        &prompt,
        rate_limit_retries,
        &std::collections::HashMap::new(),
        None::<&fn(&str)>,
        None,
        cwd,
    )
    .await
    .map_err(|e| e.to_string())
}

// ─── Session creation commands ─────────────────────────────────────────────────

/// A discovered workflow config file, returned to the frontend.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEntryDto {
    pub path: String,
    pub name: String,
}

/// List available workflow config files in `~/.cruise/` (excluding sessions/ and worktrees/).
#[tauri::command]
pub fn list_configs() -> std::result::Result<Vec<ConfigEntryDto>, String> {
    let cruise_home = get_cruise_home().map_err(|e| e.to_string())?;
    let Ok(entries) = std::fs::read_dir(&cruise_home) else {
        return Ok(vec![]);
    };
    let mut configs: Vec<ConfigEntryDto> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            if p.is_dir() {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "sessions" || name == "worktrees" {
                    return false;
                }
            }
            p.is_file() && matches!(p.extension().and_then(|e| e.to_str()), Some("yaml" | "yml"))
        })
        .map(|p| ConfigEntryDto {
            name: p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            path: p.to_string_lossy().into_owned(),
        })
        .collect();
    configs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(configs)
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
    channel: tauri::ipc::Channel<PlanEvent>,
) -> std::result::Result<String, String> {
    use cruise::config::{WorkflowConfig, validate_config};
    use cruise::session::{SessionManager, SessionState};
    use cruise::variable::VariableStore;

    let (base, yaml, source) = resolve_gui_session_paths(&base_dir, config_path.as_deref())?;
    let config =
        WorkflowConfig::from_yaml(&yaml).map_err(|e| format!("config parse error: {e}"))?;
    validate_config(&config).map_err(|e| e.to_string())?;

    let manager = new_session_manager()?;
    let session_id = SessionManager::new_session_id();
    let mut session = SessionState::new(
        session_id.clone(),
        base.clone(),
        source.display_string(),
        input.trim().to_string(),
    );
    session.config_path = source.path().cloned();
    manager.create(&session).map_err(|e| e.to_string())?;
    let _ = channel.send(PlanEvent::SessionCreated {
        session_id: session_id.clone(),
    });

    let session_dir = manager.sessions_dir().join(&session_id);
    if session.config_path.is_none() {
        std::fs::write(session_dir.join("config.yaml"), &yaml)
            .map_err(|e| format!("failed to write session config: {e}"))?;
    }

    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path.clone());

    let _ = channel.send(PlanEvent::PlanGenerating);

    match run_plan_prompt_template(&config, &mut vars, PLAN_PROMPT_TEMPLATE, 5, Some(&base)).await {
        Ok(result) => {
            let content = match cruise::metadata::resolve_plan_content(
                &plan_path,
                &result.output,
                &result.stderr,
            ) {
                Ok(c) => c,
                Err(e) => {
                    let _ = manager.delete(&session_id);
                    let msg = e.to_string();
                    let _ = channel.send(PlanEvent::PlanFailed {
                        session_id: session_id.clone(),
                        error: msg.clone(),
                    });
                    return Err(msg);
                }
            };
            let _ = channel.send(PlanEvent::PlanGenerated {
                session_id: session_id.clone(),
                content: content.clone(),
            });
            Ok(session_id)
        }
        Err(msg) => {
            let _ = manager.delete(&session_id);
            let _ = channel.send(PlanEvent::PlanFailed {
                session_id: session_id.clone(),
                error: msg.clone(),
            });
            Err(msg)
        }
    }
}

/// Approve a session, transitioning it from "Awaiting Approval" to "Planned".
#[tauri::command]
pub fn approve_session(session_id: String) -> std::result::Result<(), String> {
    let manager = new_session_manager()?;
    let mut session = manager.load(&session_id).map_err(|e| e.to_string())?;
    if let Err(err) = cruise::metadata::refresh_session_title_from_session(&manager, &mut session) {
        eprintln!("warning: failed to refresh session title: {err}");
    }
    session.approve();
    manager.save(&session).map_err(|e| e.to_string())?;
    Ok(())
}

/// Reset a session to "Planned" phase regardless of its current phase.
#[tauri::command]
pub fn reset_session(session_id: String) -> std::result::Result<SessionDto, String> {
    let manager = new_session_manager()?;
    let mut session = manager.load(&session_id).map_err(|e| e.to_string())?;
    session.reset_to_planned();
    manager.save(&session).map_err(|e| e.to_string())?;
    Ok(SessionDto::from_state(session, &manager))
}

/// Delete a session that is still in "Awaiting Approval" phase (discard).
#[tauri::command]
pub fn discard_session(session_id: String) -> std::result::Result<(), String> {
    let manager = new_session_manager()?;
    manager.delete(&session_id).map_err(|e| e.to_string())?;
    Ok(())
}

/// Delete a session and clean up its git worktree if one exists.
///
/// Running sessions cannot be deleted — cancel them first.
#[tauri::command]
pub fn delete_session(session_id: String) -> std::result::Result<(), String> {
    let manager = new_session_manager()?;
    let session = manager.load(&session_id).map_err(|e| e.to_string())?;

    if matches!(session.phase, SessionPhase::Running) {
        return Err("Cannot delete a running session. Cancel it first.".to_string());
    }

    if let Some(ctx) = session.worktree_context()
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
) -> std::result::Result<String, String> {
    let manager = new_session_manager()?;
    let mut session = manager.load(&session_id).map_err(|e| e.to_string())?;

    let _ = channel.send(PlanEvent::PlanGenerating);

    let config = manager.load_config(&session).map_err(|e| e.to_string())?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = cruise::variable::VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path.clone());
    vars.set_prev_input(Some(feedback));

    match run_plan_prompt_template(
        &config,
        &mut vars,
        FIX_PLAN_PROMPT_TEMPLATE,
        5,
        Some(&session.base_dir),
    )
    .await
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
                    let _ = channel.send(PlanEvent::PlanFailed {
                        session_id: session_id.clone(),
                        error: msg.clone(),
                    });
                    return Err(msg);
                }
            };
            cruise::metadata::refresh_session_title_from_plan(&mut session, &content);
            // Re-save to update updated_at timestamp
            manager.save(&session).map_err(|e| e.to_string())?;

            let _ = channel.send(PlanEvent::PlanGenerated {
                session_id: session_id.clone(),
                content: content.clone(),
            });
            Ok(content)
        }
        Err(msg) => {
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
    let session = manager.load(session_id).map_err(|e| e.to_string())?;
    let config = manager.load_config(&session).map_err(|e| e.to_string())?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = cruise::variable::VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path);
    vars.set_prev_input(Some(question));

    let result = run_plan_prompt_template(
        &config,
        &mut vars,
        ASK_PLAN_PROMPT_TEMPLATE,
        5,
        Some(&session.base_dir),
    )
    .await
    .map_err(|e| e.to_string())?;

    // Return raw output only — do NOT call resolve_plan_content(), which would
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

// ─── run_session / run_all_sessions ────────────────────────────────────────────

/// Core session execution logic shared by [`run_session`] and [`run_all_sessions`].
///
/// Loads the session, runs the workflow on a dedicated blocking thread, saves the
/// final phase, and emits the terminal [`WorkflowEvent`].  Returns the final
/// [`SessionPhase`] so callers can decide how to proceed (e.g. break a batch loop
/// on `Suspended`).
///
/// Infrastructure errors (mutex poisoned, session not found, …) are returned as
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
) -> std::result::Result<SessionPhase, String> {
    let mut session = manager.load(session_id).map_err(|e| e.to_string())?;

    if !session.phase.is_runnable() {
        return Err(format!(
            "Session {} is in '{}' phase and cannot be run",
            session_id,
            session.phase.label()
        ));
    }

    let config = manager.load_config(&session).map_err(|e| e.to_string())?;
    let compiled = cruise::workflow::compile(config).map_err(|e| e.to_string())?;

    let start_step = session.current_step.clone().map_or_else(
        || {
            compiled
                .steps
                .keys()
                .next()
                .ok_or_else(|| "config has no steps".to_string())
                .map(Clone::clone)
        },
        Ok,
    )?;

    let exec_root =
        prepare_run_session(&manager, &mut session, workspace_mode).map_err(|e| e.to_string())?;

    let cancel_token = CancellationToken::new();
    {
        let mut guard = state
            .cancel_token
            .lock()
            .map_err(|e| format!("lock poisoned: {e}"))?;
        *guard = Some(cancel_token.clone());
    }
    {
        let mut guard = state
            .active_session_id
            .lock()
            .map_err(|e| format!("lock poisoned: {e}"))?;
        *guard = Some(session_id.to_string());
    }

    let option_responder = Arc::clone(&state.option_responder);
    let sessions_dir = manager.sessions_dir();
    let plan_path = session.plan_path(&sessions_dir);
    let input = session.input.clone();
    let token_for_task = cancel_token.clone();
    let channel_for_step = channel.clone();
    let channel_for_emitter = channel.clone();
    let sid_for_spawn = session_id.to_string();
    let sid_for_emitter = session_id.to_string();
    let log_path = manager.run_log_path(session_id);

    let exec_result = tokio::task::spawn_blocking(
        move || -> cruise::error::Result<cruise::engine::ExecutionResult> {
            use cruise::engine::{ExecutionContext, execute_steps};
            use cruise::file_tracker::FileTracker;
            use cruise::variable::VariableStore;

            let logger = SessionLogger::new(log_path);
            logger.write("--- run started ---");

            // Temporarily change the working directory for command steps.
            let original_dir = std::env::current_dir().ok();
            std::env::set_current_dir(&exec_root).map_err(|e| {
                cruise::error::CruiseError::Other(format!("failed to set working dir: {e}"))
            })?;

            let on_step_start = |step: &str| -> cruise::error::Result<()> {
                logger.write(step);
                let _ = channel_for_step.send(WorkflowEvent::StepStarted {
                    step: step.to_string(),
                });
                Ok(())
            };

            let emitter = Arc::new(StateSavingEmitter::new(
                channel_for_emitter,
                sid_for_emitter,
            ));
            let handler = GuiOptionHandler::new(emitter, sid_for_spawn, option_responder);

            let mut vars = VariableStore::new(input);
            vars.set_named_file("plan", plan_path);
            let exec_root_path = exec_root.clone();
            let mut tracker = FileTracker::with_root(exec_root);

            let ctx = ExecutionContext {
                compiled: &compiled,
                max_retries: 10,
                rate_limit_retries: 5,
                on_step_start: &on_step_start,
                cancel_token: Some(&token_for_task),
                option_handler: &handler,
                config_reloader: None,
                working_dir: Some(&exec_root_path),
            };

            let handle = tokio::runtime::Handle::current();
            let result = handle.block_on(execute_steps(&ctx, &mut vars, &mut tracker, &start_step));

            match &result {
                Ok(exec) => logger.write(&format!(
                    "✓ completed — run: {}, skipped: {}, failed: {}",
                    exec.run, exec.skipped, exec.failed
                )),
                Err(cruise::error::CruiseError::Interrupted) => {
                    logger.write("⏸ cancelled");
                }
                Err(e) => logger.write(&format!("✗ failed: {}", e.detailed_message())),
            }

            if let Some(dir) = original_dir {
                let _ = std::env::set_current_dir(dir);
            }

            result
        },
    )
    .await
    .map_err(|e| format!("execution task panicked: {e}"))?;

    // Clear the cancel token and active session slots.
    {
        let mut guard = state
            .cancel_token
            .lock()
            .map_err(|e| format!("lock poisoned: {e}"))?;
        *guard = None;
    }
    {
        let mut guard = state
            .active_session_id
            .lock()
            .map_err(|e| format!("lock poisoned: {e}"))?;
        *guard = None;
    }

    // Reload session to pick up any intermediate saves, then apply the final phase.
    let mut final_session = manager.load(session_id).unwrap_or(session);
    final_session.awaiting_input = false;

    match exec_result {
        Ok(exec) => {
            final_session.phase = SessionPhase::Completed;
            final_session.completed_at = Some(current_iso8601());
            let _ = channel.send(WorkflowEvent::WorkflowCompleted {
                run: exec.run,
                skipped: exec.skipped,
                failed: exec.failed,
            });
            manager.save(&final_session).map_err(|e| e.to_string())?;
            Ok(SessionPhase::Completed)
        }
        Err(cruise::error::CruiseError::Interrupted) => {
            final_session.phase = SessionPhase::Suspended;
            let _ = channel.send(WorkflowEvent::WorkflowCancelled);
            manager.save(&final_session).map_err(|e| e.to_string())?;
            Ok(SessionPhase::Suspended)
        }
        Err(e) => {
            let msg = e.to_string();
            final_session.phase = SessionPhase::Failed(msg.clone());
            final_session.completed_at = Some(current_iso8601());
            let _ = channel.send(WorkflowEvent::WorkflowFailed { error: msg.clone() });
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
    match execute_single_session(&session_id, workspace_mode, &channel, &state, &manager).await? {
        SessionPhase::Failed(msg) => Err(msg),
        _ => Ok(()),
    }
}

/// Execute all Planned / Suspended sessions in series, streaming batch-level
/// [`WorkflowEvent`]s (plus the per-session events from each run) over `channel`.
///
/// Individual session failures are logged and the batch continues.  Only a
/// `Suspended` result (user cancelled) stops the loop early.
#[tauri::command]
pub async fn run_all_sessions(
    channel: tauri::ipc::Channel<WorkflowEvent>,
    state: tauri::State<'_, AppState>,
) -> std::result::Result<(), String> {
    let manager = new_session_manager()?;
    let candidates = manager.run_all_candidates().map_err(|e| e.to_string())?;
    let total = candidates.len();
    let _ = channel.send(WorkflowEvent::RunAllStarted { total });

    let mut cancelled = 0usize;

    for session in candidates {
        let session_id = session.id;
        let input = session.input;
        let workspace_mode = session.workspace_mode;
        let _ = channel.send(WorkflowEvent::RunAllSessionStarted {
            session_id: session_id.clone(),
            input: input.clone(),
        });

        let phase = execute_single_session(&session_id, workspace_mode, &channel, &state, &manager)
            .await
            .unwrap_or_else(SessionPhase::Failed);

        let (error, should_break) = match &phase {
            SessionPhase::Suspended => {
                cancelled += 1;
                (None, true)
            }
            SessionPhase::Failed(msg) => (Some(msg.clone()), false),
            _ => (None, false),
        };

        let _ = channel.send(WorkflowEvent::RunAllSessionFinished {
            session_id,
            input,
            phase: phase.label().to_string(),
            error,
        });

        if should_break {
            break;
        }
    }

    let _ = channel.send(WorkflowEvent::RunAllCompleted { cancelled });

    Ok(())
}

/// Inner logic for the `cancel_session` IPC command.
///
/// Extracted from the Tauri command handler for testability.
/// Calls `cancel()` on the active token if one is present; succeeds silently if not.
/// The token is removed from the slot after cancellation to free the underlying `Arc`.
/// Also drops any pending option-step sender so that `blocking_recv()` in
/// `GuiOptionHandler::select_option` returns immediately with `CruiseError::Interrupted`.
///
/// # Errors
///
/// Returns an error string if either mutex is poisoned.
pub fn do_cancel_session(
    cancel_token: &Mutex<Option<CancellationToken>>,
    option_responder: &Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
) -> std::result::Result<(), String> {
    let mut guard = cancel_token
        .lock()
        .map_err(|e| format!("lock poisoned: {e}"))?;
    if let Some(token) = guard.take() {
        token.cancel();
    }
    // Drop pending option sender so blocking_recv() in select_option unblocks immediately.
    let mut opt_guard = option_responder
        .lock()
        .map_err(|e| format!("lock poisoned: {e}"))?;
    let _ = opt_guard.take();
    Ok(())
}

/// Inner logic for the `respond_to_option` IPC command.
///
/// Extracted from the Tauri command handler for testability.
/// Takes the pending `oneshot::Sender` from `option_responder` and delivers the user's choice.
///
/// # Errors
///
/// Returns an error string if the mutex is poisoned or no option request is currently pending.
pub fn do_respond_to_option(
    option_responder: &Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>,
    result: OptionResult,
) -> std::result::Result<(), String> {
    let mut guard = option_responder
        .lock()
        .map_err(|e| format!("lock poisoned: {e}"))?;
    let sender = guard
        .take()
        .ok_or_else(|| "no pending option request".to_string())?;
    sender
        .send(result)
        .map_err(|_| "option receiver dropped: response not delivered".to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use cruise::cancellation::CancellationToken;
    use cruise::test_support::{init_git_repo, make_session};
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    /// Polls `pending` until a sender is available, or panics after 5 seconds.
    fn wait_for_pending(pending: &Arc<Mutex<Option<oneshot::Sender<OptionResult>>>>) {
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

    // ─── cancel_session ──────────────────────────────────────────────────────

    fn empty_option_responder() -> Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> {
        Arc::new(Mutex::new(None))
    }

    #[test]
    fn test_cancel_session_with_no_active_token_succeeds() {
        // Given: no active cancellation token in state
        let cancel_token: Mutex<Option<CancellationToken>> = Mutex::new(None);
        // When: cancel is requested
        let result = do_cancel_session(&cancel_token, &empty_option_responder());
        // Then: succeeds without error
        assert!(result.is_ok());
    }

    #[test]
    fn test_cancel_session_with_active_token_cancels_it() {
        // Given: an active token stored in state
        let token = CancellationToken::new();
        let token_for_assert = token.clone();
        let cancel_token: Mutex<Option<CancellationToken>> = Mutex::new(Some(token));
        // When: cancel is requested
        let result = do_cancel_session(&cancel_token, &empty_option_responder());
        // Then: succeeds and the token reports cancelled
        assert!(result.is_ok());
        assert!(token_for_assert.is_cancelled());
    }

    #[test]
    fn test_cancel_session_clears_token_from_slot_after_cancelling() {
        // Given: an active token
        let token = CancellationToken::new();
        let cancel_token: Mutex<Option<CancellationToken>> = Mutex::new(Some(token));
        // When: cancel is requested
        let _ = do_cancel_session(&cancel_token, &empty_option_responder());
        // Then: the token slot is cleared (frees the Arc)
        assert!(
            cancel_token
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none()
        );
    }

    #[test]
    fn test_cancel_session_drops_pending_option_sender() {
        // Given: a pending option sender in the responder slot
        let (tx, rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        let cancel_token: Mutex<Option<CancellationToken>> = Mutex::new(None);
        // When: cancel is requested
        let result = do_cancel_session(&cancel_token, &option_responder);
        // Then: succeeds and the receiver observes the sender was dropped
        assert!(result.is_ok());
        assert!(
            rx.blocking_recv().is_err(),
            "sender should have been dropped"
        );
    }

    // ─── respond_to_option ───────────────────────────────────────────────────

    #[test]
    fn test_respond_to_option_with_no_pending_request_returns_error() {
        // Given: no pending option request
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(None));
        // When: respond_to_option is called
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // Then: returns an error mentioning no pending request
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_lowercase()
                .contains("no pending option request"),
            "error message should mention 'no pending option request'"
        );
    }

    #[test]
    fn test_respond_to_option_sends_next_step_to_handler() {
        // Given: a pending option request (sender in state)
        let (tx, rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        // When: respond_to_option is called with a next_step choice
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: Some("next_step".to_string()),
                text_input: None,
            },
        );
        // Then: succeeds and the handler receives the next_step
        assert!(result.is_ok());
        let received = rx.blocking_recv().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(received.next_step, Some("next_step".to_string()));
        assert_eq!(received.text_input, None);
    }

    #[test]
    fn test_respond_to_option_sends_text_input_to_handler() {
        // Given: a pending option request
        let (tx, rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        // When: respond_to_option is called with text input
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: Some("my text input".to_string()),
            },
        );
        // Then: the text is delivered to the handler
        assert!(result.is_ok());
        let received = rx.blocking_recv().unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(received.text_input, Some("my text input".to_string()));
    }

    #[test]
    fn test_respond_to_option_clears_sender_from_state_after_use() {
        // Given: a pending option request
        let (tx, _rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        // When: respond_to_option is called
        let _ = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // Then: the sender slot is cleared (idempotency guard)
        assert!(
            option_responder
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .is_none()
        );
    }

    #[test]
    fn test_respond_to_option_second_call_returns_error() {
        // Given: a request that was already handled
        let (tx, _rx) = oneshot::channel::<OptionResult>();
        let option_responder: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> =
            Arc::new(Mutex::new(Some(tx)));
        let _ = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // When: respond_to_option is called again
        let result = do_respond_to_option(
            &option_responder,
            OptionResult {
                next_step: None,
                text_input: None,
            },
        );
        // Then: returns an error (no pending request remains)
        assert!(result.is_err());
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
        fs::write(repo.join("dirty.txt"), "dirty").unwrap_or_else(|e| panic!("{e:?}"));
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

    // ─── Integration: full option-selection round-trip ────────────────────────
    //
    // Data flow:
    //   GuiOptionHandler::select_option (engine thread)
    //     → stores sender in shared pending_response slot
    //     → emits WorkflowEvent::OptionRequired
    //   do_respond_to_option (IPC command handler / test thread)
    //     → extracts sender from slot
    //     → sends OptionResult
    //   GuiOptionHandler::select_option (engine thread)
    //     → blocking_recv returns OptionResult
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
        let pending: Arc<Mutex<Option<oneshot::Sender<OptionResult>>>> = Arc::new(Mutex::new(None));
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
        do_respond_to_option(
            &pending_for_cmd,
            OptionResult {
                next_step: Some("finalize".to_string()),
                text_input: None,
            },
        )
        .unwrap_or_else(|e| panic!("respond_to_option failed: {e}"));

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

    // ─── check_update_readiness_for_path ─────────────────────────────────────

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

    // ─── do_ask_session ───────────────────────────────────────────────────────

    /// Write a minimal `config.yaml` that uses the given shell command as the LLM.
    ///
    /// The command must read stdin (or ignore it) and write to stdout; it does
    /// not need to be an actual language model.
    fn write_test_config(session_dir: &std::path::Path, shell_command: &str) {
        let yaml = format!("command:\n  - bash\n  - -c\n  - \"{shell_command}\"\nsteps: {{}}\n");
        fs::write(session_dir.join("config.yaml"), yaml).unwrap_or_else(|e| panic!("{e}"));
    }

    /// Create a temporary SessionManager with a session that has `plan.md` and `config.yaml`.
    /// Returns `(TempDir, SessionManager)` — callers must keep `_tmp` alive for the test duration.
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
        // sessions dir doesn't even exist — load will fail immediately

        // When: ask_session is called with a nonexistent ID
        let result =
            do_ask_session(&manager, "nonexistent-session-id", "Question?".to_string()).await;

        // Then: returns an error
        assert!(result.is_err(), "expected Err for missing session, got Ok");
    }

    // ─── resolve_gui_session_paths ───────────────────────────────────────────

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
}
