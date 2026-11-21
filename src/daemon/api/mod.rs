mod exec;
mod list;
mod manager;
mod plan;
mod run;
pub(crate) mod sessions;
mod system;

use std::sync::Mutex;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::application::{ApplicationEvent, ApplicationEventSink};
use crate::daemon::instance;
use crate::daemon::server::WebState;

/// Defers terminal SSE events until the daemon has exported the owning operation root.
pub(super) struct TerminalEventSink {
    inner: std::sync::Arc<dyn ApplicationEventSink>,
    pending: Mutex<Vec<ApplicationEvent>>,
}

impl TerminalEventSink {
    pub(super) fn new(inner: std::sync::Arc<dyn ApplicationEventSink>) -> Self {
        Self {
            inner,
            pending: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn finish(&self) -> crate::error::Result<()> {
        self.release_pending(None, true)
    }

    fn release_pending(&self, session_id: Option<&str>, flush: bool) -> crate::error::Result<()> {
        let events = {
            let mut pending = self
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut events = Vec::new();
            let mut remaining = Vec::new();
            for event in pending.drain(..) {
                let ready = session_id
                    .is_none_or(|session_id| terminal_session_id(&event) == Some(session_id));
                if ready {
                    events.push(event);
                } else {
                    remaining.push(event);
                }
            }
            *pending = remaining;
            events
        };

        if flush && !events.is_empty() {
            crate::telemetry::force_flush();
        }
        for event in events {
            self.inner.send(event)?;
        }
        Ok(())
    }
}

impl ApplicationEventSink for TerminalEventSink {
    fn send(&self, event: ApplicationEvent) -> crate::error::Result<()> {
        if is_terminal_event(&event) {
            self.pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event);
            Ok(())
        } else {
            self.inner.send(event)
        }
    }

    fn operation_finished(&self, session_id: &str) -> crate::error::Result<()> {
        self.release_pending(Some(session_id), false)
    }
}

fn is_terminal_event(event: &ApplicationEvent) -> bool {
    matches!(
        event,
        ApplicationEvent::PlanFinished { .. }
            | ApplicationEvent::PlanFailed { .. }
            | ApplicationEvent::PlanCancelled { .. }
            | ApplicationEvent::RunFinished { .. }
            | ApplicationEvent::RunFailed { .. }
            | ApplicationEvent::RunCancelled { .. }
            | ApplicationEvent::BatchFinished { .. }
            | ApplicationEvent::BatchFailed { .. }
    )
}

fn terminal_session_id(event: &ApplicationEvent) -> Option<&str> {
    match event {
        ApplicationEvent::PlanFinished { session_id, .. }
        | ApplicationEvent::PlanFailed { session_id, .. }
        | ApplicationEvent::PlanCancelled { session_id }
        | ApplicationEvent::RunFinished { session_id, .. }
        | ApplicationEvent::RunFailed { session_id, .. }
        | ApplicationEvent::RunCancelled { session_id } => Some(session_id),
        ApplicationEvent::BatchSessionFinished { .. }
        | ApplicationEvent::BatchFinished { .. }
        | ApplicationEvent::BatchFailed { .. }
        | ApplicationEvent::PlanStarted { .. }
        | ApplicationEvent::PlanChunk { .. }
        | ApplicationEvent::AskUserRequired { .. }
        | ApplicationEvent::RunStarted { .. }
        | ApplicationEvent::RunPhase { .. }
        | ApplicationEvent::StepStarted { .. }
        | ApplicationEvent::OptionRequired { .. }
        | ApplicationEvent::PrCreated { .. }
        | ApplicationEvent::BatchStarted { .. }
        | ApplicationEvent::BatchTotalChanged { .. }
        | ApplicationEvent::BatchSessionStarted { .. }
        | ApplicationEvent::AutoRequeued { .. }
        | ApplicationEvent::LogChunk { .. } => None,
    }
}

pub(crate) fn router(state: WebState) -> Router {
    Router::<WebState>::new()
        .merge(system::routes())
        .merge(sessions::routes())
        .merge(exec::routes())
        .merge(list::routes())
        .merge(plan::routes())
        .merge(run::routes())
        .merge(crate::daemon::events::routes())
        .fallback(not_found)
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

pub(crate) fn manager_router(state: WebState) -> Router {
    manager::routes()
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
        .with_state(state)
}

async fn authenticate(
    State(state): State<WebState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let expected = state.api_token.as_deref().unwrap_or_default();
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    if !instance::tokens_match(expected, supplied) {
        return error_response(StatusCode::UNAUTHORIZED, "invalid or missing bearer token");
    }
    next.run(request).await
}

async fn not_found() -> Response {
    error_response(StatusCode::NOT_FOUND, "API route not found")
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
}

pub(super) fn error_response(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(ErrorBody { error: message })).into_response()
}

pub(super) fn map_error(error: &crate::error::CruiseError) -> Response {
    let status = match error {
        crate::error::CruiseError::Busy(_) => StatusCode::CONFLICT,
        crate::error::CruiseError::SessionNotFound(_) => StatusCode::NOT_FOUND,
        crate::error::CruiseError::IoError(error)
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            StatusCode::NOT_FOUND
        }
        crate::error::CruiseError::Other(message)
            if message.contains("stale or unknown prompt")
                || message.contains("stale prompt request")
                || message.contains("prompt worker is no longer waiting") =>
        {
            StatusCode::CONFLICT
        }
        crate::error::CruiseError::Other(message)
            if message.contains("must not be empty")
                || message.contains("invalid option response") =>
        {
            StatusCode::BAD_REQUEST
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let message = match error {
        crate::error::CruiseError::Busy(message)
        | crate::error::CruiseError::SessionNotFound(message) => message.clone(),
        crate::error::CruiseError::IoError(error) => error.to_string(),
        _ => error.to_string(),
    };
    error_response(status, &message)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt as _;

    use super::router;
    use crate::application::CruiseApplication;
    use crate::daemon::server::WebState;
    use crate::session::SessionManager;
    use crate::webui::assets::Assets;

    fn state(path: &std::path::Path) -> WebState {
        let webui = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/webui"));
        let assets = Assets {
            templates_dir: webui.join("templates"),
            static_dir: webui.join("static"),
            dev_mode: true,
        };
        WebState::new(
            CruiseApplication::new(SessionManager::new(path.to_path_buf())),
            &assets,
        )
        .unwrap_or_else(|error| panic!("create web state: {error}"))
    }

    #[tokio::test]
    async fn api_requires_bearer_auth_and_lists_sessions() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let mut state = state(dir.path());
        let request = Request::builder()
            .uri("/sessions")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("request: {error}"));
        let response = router(state.clone())
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {});
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap_or_else(|error| panic!("response body: {error}"));
        assert!(
            serde_json::from_slice::<serde_json::Value>(&body)
                .is_ok_and(|body| body["error"].is_string())
        );

