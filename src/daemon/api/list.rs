use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

use crate::application::{LocalMergeMode, SessionSettingsRequest};
use crate::daemon::server::WebState;
use crate::request_context::RequestContext;

pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/clean", post(clean))
        .route("/sessions/{id}/diff", get(session_diff))
        .route("/sessions/{id}/reset", post(reset_session))
        .route("/sessions/{id}/delete", post(delete_session))
        .route("/sessions/{id}/settings", post(update_settings))
        .route("/sessions/{id}/merge-status", get(inspect_merge))
        .route("/sessions/{id}/merge-pr", post(merge_pr))
        .route("/sessions/{id}/local-merge", post(local_merge))
        .route("/sessions/{id}/create-pr", post(create_pr))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ContextRequest {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsRequest {
    settings: SessionSettingsRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MergePrRequest {
    method: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocalMergeRequest {
    mode: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreatePrRequest {
    context: Option<RequestContext>,
}

#[derive(Deserialize)]
struct DiffQuery {
    stat: Option<String>,
    color: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiffResponse {
    branch: String,
    text: String,
    truncated: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MergeStatusResponse {
    state: String,
    mergeable: String,
    review_decision: String,
    checks: Vec<MergeCheck>,
}

#[derive(Serialize)]
struct MergeCheck {
    name: String,
    status: String,
}

fn invalid_session_id(id: &str) -> Option<Response> {
    crate::session::validate_session_id(id)
        .err()
        .map(|error| super::error_response(StatusCode::BAD_REQUEST, &error.to_string()))
}

async fn clean(
    State(state): State<WebState>,
    axum::Json(_request): axum::Json<ContextRequest>,
) -> Response {
    let application = state.application.clone();
    match tokio::task::spawn_blocking(move || application.clean()).await {
        Ok(Ok(report)) => axum::Json(report).into_response(),
        Ok(Err(error)) => super::map_error(&error),
        Err(error) => super::error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("clean task failed: {error}"),
        ),
    }
}

async fn session_diff(
    Path(id): Path<String>,
    Query(query): Query<DiffQuery>,
    State(state): State<WebState>,
) -> Response {
    if let Some(response) = invalid_session_id(&id) {
        return response;
    }
    let stat = query
        .stat
        .as_deref()
        .is_some_and(|value| matches!(value, "1" | "true"));
    let color = query
        .color
        .as_deref()
        .is_some_and(|value| matches!(value, "1" | "true"));
    match state.application.session_diff(&id, color) {
        Ok(diff) => axum::Json(DiffResponse {
            branch: diff.branch,
            text: if stat { diff.stat } else { diff.patch },
            truncated: diff.truncated,
        })
        .into_response(),
        Err(error) => super::map_error(&error),
    }
}

async fn reset_session(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(_request): axum::Json<ContextRequest>,
) -> Response {
    if let Some(response) = invalid_session_id(&id) {
        return response;
    }
    match state.application.reset_to_planned(&id) {
        Ok(session) => axum::Json(session).into_response(),
        Err(error) => super::map_error(&error),
    }
}

async fn delete_session(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(_request): axum::Json<ContextRequest>,
) -> Response {
    if let Some(response) = invalid_session_id(&id) {
        return response;
    }
    match state.application.delete_session(&id) {
        Ok(()) => axum::Json(serde_json::json!({})).into_response(),
        Err(error) => super::map_error(&error),
    }
}

async fn update_settings(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(request): axum::Json<SettingsRequest>,
) -> Response {
    if let Some(response) = invalid_session_id(&id) {
        return response;
    }
    match state.application.update_settings(&id, request.settings) {
        Ok(session) => axum::Json(session).into_response(),
        Err(error) => super::map_error(&error),
    }
}

async fn inspect_merge(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    if let Some(response) = invalid_session_id(&id) {
        return response;
    }
    match state.application.inspect_pr_for_merge(&id) {
        Ok(status) => axum::Json(MergeStatusResponse {
            state: status.state,
            mergeable: status.mergeable,
            review_decision: status.review_decision,
            checks: status
                .checks
                .into_iter()
                .map(|check| MergeCheck {
                    name: check.name,
                    status: check.status,
                })
                .collect(),
        })
        .into_response(),
        Err(error) => super::map_error(&error),
    }
}

async fn merge_pr(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(request): axum::Json<MergePrRequest>,
) -> Response {
    if let Some(response) = invalid_session_id(&id) {
        return response;
    }
    let method = match request.method.as_str() {
        "merge" => crate::pr_merge::PrMergeMethod::Merge,
        "rebase" => crate::pr_merge::PrMergeMethod::Rebase,
        "squash" => crate::pr_merge::PrMergeMethod::Squash,
        _ => return super::error_response(StatusCode::BAD_REQUEST, "invalid merge method"),
    };
    let application = state.application.clone();
    let operation_id = id;
    match tokio::task::spawn_blocking(move || application.merge_pr(&operation_id, method)).await {
        Ok(Ok(crate::application::MergePrOutcome::Cleaned)) => {
            axum::Json(serde_json::json!({ "outcome": "cleaned" })).into_response()
        }
        Ok(Ok(crate::application::MergePrOutcome::Pending)) => {
            axum::Json(serde_json::json!({ "outcome": "pending" })).into_response()
        }
        Ok(Err(error)) => super::map_error(&error),
        Err(error) => super::error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("merge PR task failed: {error}"),
        ),
    }
}

async fn local_merge(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(request): axum::Json<LocalMergeRequest>,
) -> Response {
    if let Some(response) = invalid_session_id(&id) {
        return response;
    }
    let mode = match request.mode.as_str() {
        "stage" => LocalMergeMode::Stage,
        "mergeAndCleanup" => LocalMergeMode::MergeAndCleanup,
        _ => return super::error_response(StatusCode::BAD_REQUEST, "invalid local merge mode"),
    };
    let application = state.application.clone();
    match tokio::task::spawn_blocking(move || application.local_merge(&id, mode)).await {
        Ok(Ok(crate::local_merge::LocalMergeOutcome::Staged)) => {
            axum::Json(serde_json::json!({ "outcome": "staged" })).into_response()
        }
        Ok(Ok(crate::local_merge::LocalMergeOutcome::Merged)) => {
            axum::Json(serde_json::json!({ "outcome": "merged" })).into_response()
        }
        Ok(Ok(crate::local_merge::LocalMergeOutcome::MergedCleanupFailed { error })) => {
            axum::Json(serde_json::json!({ "outcome": "mergedCleanupFailed", "error": error }))
                .into_response()
        }
        Ok(Err(error)) => super::map_error(&error),
        Err(error) => super::error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("local merge task failed: {error}"),
        ),
    }
}

async fn create_pr(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(request): axum::Json<CreatePrRequest>,
) -> Response {
    if let Some(response) = invalid_session_id(&id) {
        return response;
    }
    let prepared = match state
        .application
        .prepare_create_pr_operation(&id, request.context.as_ref())
    {
        Ok(prepared) => prepared,
        Err(error) => {
            return match &error {
                crate::error::CruiseError::Busy(_)
                | crate::error::CruiseError::SessionNotFound(_) => super::map_error(&error),
                crate::error::CruiseError::IoError(io_error)
                    if io_error.kind() == std::io::ErrorKind::NotFound =>
                {
                    super::map_error(&error)
                }
                _ => super::error_response(StatusCode::BAD_REQUEST, &error.to_string()),
            };
        }
    };
    let application = state.application.clone();
    let hub = state.hub.clone();
    let session_id = id.clone();
    tokio::spawn(async move {
        let (sink, log_sink) = hub.sinks();
        if let Err(error) = application
            .run_create_pr_operation(&session_id, prepared, sink, Some(log_sink))
            .await
        {
            eprintln!("daemon: create PR for {session_id} failed: {error}");
        }
    });
    (
        StatusCode::ACCEPTED,
        axum::Json(serde_json::json!({ "sessionId": id })),
    )
        .into_response()
}
