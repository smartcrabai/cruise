use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::Serialize;

use crate::application::ExecStartRequest;
use crate::daemon::server::WebState;

pub(super) fn routes() -> Router<WebState> {
    Router::new().route("/exec", post(start_exec))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartedExec {
    session_id: String,
}

async fn start_exec(
    State(state): State<WebState>,
    Json(request): Json<ExecStartRequest>,
) -> Response {
    let (event_sink, log_sink) = state.hub.sinks();
    let event_sink = std::sync::Arc::new(super::TerminalEventSink::new(event_sink));
    match state
        .application
        .start_exec(request, event_sink, Some(log_sink))
    {
        Ok(session_id) => (StatusCode::ACCEPTED, Json(StartedExec { session_id })).into_response(),
        Err(error) => super::map_error(&error),
    }
}