        state.api_token = Some("test-token".to_owned());
        let request = Request::builder()
            .method("POST")
            .uri("/login-nonce")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("unauthenticated login-nonce request: {error}"));
        let response = router(state.clone())
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {});
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let request = Request::builder()
            .uri("/sessions")
            .header("authorization", "Bearer test-token")
            .body(Body::empty())
            .unwrap_or_else(|error| panic!("request: {error}"));
        let response = router(state)
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {});
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap_or_else(|error| panic!("response body: {error}"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body)
                .unwrap_or_else(|error| panic!("session list: {error}")),
            serde_json::json!([])
        );
    }

    #[tokio::test]
    async fn error_mapping_sends_inner_message_without_variant_prefix() {
        for (error, expected_status, expected_message) in [
            (
                crate::error::CruiseError::Busy("occupied".to_owned()),
                StatusCode::CONFLICT,
                "occupied",
            ),
            (
                crate::error::CruiseError::SessionNotFound("missing".to_owned()),
                StatusCode::NOT_FOUND,
                "missing",
            ),
            (
                crate::error::CruiseError::IoError(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "gone",
                )),
                StatusCode::NOT_FOUND,
                "gone",
            ),
        ] {
            let response = super::map_error(&error);
            assert_eq!(response.status(), expected_status);
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap_or_else(|error| panic!("error response body: {error}"));
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&body)
                    .unwrap_or_else(|error| panic!("error response JSON: {error}"))["error"],
                expected_message
            );
        }
    }

    #[test]
    fn batch_session_finish_is_not_held_until_the_batch_ends() {
        use crate::application::{ApplicationEvent, ApplicationEventSink};
        use std::sync::{Arc, Mutex};

        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let inner: Arc<dyn ApplicationEventSink> = Arc::new(move |event: ApplicationEvent| {
            recorder
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event);
            Ok(())
        });
        let sink = super::TerminalEventSink::new(inner);
        let run_finished = ApplicationEvent::RunFinished {
            session_id: "s1".into(),
            phase: "Completed".into(),
        };
        let batch_session_finished = ApplicationEvent::BatchSessionFinished {
            id: "s1".into(),
            phase: "Completed".into(),
            error: None,
        };
        let send = |event: &ApplicationEvent| {
            sink.send(event.clone())
                .unwrap_or_else(|error| panic!("send: {error}"));
        };
        let delivered = || {
            seen.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };

        // The session's own terminal event waits for its operation to finish ...
        send(&run_finished);
        assert_eq!(delivered(), Vec::<ApplicationEvent>::new());
        sink.operation_finished("s1")
            .unwrap_or_else(|error| panic!("operation_finished: {error}"));
        assert_eq!(delivered(), vec![run_finished.clone()]);

        // ... but the batch row update for that session is delivered immediately,
        // not when the whole batch finishes.
        send(&batch_session_finished);
        assert_eq!(delivered(), vec![run_finished, batch_session_finished]);
    }
}
