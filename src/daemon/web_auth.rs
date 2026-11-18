//! Browser authentication and request-origin checks for the daemon.

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header::{COOKIE, HOST, LOCATION, ORIGIN, SET_COOKIE};
use axum::http::{HeaderValue, Method, Request, StatusCode, Uri, uri::Authority};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use url::Url;

use crate::daemon::instance;
use crate::daemon::server::WebState;

const SESSION_COOKIE: &str = "cruise_webui";
const LOGIN_NONCE_TTL: Duration = Duration::from_mins(5);
const UNAUTHORIZED_HTML: &str = "<!doctype html><html><head><title>Sign in required</title></head><body><h1>Sign in required</h1><p>Run <code>cruise webui</code> to sign in.</p></body></html>";

#[derive(Clone, Copy)]
pub(crate) struct HostGuard {
    pub(crate) loopback: bool,
}

#[derive(Deserialize)]
pub(crate) struct LoginQuery {
    nonce: String,
}

#[must_use]
pub(crate) fn login_url(origin: &str, nonce: &str) -> String {
    format!("{origin}login?nonce={nonce}")
}

#[must_use]
pub(crate) fn new_login_url(state: &WebState, origin: &str) -> String {
    login_url(origin, &issue_login_nonce(state))
}

pub(crate) fn issue_login_nonce(state: &WebState) -> String {
    let now = Instant::now();
    let nonce = instance::new_token();
    let mut nonces = state
        .login_nonces
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    nonces.retain(|_, expires_at| *expires_at > now);
    nonces.insert(nonce.clone(), now + LOGIN_NONCE_TTL);
    nonce
}

fn consume_login_nonce(state: &WebState, nonce: &str) -> bool {
    let now = Instant::now();
    let Some(expires_at) = state
        .login_nonces
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(nonce)
    else {
        return false;
    };
    expires_at > now
}

fn new_session_id(state: &WebState) -> String {
    let session_id = instance::new_token();
    state
        .web_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(session_id.clone());
    session_id
}

fn has_session(state: &WebState, session_id: &str) -> bool {
    state
        .web_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(session_id)
}

pub(crate) async fn login(
    State(state): State<WebState>,
    Query(query): Query<LoginQuery>,
) -> Response {
    if !consume_login_nonce(&state, &query.nonce) {
        return unauthorized();
    }

    let cookie = format!(
        "{SESSION_COOKIE}={}; HttpOnly; SameSite=Strict; Path=/",
        new_session_id(&state)
    );
    let Ok(cookie) = HeaderValue::from_str(&cookie) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut response = StatusCode::SEE_OTHER.into_response();
    response
        .headers_mut()
        .insert(LOCATION, HeaderValue::from_static("/"));
    response.headers_mut().insert(SET_COOKIE, cookie);
    response
}

pub(crate) async fn authenticate(
    State(state): State<WebState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if path == "/login" || path == "/static" || path.starts_with("/static/") {
        return next.run(request).await;
    }

    let cookie_header = request
        .headers()
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let supplied = cookie_header
        .split(';')
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find_map(|(name, value)| (name == SESSION_COOKIE).then_some(value))
        .unwrap_or_default();
    if !has_session(&state, supplied) {
        return unauthorized();
    }
    next.run(request).await
}

pub(crate) async fn guard(
    State(config): State<HostGuard>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let authority = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<Authority>().ok());

    if config.loopback {
        let Some(authority) = authority.as_ref() else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        let host = normalized_host(authority.host());
        if !(host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1") {
            return StatusCode::BAD_REQUEST.into_response();
        }
    }

    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        let mut origins = request.headers().get_all(ORIGIN).iter();
        if let Some(origin) = origins.next() {
            if origins.next().is_some() {
                return StatusCode::FORBIDDEN.into_response();
            }
            let Some(authority) = authority.as_ref() else {
                return StatusCode::FORBIDDEN.into_response();
            };
            if !is_same_origin(
                origin,
                authority,
                request.uri(),
                request.headers().get("x-forwarded-proto"),
            ) {
                return StatusCode::FORBIDDEN.into_response();
            }
        }
    }

    next.run(request).await
}

fn normalized_host(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host)
}

