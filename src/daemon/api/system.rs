use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Serialize;

use crate::daemon::instance::Registry;
use crate::daemon::server::{self, WebState};

pub(super) fn routes() -> Router<WebState> {
    Router::new()
        .route("/status", get(status))
        .route("/shutdown", post(shutdown))
        .route("/login-nonce", post(login_nonce))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusResponse {
    version: String,
    pid: u32,
    started_at: u64,
    active_operations: usize,
    batch_active: bool,
    origin: String,
}

#[derive(Serialize)]
struct LoginNonceResponse {
    url: String,
}

async fn status(State(state): State<WebState>) -> Response {
    let record = match Registry::new().and_then(|registry| registry.read()) {
        Ok(Some(record)) => record,
        Ok(None) => {
            return super::error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "daemon registry record is missing",
            );
        }
        Err(error) => return super::map_error(&error),
    };
    let runtime = state.application.runtime();
    axum::Json(StatusResponse {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        pid: std::process::id(),
        started_at: record.started_at,
        active_operations: runtime.active_operations(),
        batch_active: runtime.batch_active(),
        origin: record.origin,
    })
    .into_response()
}

async fn login_nonce(State(state): State<WebState>) -> Response {
    let Some(origin) = state.origin.as_deref() else {
        return super::error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "daemon origin is unavailable",
        );
    };
    axum::Json(LoginNonceResponse {
        url: crate::daemon::web_auth::new_login_url(&state, origin),
    })
    .into_response()
}

async fn shutdown(State(state): State<WebState>) -> Response {
    match server::signal_shutdown(&state) {
        Some(active_operations) => (
            StatusCode::ACCEPTED,
            [(header::CONNECTION, "close")],
            axum::Json(serde_json::json!({ "activeOperations": active_operations })),
        )
            .into_response(),
        None => super::error_response(StatusCode::NOT_FOUND, "daemon shutdown is unavailable"),
    }
}
