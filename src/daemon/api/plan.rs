use crate::application::{OperationKind, PlanRequest, PlanStart};
use crate::daemon::server::WebState;
use crate::error::CruiseError;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::json;
const NEW_SESSION_BODY_LIMIT: usize =
    crate::issue_images::MAX_ISSUE_IMAGES * crate::issue_images::MAX_IMAGE_BYTES * 5;

pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route(
            "/sessions",
            post(create_session)
                .layer(axum::extract::DefaultBodyLimit::max(NEW_SESSION_BODY_LIMIT)),
        )
        .route("/sessions/{id}/config", get(read_config))
        .route("/sessions/{id}/plan", get(read_plan).post(start_plan))
        .route("/sessions/{id}/plan/fix", post(start_fix))
        .route("/sessions/{id}/plan/ask", post(start_ask))
        .route("/sessions/{id}/plan/verify", post(start_verify))
        .route("/sessions/{id}/plan/replan", post(start_replan))
        .route("/sessions/{id}/approve", post(approve))
        .route("/sessions/{id}/publish", post(publish))
        .route("/sessions/{id}/discard", post(discard))
}

async fn create_session(
    State(state): State<WebState>,
    axum::Json(request): axum::Json<crate::application::NewSessionRequest>,
) -> Response {
    let application = state.application.clone();
    match tokio::task::spawn_blocking(move || application.create_session(request)).await {
        Ok(Ok(session)) => (StatusCode::CREATED, axum::Json(session)).into_response(),
        Ok(Err(error)) => super::map_error(&error),
        Err(error) => super::error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to create session: {error}"),
        ),
    }
}

async fn read_plan(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    match state.application.session_plan(&id) {
        Ok(plan) => axum::Json(plan).into_response(),
        Err(error) => super::map_error(&error),
    }
}

async fn read_config(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    let result = state
        .application
        .read_session(&id)
        .and_then(|session| state.application.session_config(&session));
    match result {
        Ok(config) => axum::Json(config).into_response(),
        Err(error) => super::map_error(&error),
    }
}

async fn start_plan(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(request): axum::Json<PlanStart>,
) -> Response {
    start_operation(&id, &state, OperationKind::Generate, "plan", request)
}

async fn start_fix(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(request): axum::Json<PlanRequest>,
) -> Response {
    start_operation(&id, &state, OperationKind::Fix, "fix", request)
}

async fn start_ask(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(request): axum::Json<PlanRequest>,
) -> Response {
    start_operation(&id, &state, OperationKind::Ask, "ask", request)
}

async fn start_verify(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(request): axum::Json<PlanRequest>,
) -> Response {
    start_operation(&id, &state, OperationKind::Verify, "verify", request)
}

async fn start_replan(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(request): axum::Json<PlanRequest>,
) -> Response {
    start_operation(&id, &state, OperationKind::Replan, "replan", request)
}

fn start_operation(
    id: &str,
    state: &WebState,
    operation: OperationKind,
    execution_name: &'static str,
    request: PlanRequest,
) -> Response {
    if let Err(error) = crate::session::validate_session_id(id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    let claim = match state
        .application
        .claim_plan_operation(id, operation, &request)
    {
        Ok(claim) => claim,
        Err(CruiseError::Other(message)) => {
            return super::error_response(StatusCode::BAD_REQUEST, &message);
        }
        Err(error) => return super::map_error(&error),
    };
    let session_id = id.to_string();
    let application = state.application.clone();
    let (sink, _logs) = state.hub.sinks();
    let sink = std::sync::Arc::new(super::TerminalEventSink::new(sink));
    let terminal_sink = std::sync::Arc::clone(&sink);
    tokio::spawn(async move {
        if let Err(error) = application
            .plan_with_claim(&session_id, request, sink, operation, execution_name, claim)
            .await
            && !matches!(error, CruiseError::Interrupted)
        {
            eprintln!("daemon plan operation failed for {session_id}: {error}");
        }
        if let Err(error) = terminal_sink.finish() {
            eprintln!("daemon plan terminal event delivery failed for {session_id}: {error}");
        }
    });
    (StatusCode::ACCEPTED, axum::Json(json!({ "sessionId": id }))).into_response()
}

async fn approve(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(request): axum::Json<crate::application::ApproveRequest>,
) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    match state.application.approve_with_request(&id, request).await {
        Ok(session) => axum::Json(session).into_response(),
        Err(error) => super::map_error(&error),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PublishRequest {
    trigger_cruise: bool,
    #[serde(rename = "context")]
    _context: Option<crate::request_context::RequestContext>,
}

async fn publish(
    Path(id): Path<String>,
    State(state): State<WebState>,
    axum::Json(request): axum::Json<PublishRequest>,
) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    let application = state.application.clone();
    match tokio::task::spawn_blocking(move || application.publish(&id, request.trigger_cruise))
        .await
    {
        Ok(Ok(issue)) => axum::Json(json!({ "url": issue.url })).into_response(),
        Ok(Err(error)) => super::map_error(&error),
        Err(error) => super::error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("publish task failed: {error}"),
        ),
    }
}

async fn discard(Path(id): Path<String>, State(state): State<WebState>) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    let application = state.application.clone();
    match tokio::task::spawn_blocking(move || application.discard_session(&id)).await {
        Ok(Ok(())) => axum::Json(json!({})).into_response(),
        Ok(Err(error)) => super::map_error(&error),
        Err(error) => super::error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("discard task failed: {error}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::{CruiseApplication, OperationKind};
    use crate::daemon::server::WebState;
    use crate::session::SessionManager;

    #[tokio::test]
    async fn plan_start_returns_conflict_for_a_busy_session() -> crate::error::Result<()> {
        let dir = tempfile::tempdir()?;
        let manager = SessionManager::new(dir.path().to_path_buf());
        let session_id = SessionManager::new_session_id();
        manager.create(&crate::session::SessionState::new(
            session_id.clone(),
            dir.path().to_path_buf(),
            crate::session_config::SessionConfigRef::inline_snapshot(),
            "busy plan test".to_owned(),
        ))?;
        let application = CruiseApplication::new(manager);
        let _claim = application
            .runtime()
            .try_begin(&session_id, OperationKind::Run)?;
        let webui = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/webui"));
        let assets = crate::webui::assets::Assets {
            templates_dir: webui.join("templates"),
            static_dir: webui.join("static"),
            dev_mode: true,
        };
        let state = WebState::new(application, &assets)?;

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            start_plan(
                Path(session_id),
                State(state),
                axum::Json(PlanStart::default()),
            ),
        )
        .await
        .map_err(|error| {
            crate::error::CruiseError::Other(format!("busy plan start timed out: {error}"))
        })?;

        assert_eq!(response.status(), StatusCode::CONFLICT);
        Ok(())
    }
}