fn is_same_origin(
    origin: &HeaderValue,
    authority: &Authority,
    uri: &Uri,
    forwarded_proto: Option<&HeaderValue>,
) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(origin) = Url::parse(origin) else {
        return false;
    };
    if !origin.username().is_empty()
        || origin.password().is_some()
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.fragment().is_some()
    {
        return false;
    }

    let scheme = forwarded_proto
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .or_else(|| uri.scheme_str())
        .unwrap_or("http");
    if !matches!(scheme, "http" | "https") || origin.scheme() != scheme {
        return false;
    }

    let Some(origin_host) = origin.host_str() else {
        return false;
    };
    let origin_port = origin.port_or_known_default();
    let request_port = authority.port_u16().or_else(|| default_port(scheme));
    normalized_host(origin_host).eq_ignore_ascii_case(normalized_host(authority.host()))
        && origin_port == request_port
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn unauthorized() -> Response {
    let mut response = (StatusCode::UNAUTHORIZED, UNAUTHORIZED_HTML).into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt as _;

    use super::{
        HostGuard, consume_login_nonce, guard, has_session, issue_login_nonce, new_session_id,
    };
    use crate::application::CruiseApplication;
    use crate::daemon::server::WebState;
    use crate::session::SessionManager;
    use crate::webui::assets::Assets;
    use axum::Router;
    use axum::middleware;
    use axum::routing::get;

    fn guarded(loopback: bool) -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }).post(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                HostGuard { loopback },
                guard,
            ))
    }

    fn test_state() -> (WebState, tempfile::TempDir) {
        let data_dir = tempfile::tempdir().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let webui = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/webui"));
        let assets = Assets {
            templates_dir: webui.join("templates"),
            static_dir: webui.join("static"),
            dev_mode: true,
        };
        let state = WebState::new(
            CruiseApplication::new(SessionManager::new(data_dir.path().to_path_buf())),
            &assets,
        )
        .unwrap_or_else(|error| panic!("create auth test state: {error}"));
        (state, data_dir)
    }

    #[tokio::test]
    async fn loopback_host_allows_local_names_and_tunnel_ports_only() {
        let response = guarded(true)
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("host", "evil.example:8484")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request: {error}")),
            )
            .await
            .unwrap_or_else(|never| match never {});
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        for host in ["localhost:9999", "127.0.0.1:8484", "[::1]:8080"] {
            let response = guarded(true)
                .oneshot(
                    Request::builder()
                        .uri("/")
                        .header("host", host)
                        .body(Body::empty())
                        .unwrap_or_else(|error| panic!("request: {error}")),
                )
                .await
                .unwrap_or_else(|never| match never {});
            assert_eq!(response.status(), StatusCode::OK, "{host}");
        }
    }

    #[tokio::test]
    async fn foreign_origin_is_rejected_for_mutating_requests() {
        let response = guarded(true)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("host", "localhost:8484")
                    .header("origin", "http://evil.example:8484")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request: {error}")),
            )
            .await
            .unwrap_or_else(|never| match never {});
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn same_origin_is_accepted_for_mutating_requests() {
        let response = guarded(true)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/")
                    .header("host", "localhost:8484")
                    .header("origin", "http://localhost:8484")
                    .body(Body::empty())
                    .unwrap_or_else(|error| panic!("request: {error}")),
            )
            .await
            .unwrap_or_else(|never| match never {});
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn login_nonce_is_single_use_and_expires_and_session_is_independent() {
        let (mut state, _data_dir) = test_state();
        state.api_token = Some("test-secret".to_owned());
        let nonce = issue_login_nonce(&state);
        assert!(consume_login_nonce(&state, &nonce));
        assert!(!consume_login_nonce(&state, &nonce));

        let expired = issue_login_nonce(&state);
        state
            .login_nonces
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                expired.clone(),
                std::time::Instant::now()
                    .checked_sub(std::time::Duration::from_secs(1))
                    .unwrap_or_else(|| panic!("monotonic clock is less than one second old")),
            );
        assert!(!consume_login_nonce(&state, &expired));

        let session_id = new_session_id(&state);
        assert_ne!(session_id, state.api_token.as_deref().unwrap_or_default());
        assert!(has_session(&state, &session_id));
        assert!(!has_session(&state, "test-secret"));
    }
}
