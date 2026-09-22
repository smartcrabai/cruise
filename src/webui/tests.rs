//! Router and template contract tests.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::StreamExt as _;
use tower::ServiceExt as _;

use super::{WebState, assets, router};
use crate::application::{CruiseApplication, NewSessionRequest};
use crate::session::{SessionManager, SessionState, WorkspaceMode};

const CONFIG_YAML: &str = "command: [cat]\nsteps:\n  s1:\n    prompt: plan\n";

fn webui_root() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/webui"))
}

fn repo_assets() -> assets::Assets {
    let root = webui_root();
    assets::Assets {
        templates_dir: root.join("templates"),
        static_dir: root.join("static"),
        dev_mode: true,
    }
}

struct Harness {
    state: WebState,
    dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let Ok(dir) = tempfile::tempdir() else {
        panic!("tempdir");
    };
    let application = CruiseApplication::new(SessionManager::new(dir.path().to_path_buf()));
    let Ok(state) = WebState::new(application, &repo_assets()) else {
        panic!("web state");
    };
    Harness { state, dir }
}

fn create_session(harness: &Harness) -> SessionState {
    let base = harness.dir.path().join("work");
    let Ok(()) = std::fs::create_dir_all(&base) else {
        panic!("create base dir");
    };
    let result = harness.state.application.create_session(NewSessionRequest {
        input: "add a webui".to_string(),
        base_dir: base,
        config_path: None,
        config_yaml: Some(CONFIG_YAML.to_string()),
        repo: None,
        workspace_mode: WorkspaceMode::Worktree,
        allow_dirty_working_tree: true,
        attachments: Vec::new(),
        skipped_steps: Vec::new(),
    });
    match result {
        Ok(session) => session,
        Err(error) => panic!("create_session: {error}"),
    }
}

async fn get(
    harness: &Harness,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, String, String) {
    let mut builder = Request::builder().uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let Ok(request) = builder.body(Body::empty()) else {
        panic!("request");
    };
    send(harness, request).await
}

async fn post(harness: &Harness, uri: &str, body: &'static str) -> (StatusCode, String, String) {
    let Ok(request) = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Body::from(body))
    else {
        panic!("request");
    };
    send(harness, request).await
}

async fn send(harness: &Harness, request: Request<Body>) -> (StatusCode, String, String) {
    let response = match router(harness.state.clone()).oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    };
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let Ok(bytes) = axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024).await else {
        panic!("body");
    };
    (
        status,
        content_type,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[test]
fn templates_render_all_fixtures() {
    let harness = harness();
    let root = webui_root().join("templates");
    let fixtures = super::fixtures::all();
    for (name, props) in &fixtures {
        let rendered = if *name == "layout" {
            harness.state.templates.render_document(props)
        } else {
            harness.state.templates.render(name, props)
        };
        match rendered {
            Ok(html) => assert!(!html.is_empty(), "{name} rendered empty"),
            Err(error) => panic!("{error}"),
        }
    }

    let mut on_disk = Vec::new();
    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if entry.file_type().is_file()
            && entry.path().extension().is_some_and(|ext| ext == "tsx")
            && let Ok(relative) = entry.path().strip_prefix(&root)
        {
            on_disk.push(relative.with_extension("").to_string_lossy().into_owned());
        }
    }
    on_disk.sort();
    let mut covered: Vec<String> = fixtures
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    covered.sort();
    assert_eq!(on_disk, covered, "every template needs exactly one fixture");
}

#[tokio::test]
async fn root_page_has_shell_contract() {
    let harness = harness();
    let (status, _, body) = get(&harness, "/", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("<!DOCTYPE html>"), "{body}");
    for needle in [
        "id=\"main\"",
        "id=\"toasts\"",
        "id=\"dialogs\"",
        "hx-sse:connect=\"/webui/events\"",
        "/static/app.css",
        "/static/htmax.min.js",
    ] {
        assert!(body.contains(needle), "missing {needle}");
    }
}

