use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::Deserialize;

use crate::application::{
    ApplicationEvent, ApplicationEventSink, OperationKind, RunAllRequest, RunRequest, TellRequest,
};
use crate::daemon::server::WebState;

pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/sessions/{id}/run", post(start_run))
        .route("/run-all", post(start_run_all))
        .route("/sessions/{id}/instructions", post(submit_instruction))
        .route(
            "/sessions/{id}/instructions/history",
            post(instruction_history),
        )
}

async fn start_run(Path(id): Path<String>, State(state): State<WebState>, body: Bytes) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    if let Err(error) = state.application.read_session(&id) {
        return super::map_error(&error);
    }
    let request = match serde_json::from_slice::<RunRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
        }
    };
    let active = state.application.runtime().active_operation(&id);
    let attached = active == Some(OperationKind::Run);
    if let Some(operation) = active
        && !attached
    {
        return super::map_error(&crate::error::CruiseError::Busy(format!(
            "session {id} already has an active {operation:?} operation"
        )));
    }
    let claim = if attached {
        None
    } else {
        if let Err(error) = state.application.preflight_run_auth(&id, &request) {
            return if matches!(&error, crate::error::CruiseError::AgentAuthExpired(_)) {
                super::error_response(StatusCode::BAD_REQUEST, &error.to_string())
            } else {
                super::map_error(&error)
            };
        }
        match state
            .application
            .runtime()
            .try_begin(&id, OperationKind::Run)
        {
            Ok(claim) => Some(claim),
            Err(error) => return super::map_error(&error),
        }
    };
    if let Some(claim) = claim {
        let application = state.application.clone();
        let (sink, log_sink) = state.hub.sinks();
        let sink = std::sync::Arc::new(super::TerminalEventSink::new(sink));
        let terminal_sink = std::sync::Arc::clone(&sink);
        let operation_id = id.clone();
        tokio::spawn(async move {
            let _ = application
                .run_with_claim_and_log_sink(claim, request, sink, Some(log_sink))
                .await;
            if let Err(error) = terminal_sink.finish() {
                eprintln!("daemon run terminal event delivery failed for {operation_id}: {error}");
            }
        });
    }
    (
        StatusCode::ACCEPTED,
        axum::Json(serde_json::json!({ "attached": attached })),
    )
        .into_response()
}

async fn start_run_all(State(state): State<WebState>, body: Bytes) -> Response {
    let request = match serde_json::from_slice::<RunAllRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
        }
    };
    if request.parallelism == Some(0) {
        return super::error_response(StatusCode::BAD_REQUEST, "parallelism must be at least 1");
    }
    let attached = state.application.runtime().batch_active();
    if !attached {
        let titles = state
            .application
            .run_all_candidates()
            .unwrap_or_default()
            .into_iter()
            .map(|session| (session.id.clone(), session.input_or_title().to_owned()))
            .collect();
        state.run_all.start(titles);
        let sink: std::sync::Arc<dyn ApplicationEventSink> =
            std::sync::Arc::new(crate::webui::events::RunAllSink {
                hub: std::sync::Arc::clone(&state.hub),
                state: std::sync::Arc::clone(&state.run_all),
            });
        let log_sink: std::sync::Arc<dyn crate::application::LogSink> =
            std::sync::Arc::new(crate::webui::events::RunAllLogSink {
                hub: std::sync::Arc::clone(&state.hub),
                state: std::sync::Arc::clone(&state.run_all),
            });
        let application = state.application.clone();
        let sink = std::sync::Arc::new(super::TerminalEventSink::new(sink));
        let terminal_sink = std::sync::Arc::clone(&sink);
        let failure_sink = std::sync::Arc::clone(&sink);
        tokio::spawn(async move {
            if let Err(error) = application
                .run_all_with_request(request, sink, Some(log_sink))
                .await
            {
                let _ = failure_sink.send(ApplicationEvent::BatchFailed {
                    error: error.to_string(),
                });
            }
            if let Err(error) = terminal_sink.finish() {
                eprintln!("daemon run-all terminal event delivery failed: {error}");
            }
        });
    }
    (
        StatusCode::ACCEPTED,
        axum::Json(serde_json::json!({ "attached": attached })),
    )
        .into_response()
}

async fn submit_instruction(
    Path(id): Path<String>,
    State(state): State<WebState>,
    body: Bytes,
) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    let request = match serde_json::from_slice::<TellRequest>(&body) {
        Ok(request) => request,
        Err(error) => {
            return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
        }
    };
    match state.application.submit_instruction(&id, request) {
        Ok(receipt) => axum::Json(receipt).into_response(),
        Err(error) => super::map_error(&error),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstructionHistoryRequest {
    #[serde(default)]
    _context: Option<crate::request_context::RequestContext>,
}

async fn instruction_history(
    Path(id): Path<String>,
    State(state): State<WebState>,
    body: Bytes,
) -> Response {
    if let Err(error) = crate::session::validate_session_id(&id) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    if let Err(error) = serde_json::from_slice::<InstructionHistoryRequest>(&body) {
        return super::error_response(StatusCode::BAD_REQUEST, &error.to_string());
    }
    match state.application.instruction_history(&id) {
        Ok(history) => axum::Json(history).into_response(),
        Err(error) => super::map_error(&error),
    }
}
