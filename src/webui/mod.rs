//! Server-rendered browser client for Cruise.
//!
//! `cruise webui` serves an axum application that renders TSX templates with
//! `jsxrs` and drives updates with HTMX 4: actions answer with `<hx-partial>`
//! fragments and one Server-Sent-Events connection streams the same fragments
//! for work happening in the background.

pub(crate) mod actions;
pub(crate) mod assets;
pub(crate) mod css;
pub(crate) mod dag_mermaid;
pub(crate) mod dto;
pub(crate) mod events;
pub(crate) mod handlers;
pub(crate) mod markdown;
pub(crate) mod ops;
pub(crate) mod pages;
pub(crate) mod partials;
pub(crate) mod run_all_state;
pub(crate) mod templates;
pub(crate) mod view;

#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod tests;

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::HeaderValue;
use axum::http::header::CONTENT_TYPE;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures::Stream;
use tower_http::services::ServeDir;

use crate::application::CruiseApplication;
use crate::cli::WebuiArgs;
use crate::error::{CruiseError, Result};
use crate::session::SessionManager;

use css::GeneratedCss;
use events::EventHub;
use partials::SseMessage;
use run_all_state::RunAllState;
use templates::Templates;

const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

/// Everything a request handler needs; cheap to clone.
#[derive(Clone)]
pub(crate) struct WebState {
    pub(crate) application: CruiseApplication,
    pub(crate) hub: Arc<EventHub>,
    pub(crate) templates: Arc<Templates>,
    pub(crate) run_all: Arc<RunAllState>,
    pub(crate) templates_dir: PathBuf,
    pub(crate) static_dir: PathBuf,
    pub(crate) css: Arc<GeneratedCss>,
}

impl std::fmt::Debug for WebState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WebState").finish_non_exhaustive()
    }
}

impl WebState {
    pub(crate) fn new(application: CruiseApplication, assets: &assets::Assets) -> Result<Self> {
        let generated = if assets.dev_mode {
            None
        } else {
            Some(css::generate(&assets.templates_dir, &assets.static_dir)?)
        };
        Ok(Self {
            application,
            hub: Arc::new(EventHub::default()),
            templates: Arc::new(Templates::new(assets.templates_dir.clone())),
            run_all: Arc::new(RunAllState::default()),
            templates_dir: assets.templates_dir.clone(),
            static_dir: assets.static_dir.clone(),
            css: Arc::new(GeneratedCss::new(generated, assets.dev_mode)),
        })
    }
}

/// Serve the `WebUI` until Ctrl-C.
///
/// # Errors
///
/// Returns an error when assets cannot be prepared or the listener cannot bind.
pub async fn run(args: WebuiArgs) -> Result<()> {
    let data_dir = crate::paths::data_dir()?;
    let assets = assets::prepare(args.webui_dir, &data_dir)?;
    let application = CruiseApplication::new(SessionManager::new(data_dir));
    let state = WebState::new(application, &assets)?;

    let listener = tokio::net::TcpListener::bind((args.host.as_str(), args.port))
        .await
        .map_err(|error| {
            CruiseError::Other(format!(
                "failed to bind {}:{}: {error}",
                args.host, args.port
            ))
        })?;
    let local_addr = listener
        .local_addr()
        .map_err(|error| CruiseError::Other(error.to_string()))?;
    let url = format!("http://{local_addr}/");
    println!("cruise webui listening on {url}");
    if !local_addr.ip().is_loopback() {
        eprintln!(
            "warning: the WebUI has no authentication; do not expose it on untrusted networks"
        );
    }
    if !args.no_open
        && let Err(error) = crate::platform::open_url(&url)
    {
        eprintln!("could not open a browser ({error}); open {url} manually");
    }
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|error| CruiseError::Other(error.to_string()))
}

