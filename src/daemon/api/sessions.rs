use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use crate::daemon::server::WebState;
use crate::session::{PrSkippedReason, SessionPhase, SessionState, WorkspaceMode};
use crate::step::option::OptionResult;

pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}", get(read_session))
        .route("/sessions/{id}/prompts", get(list_prompts))
        .route("/sessions/{id}/prompts/{request_id}", post(answer_prompt))
        .route("/sessions/{id}/cancel", post(cancel_session))
        .route("/sessions/{id}/pause", post(pause_session))
        .route("/run-all/cancel", post(cancel_run_all))
}

#[derive(Deserialize, Serialize)]
pub(crate) struct SessionListItem {
    pub(crate) id: String,
    pub(crate) base_dir: String,
    pub(crate) phase: String,
    pub(crate) phase_error: Option<String>,
    pub(crate) plan_error: Option<String>,
    pub(crate) config_source: String,
    pub(crate) input: String,
    pub(crate) title: Option<String>,
    pub(crate) current_step: Option<String>,
    pub(crate) created_at: String,
    pub(crate) completed_at: Option<String>,
    pub(crate) worktree_path: Option<String>,
    pub(crate) worktree_branch: Option<String>,
    pub(crate) workspace_mode: WorkspaceMode,
    pub(crate) target_branch: Option<String>,
    pub(crate) pr_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pr_target: Option<crate::pr_review_fix::PrTarget>,
    pub(crate) pr_skipped_reason: Option<PrSkippedReason>,
    pub(crate) config_path: Option<String>,
    pub(crate) updated_at: Option<String>,
    pub(crate) awaiting_input: bool,
    pub(crate) plan_available: bool,
    pub(crate) pending_instructions: usize,
    pub(crate) auto_requeue_count: u32,
}

fn list_item(state: &WebState, session: SessionState) -> SessionListItem {
    let (phase, phase_error) = match &session.phase {
        SessionPhase::Draft => ("Draft", None),
        SessionPhase::AwaitingInput => ("AwaitingInput", None),
        SessionPhase::AwaitingApproval => ("AwaitingApproval", None),
        SessionPhase::Planned => ("Planned", None),
        SessionPhase::Running => ("Running", None),
        SessionPhase::Completed => ("Completed", None),
        SessionPhase::Failed(message) => ("Failed", Some(message.clone())),
        SessionPhase::PrFailed(message) => ("PrFailed", Some(message.clone())),
        SessionPhase::Suspended => ("Suspended", None),
    };
    let plan_available = state.application.session_plan_available(&session);
    let pending_instructions = state
        .application
        .pending_summary(&session.id)
        .map_or(0, |summary| summary.pending.len());
    SessionListItem {
        phase: phase.to_owned(),
        id: session.id,
        base_dir: session.base_dir.to_string_lossy().into_owned(),
        phase_error,
        plan_error: session.plan_error,
        config_source: session.config.display_label(),
        input: session.input,
        title: session.title,
        current_step: session.current_step,
        created_at: session.created_at,
        completed_at: session.completed_at,
        worktree_path: session
            .worktree_path
            .map(|path| path.to_string_lossy().into_owned()),
        worktree_branch: session.worktree_branch,
        workspace_mode: session.workspace_mode,
        target_branch: session.target_branch,
        pr_url: session.pr_url,
        pr_target: session.pr_target,
        pr_skipped_reason: session.pr_skipped_reason,
        config_path: session.config.selection_value(),
        updated_at: session.updated_at,
        awaiting_input: session.awaiting_input,
        plan_available,
        pending_instructions,
        auto_requeue_count: session.auto_requeue_count,
    }
}

async fn list_sessions(State(state): State<WebState>) -> Response {
    match state.application.reconciled_sessions() {
        Ok(sessions) => axum::Json(
            sessions
                .into_iter()
                .map(|session| list_item(&state, session))
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(error) => super::map_error(&error),
    }
}

async fn read_session(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    match state.application.reconcile_session(&id) {
        Ok(session) => {
            let active = state.application.runtime().active_operation(&id).is_some();
            let capabilities = state.application.capabilities(&session);
            match serde_json::to_value(session) {
                Ok(mut value) => {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("activeOperation".to_owned(), active.into());
                        object.insert("capabilities".to_owned(), serde_json::json!(capabilities));
                        axum::Json(value).into_response()
                    } else {
                        super::error_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "serialized session state is not an object",
                        )
                    }
                }
                Err(error) => {
                    super::error_response(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
                }
            }
        }
        Err(error) => super::map_error(&error),
    }
}

async fn list_prompts(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    if let Err(error) = state.application.read_session(&id) {
        return super::map_error(&error);
    }
    axum::Json(state.application.pending_prompts(&id)).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptAnswer {
    answer: Option<String>,
    option: Option<OptionResult>,
}

async fn answer_prompt(
    Path((id, request_id)): Path<(String, String)>,
    State(state): State<WebState>,
    body: Bytes,
) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    if let Err(error) = state.application.read_session(&id) {
        return super::map_error(&error);
    }
    let request = match serde_json::from_slice::<PromptAnswer>(&body) {
        Ok(request) => request,
        Err(error) => return super::error_response(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let result = match (request.answer, request.option) {
        (Some(answer), None) => state.application.respond_to_ask(&id, &request_id, answer),
        (None, Some(option)) => state
            .application
            .respond_to_option(&id, &request_id, option),
        _ => {
            return super::error_response(
                StatusCode::BAD_REQUEST,
                "provide exactly one of answer or option",
            );
        }
    };
    match result {
        Ok(()) => axum::Json(serde_json::json!({})).into_response(),
        Err(error) => super::map_error(&error),
    }
}

async fn cancel_session(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    if let Err(error) = state.application.read_session(&id) {
        return super::map_error(&error);
    }
    axum::Json(serde_json::json!({
        "cancelled": state.application.cancel_session(&id)
    }))
    .into_response()
}

async fn pause_session(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    if let Err(error) = state.application.read_session(&id) {
        return super::map_error(&error);
    }
    axum::Json(serde_json::json!({
        "paused": state.application.pause_session(&id)
    }))
    .into_response()
}

async fn cancel_run_all(State(state): State<WebState>) -> Response {
    axum::Json(serde_json::json!({
        "cancelled": state.application.cancel_run_all()
    }))
    .into_response()
}
