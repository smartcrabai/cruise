use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use cruise::new_session_history::{
    NewSessionHistory, NewSessionHistoryEntry, expand_tilde, resolved_config_key_for_session,
};
use cruise::session::{
    PLAN_VAR, SessionLogger, SessionManager, SessionPhase, SessionState, WorkspaceMode,
    current_iso8601, get_cruise_home,
};
use cruise::step::option::OptionResult;
use cruise::workspace::{prepare_execution_workspace, update_session_workspace};
use serde::{Deserialize, Serialize};

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

    // Preflight: verify `gh` is available before doing any workspace setup so
    // the session is never left in Running phase when `gh` is absent.
    if effective_workspace_mode == WorkspaceMode::Worktree {
        cruise::worktree_pr::ensure_gh_available()?;
    }

    session.workspace_mode = effective_workspace_mode;
    let execution_workspace =
        prepare_execution_workspace(manager, session, effective_workspace_mode)?;
    update_session_workspace(session, &execution_workspace);
    session.phase = SessionPhase::Running;
    manager.save(session)?;

    Ok(execution_workspace.path().to_path_buf())
}

// ─── Filesystem commands ───────────────────────────────────────────────────────

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
    pub default_skipped_steps: Vec<String>,
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
    skipped_steps: Vec<String>,
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
    session.skipped_steps = skipped_steps;
    manager.create(&session).map_err(|e| e.to_string())?;

    let mut history = NewSessionHistory::load_best_effort();
    history.record_selection(NewSessionHistoryEntry {
        selected_at: current_iso8601(),
        requested_config_path: config_path,
        working_dir: base.to_string_lossy().into_owned(),
        resolved_config_key: resolved_config_key_for_session(source.path()),
        skipped_steps: session.skipped_steps.clone(),
    });
    history.save_best_effort();

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
) -> std::result::Result<NewSessionConfigDefaultsDto, String> {
    let (_, yaml, source) = resolve_gui_session_paths(&base_dir, config_path.as_deref())?;
    let config = cruise::config::WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| format!("Failed to parse config: {e}"))?;
    cruise::config::validate_config(&config)
        .map_err(|e| format!("Failed to validate config: {e}"))?;
    let steps = cruise::workflow::list_skippable_steps(&config)
        .map_err(|e| format!("Failed to list skippable steps: {e}"))?;
    let resolved_config_key = resolved_config_key_for_session(source.path());
    let history = NewSessionHistory::load_best_effort();
    let default_skipped_steps = history
        .latest_entry_for_config(&resolved_config_key)
        .map(|entry| entry.skipped_steps.clone())
        .unwrap_or_default();
    Ok(NewSessionConfigDefaultsDto {
        steps,
        default_skipped_steps,
    })
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

    let (option_responder, cancel_token) = state.register_session(sid_for_pr.clone());
    let sessions_dir = manager.sessions_dir();
    let plan_path = session.plan_path(&sessions_dir);
    let input = session.input.clone();
    let skipped_steps = session.skipped_steps.clone();
    let token_for_task = cancel_token.clone();
    let channel_for_step = channel.clone();
    let channel_for_emitter = channel.clone();
    let sid_for_cleanup = sid_for_pr.clone();
    let log_path = manager.run_log_path(session_id);

    let join_result = tokio::task::spawn_blocking(
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
                    session_id: sid_for_pr.clone(),
                    step: step.to_string(),
                });
                Ok(())
            };

            let emitter = Arc::new(StateSavingEmitter::new(
                channel_for_emitter,
                sid_for_pr.clone(),
            ));
            let handler = GuiOptionHandler::new(emitter, sid_for_pr.clone(), option_responder);

            let mut vars = VariableStore::new(input);
            vars.set_named_file(PLAN_VAR, plan_path);
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
                skipped_steps: &skipped_steps,
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

            let exec = result?;
            if let Some(ref ctx) = worktree_ctx_for_pr {
                let manager_for_pr =
                    new_session_manager().map_err(cruise::error::CruiseError::Other)?;
                let mut session_for_pr = manager_for_pr
                    .load(&sid_for_pr)
                    .map_err(|e| cruise::error::CruiseError::Other(e.to_string()))?;
                let pr_result = handle.block_on(cruise::worktree_pr::handle_worktree_pr(
                    ctx,
                    &compiled,
                    &mut vars,
                    &mut tracker,
                    &mut session_for_pr,
                    5,
                    10,
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
            final_session.phase = SessionPhase::Completed;
            final_session.completed_at = Some(current_iso8601());
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
            final_session.phase = SessionPhase::Suspended;
            let _ = channel.send(WorkflowEvent::WorkflowCancelled {
                session_id: sid_for_cleanup.clone(),
            });
            manager.save(&final_session).map_err(|e| e.to_string())?;
            Ok(SessionPhase::Suspended)
        }
        Err(e) => {
            let msg = e.to_string();
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
    let mut cancelled = 0usize;
    let mut seen: HashSet<String> = HashSet::new();
    let mut remaining = manager
        .run_all_remaining(&seen)
        .map_err(|e| e.to_string())?;
    let parallelism = cruise::app_config::AppConfig::load()
        .map_err(|e| e.to_string())?
        .run_all_parallelism;
    let _ = channel.send(WorkflowEvent::RunAllStarted {
        total: remaining.len(),
        parallelism,
    });

    loop {
        let remaining_count = remaining.len();
        let Some(session) = remaining.into_iter().next() else {
            break;
        };
        seen.insert(session.id.clone());

        let session_id = session.id;
        let input = session.input;
        let workspace_mode = session.workspace_mode;
        let total = seen.len() + remaining_count - 1;
        let _ = channel.send(WorkflowEvent::RunAllSessionStarted {
            session_id: session_id.clone(),
            input: input.clone(),
            total,
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
        remaining = manager
            .run_all_remaining(&seen)
            .map_err(|e| e.to_string())?;
    }

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
/// Validates the config before writing (e.g. `run_all_parallelism` must be ≥ 1).
#[tauri::command]
pub fn update_app_config(config: cruise::app_config::AppConfig) -> std::result::Result<(), String> {
    config.save().map_err(|e| e.to_string())
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
    use cruise::new_session_history::{
        BUILTIN_CONFIG_KEY, NewSessionHistory, NewSessionHistoryEntry,
    };
    use cruise::test_support::{init_git_repo, make_session};
    use std::fs;
    use std::path::Path;
    use std::sync::Mutex;
    use tempfile::TempDir;
    use tokio::sync::oneshot;

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
    fn test_get_new_session_history_summary_prefers_latest_gui_entry_even_when_auto() {
        let _lock = cruise::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _home_guards = cruise::test_support::set_fake_home(tmp.path());

        let mut history = NewSessionHistory::default();
        history.record_selection(NewSessionHistoryEntry {
            selected_at: String::new(),
            requested_config_path: Some("/Users/takumi/.cruise/team.yaml".to_string()),
            working_dir: "/Users/takumi/projects/demo".to_string(),
            resolved_config_key: "/Users/takumi/.cruise/team.yaml".to_string(),
            skipped_steps: vec!["review".to_string()],
        });
        history.record_selection(NewSessionHistoryEntry {
            selected_at: String::new(),
            requested_config_path: None,
            working_dir: "/Users/takumi/projects/another-repo".to_string(),
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
    //   test thread: extracts sender from slot and sends OptionResult
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

    #[test]
    fn test_get_config_steps_uses_local_config_from_base_dir() {
        let repo_dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(
            repo_dir.path().join("cruise.yaml"),
            r#"
command: [echo]
groups:
  review:
    steps:
      simplify:
        command: echo simplify
steps:
  build:
    command: echo build
  review-pass:
    group: review
"#,
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let fake_home = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _lock = cruise::test_support::lock_process();
        let _home_guard = cruise::test_support::EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _env_guard = cruise::test_support::EnvGuard::remove("CRUISE_CONFIG");

        let steps = get_config_steps(
            repo_dir
                .path()
                .to_str()
                .unwrap_or_else(|| panic!("unexpected None"))
                .to_string(),
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].id, "build");
        assert_eq!(steps[1].id, "review-pass");
        assert_eq!(steps[1].expanded_step_ids, vec!["review-pass/simplify"]);
    }

    #[test]
    fn test_get_config_steps_returns_builtin_default_steps_when_no_config_found() {
        let repo_dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let fake_home = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let _lock = cruise::test_support::lock_process();
        let _home_guard = cruise::test_support::EnvGuard::set("HOME", fake_home.path().as_os_str());
        let _env_guard = cruise::test_support::EnvGuard::remove("CRUISE_CONFIG");

        let steps = get_config_steps(
            repo_dir
                .path()
                .to_str()
                .unwrap_or_else(|| panic!("unexpected None"))
                .to_string(),
            None,
        )
        .unwrap_or_else(|e| panic!("{e}"));

        assert!(
            !steps.is_empty(),
            "builtin default config should expose skippable steps"
        );
    }

    // ─── prepare_run_session: worktree gh preflight ──────────────────────────

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
}