#[tokio::test]
async fn session_page_fragment_vs_full() {
    let harness = harness();
    let session = create_session(&harness);
    let uri = format!("/sessions/{}", session.id);

    let (status, _, fragment) = get(&harness, &uri, &[("hx-request", "true")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!fragment.starts_with("<!DOCTYPE"), "{fragment}");
    assert!(fragment.contains("id=\"session-detail\""));
    assert!(fragment.contains("<hx-partial id=\"session-list\""));

    let (status, _, full) = get(&harness, &uri, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(full.starts_with("<!DOCTYPE html>"));
}

#[tokio::test]
async fn unknown_session_is_404() {
    let harness = harness();
    let (status, _, _) = get(&harness, "/sessions/nope", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = get(&harness, "/no-such-page", &[]).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn run_all_provider_failure_reaches_connected_sse_client() {
    let _process_lock = crate::test_support::lock_process();
    let harness = harness();
    let config_home = harness.dir.path().join("xdg-config");
    let config_dir = config_home.join("cruise");
    if let Err(error) = std::fs::create_dir_all(&config_dir) {
        panic!("create config dir: {error}");
    }
    if let Err(error) = std::fs::write(config_dir.join("config.json"), "not-json") {
        panic!("write invalid config: {error}");
    }
    let _xdg = crate::test_support::EnvGuard::set("XDG_CONFIG_HOME", config_home.as_os_str());
    create_session(&harness);

    let Ok(request) = Request::builder().uri("/webui/events").body(Body::empty()) else {
        panic!("SSE request");
    };
    let response = match router(harness.state.clone()).oneshot(request).await {
        Ok(response) => response,
        Err(error) => match error {},
    };
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();

    let (_, _, body) = post(&harness, "/webui/run-all", "").await;
    assert!(body.contains("id=\"run-all-view\""), "{body}");

    let mut payload = String::new();
    while !payload.contains("hx-target=\"#run-all-view\"") || !payload.contains("event: notify") {
        let chunk =
            match tokio::time::timeout(std::time::Duration::from_secs(5), stream.next()).await {
                Ok(Some(Ok(chunk))) => chunk,
                Ok(Some(Err(error))) => panic!("SSE body error: {error}"),
                Ok(None) => panic!("SSE stream closed before Run All failure"),
                Err(error) => panic!("timed out waiting for Run All failure event: {error}"),
            };
        payload.push_str(&String::from_utf8_lossy(&chunk));
    }

    assert!(payload.contains("hx-swap=\"outerHTML\""), "{payload}");
    assert!(payload.contains(">Error</span>"), "{payload}");
    assert!(payload.contains("invalid config JSON"), "{payload}");
    assert!(
        payload.contains("Failed -- invalid config JSON"),
        "{payload}"
    );
}

#[tokio::test]
async fn action_error_is_toast_partial() {
    let harness = harness();
    let (status, _, body) = post(&harness, "/webui/sessions/nope/approve", "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("hx-target=\"#toasts\""), "{body}");
    assert!(!body.contains("id=\"session-header"), "{body}");
}

#[tokio::test]
async fn css_endpoint_serves_generated_css() {
    let harness = harness();
    let (status, content_type, body) = get(&harness, "/static/app.css", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/css"), "{content_type}");
    assert!(body.contains(".prose"), "handwritten extras missing");
    assert!(body.contains(".h-screen"), "layout utility missing");
}

#[tokio::test]
async fn static_htmax_served() {
    let harness = harness();
    let (status, content_type, _) = get(&harness, "/static/htmax.min.js", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.contains("javascript"), "{content_type}");
}

#[tokio::test]
async fn preview_rejects_paths_outside_uploads_dir() {
    let harness = harness();
    let (status, _, _) = get(
        &harness,
        "/webui/attachments/preview?path=%2Fetc%2Fhosts",
        &[],
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND,
        "{status}"
    );
}

#[tokio::test]
async fn new_and_run_all_pages_render() {
    let harness = harness();
    let (status, _, body) = get(&harness, "/new", &[("hx-request", "true")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("id=\"new-session-form\""), "{body}");

    let (status, _, body) = get(&harness, "/run-all", &[("hx-request", "true")]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("id=\"run-all-view\""), "{body}");
}

#[tokio::test]
async fn session_tabs_render_their_contract_ids() {
    let harness = harness();
    let session = create_session(&harness);
    for (tab, needle) in [
        ("info", "id=\"tab-info-"),
        ("plan", "id=\"tab-plan-"),
        ("dag", "id=\"tab-dag-"),
        ("log", "id=\"tab-log-"),
    ] {
        let uri = format!("/webui/sessions/{}/tab/{tab}", session.id);
        let (status, _, body) = get(&harness, &uri, &[]).await;
        assert_eq!(status, StatusCode::OK, "{tab}");
        assert!(body.contains(needle), "{tab}: {body}");
    }
}

mod event_partials {
    use super::{create_session, harness};
    use crate::application::{
        ApplicationEvent, EventStream, OptionChoiceKind, OptionChoicePayload,
    };
    use crate::webui::partials::{SseMessage, for_event};

    fn html_of(messages: &[SseMessage]) -> String {
        messages
            .iter()
            .filter_map(|message| match message {
                SseMessage::Html(html) => Some(html.clone()),
                SseMessage::Notify { .. } => None,
            })
            .collect()
    }

    #[test]
    fn log_chunk_targets_the_run_all_log_and_escapes_markup() {
        let harness = harness();
        let messages = for_event(
            &harness.state,
            &ApplicationEvent::LogChunk {
                session_id: None,
                stream: EventStream::Stdout,
                text: "<script>".to_string(),
                batch: true,
            },
        );
        let html = html_of(&messages);
        assert!(html.contains("hx-target=\"#run-all-log\""), "{html}");
        assert!(html.contains("&lt;script&gt;"), "{html}");
    }

    #[test]
    fn option_required_opens_a_dialog_in_the_dialogs_container() {
        let harness = harness();
        let session = create_session(&harness);
        let messages = for_event(
            &harness.state,
            &ApplicationEvent::OptionRequired {
                session_id: session.id.clone(),
                request_id: "r1".to_string(),
                prompt: "Pick one".to_string(),
                choices: vec![OptionChoicePayload {
                    label: "Continue".to_string(),
                    kind: OptionChoiceKind::Selector,
                    next_step: Some("s1".to_string()),
                }],
            },
        );
        let html = html_of(&messages);
        assert!(html.contains("hx-target=\"#dialogs\""), "{html}");
        assert!(html.contains("id=\"dialog-r1\""), "{html}");
    }

    #[test]
    fn ask_user_required_fills_the_ask_panel_and_notifies() {
        let harness = harness();
        let session = create_session(&harness);
        let messages = for_event(
            &harness.state,
            &ApplicationEvent::AskUserRequired {
                session_id: session.id.clone(),
                request_id: "r1".to_string(),
                question: "Which database?".to_string(),
            },
        );
        let html = html_of(&messages);
        assert!(
            html.contains(&format!("id=\"ask-panel-{}\"", session.id)),
            "{html}"
        );
        assert!(messages.iter().any(|message| matches!(
            message,
            SseMessage::Notify { body, .. } if body.contains("Action required")
        )));
    }

    #[test]
    fn plan_finished_awaiting_approval_notifies_plan_ready() {
        let harness = harness();
        let session = create_session(&harness);
        let messages = for_event(
            &harness.state,
            &ApplicationEvent::PlanFinished {
                session_id: session.id.clone(),
                phase: "Awaiting Approval".to_string(),
            },
        );
        assert!(messages.iter().any(|message| matches!(
            message,
            SseMessage::Notify { body, .. } if body.contains("Plan ready")
        )));
    }
}