/// Build the application router.
pub(crate) fn router(state: WebState) -> Router {
    let static_dir = state.static_dir.clone();
    Router::new()
        .route("/", get(pages::root))
        .route("/sessions/{id}", get(pages::session))
        .route("/new", get(pages::new_session))
        .route("/run-all", get(pages::run_all))
        .route("/static/app.css", get(app_css))
        .route("/webui/events", get(sse))
        .route("/webui/sidebar", get(handlers::sidebar))
        .route("/webui/directories", get(handlers::directories))
        .route("/webui/configs", get(handlers::configs))
        .route("/webui/new/steps", get(handlers::new_steps))
        .route("/webui/repos", get(handlers::repos))
        .route("/webui/settings", get(handlers::settings_modal))
        .route("/webui/settings", post(handlers::save_settings))
        .route(
            "/webui/attachments/preview",
            get(handlers::attachment_preview),
        )
        .route(
            "/webui/attachments",
            post(handlers::upload_attachments).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/webui/sessions/{id}/tab/{tab}", get(handlers::tab))
        .route("/webui/sessions/{id}/log", get(handlers::session_log))
        .route(
            "/webui/sessions/{id}/settings",
            get(handlers::session_settings).post(handlers::update_settings),
        )
        .route(
            "/webui/sessions/{id}/settings/regenerate",
            post(handlers::regenerate_settings),
        )
        .route("/webui/sessions/{id}/editor/{kind}", get(handlers::editor))
        .route("/webui/sessions", post(handlers::create_session))
        .route(
            "/webui/sessions/draft",
            post(handlers::create_draft_session),
        )
        .route("/webui/drafts", post(handlers::save_draft))
        .route("/webui/sessions/{id}/approve", post(handlers::approve))
        .route(
            "/webui/sessions/{id}/use-input-as-plan",
            post(handlers::use_input_as_plan),
        )
        .route("/webui/sessions/{id}/generate", post(handlers::generate))
        .route("/webui/sessions/{id}/replan", post(handlers::replan))
        .route("/webui/sessions/{id}/fix", post(handlers::fix))
        .route("/webui/sessions/{id}/ask", post(handlers::ask))
        .route("/webui/sessions/{id}/run", post(handlers::run_session))
        .route("/webui/sessions/{id}/cancel", post(handlers::cancel))
        .route("/webui/sessions/{id}/reset", post(handlers::reset))
        .route(
            "/webui/sessions/{id}/ask-answer",
            post(handlers::ask_answer),
        )
        .route("/webui/sessions/{id}/option", post(handlers::option_answer))
        .route("/webui/sessions/{id}/publish", post(handlers::publish))
        .route("/webui/sessions/{id}/discard", post(handlers::discard))
        .route("/webui/sessions/{id}/delete", post(handlers::delete))
        .route("/webui/run-all", post(handlers::start_run_all))
        .route("/webui/run-all/cancel", post(handlers::cancel_run_all))
        .route("/webui/clean", post(handlers::clean))
        .nest_service("/static", ServeDir::new(static_dir))
        .fallback(pages::not_found)
        .layer(axum::middleware::map_response(no_store))
        .with_state(state)
}

/// Every response is generated from live session state; a cached fragment
/// would silently show stale data after an action.
async fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    response
}

async fn app_css(State(state): State<WebState>) -> Response {
    match state.css.text(&state.templates_dir, &state.static_dir) {
        Ok(text) => ([(CONTENT_TYPE, "text/css; charset=utf-8")], text).into_response(),
        Err(error) => {
            eprintln!("webui: failed to generate CSS: {error}");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                [(CONTENT_TYPE, "text/css; charset=utf-8")],
                String::new(),
            )
                .into_response()
        }
    }
}

async fn sse(
    State(state): State<WebState>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let receiver = state.hub.subscribe();
    let stream = futures::stream::unfold(
        (state, receiver, std::collections::VecDeque::new()),
        |(state, mut receiver, mut queue): (
            WebState,
            tokio::sync::mpsc::Receiver<crate::application::ApplicationEvent>,
            std::collections::VecDeque<Event>,
        )| async move {
            loop {
                if let Some(event) = queue.pop_front() {
                    return Some((Ok(event), (state, receiver, queue)));
                }
                let application_event = receiver.recv().await?;
                for message in partials::for_event(&state, &application_event) {
                    match message {
                        SseMessage::Html(html) => queue.push_back(Event::default().data(html)),
                        SseMessage::Notify { title, body } => {
                            match Event::default()
                                .event("notify")
                                .json_data(serde_json::json!({ "title": title, "body": body }))
                            {
                                Ok(event) => queue.push_back(event),
                                Err(error) => {
                                    eprintln!("webui: failed to encode notification: {error}");
                                }
                            }
                        }
                    }
                }
            }
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}
