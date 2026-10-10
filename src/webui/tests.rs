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
fn publish_dialog_hides_trigger_checkbox_when_unsupported() {
    let harness = harness();
    let render = |supports: bool| {
        harness
            .state
            .templates
            .render(
                "publish-dialog",
                &serde_json::json!({
                    "id": "s1",
                    "submitUrl": "/webui/sessions/s1/publish",
                    "supportsTrigger": supports,
                }),
            )
            .unwrap_or_else(|error| panic!("{error}"))
    };
    assert!(render(true).contains("triggerCruise"));
    assert!(!render(false).contains("triggerCruise"));
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
            let name = relative
                .with_extension("")
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            on_disk.push(name);
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

#[test]
fn header_omits_input_paragraph_for_input_as_plan_session() {
    let harness = harness();
    let session = create_session(&harness);
    let manager = SessionManager::new(harness.dir.path().to_path_buf());
    let mut stored = manager.load(&session.id).unwrap_or_else(|e| panic!("{e}"));
    stored.input = String::new();
    stored.input_as_plan = true;
    stored.title = Some("plan title".to_string());
    manager.save(&stored).unwrap_or_else(|e| panic!("{e}"));

    let html = super::partials::header_html(&harness.state, &session.id, None)
        .unwrap_or_else(|e| panic!("{e}"));

    assert!(html.contains("plan title"), "{html}");
    assert!(!html.contains("<p "), "{html}");
}

#[test]
fn header_shows_input_paragraph_for_normal_session() {
    let harness = harness();
    let session = create_session(&harness);

    let html = super::partials::header_html(&harness.state, &session.id, None)
        .unwrap_or_else(|e| panic!("{e}"));

    assert!(html.contains("<p "), "{html}");
    assert!(html.contains("add a webui"), "{html}");
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

#[cfg(unix)]
mod merge_pr {
    use super::*;
    use crate::pr_merge::fake_gh::{FakeGh, view_json};

    const URL: &str = "https://github.com/owner/repo/pull/8";

    struct Merge {
        harness: Harness,
        session: SessionState,
        gh: FakeGh,
        _lock: crate::test_support::ProcessLock,
        _path: crate::test_support::EnvGuard,
    }

    fn merge_harness(phase: crate::session::SessionPhase, pr_url: Option<&str>) -> Merge {
        let lock = crate::test_support::lock_process();
        let harness = harness();
        let gh = FakeGh::install(harness.dir.path());
        gh.set(
            "view.out",
            r#"{"state":"OPEN","mergeable":"MERGEABLE","reviewDecision":"APPROVED","statusCheckRollup":[{"__typename":"CheckRun","name":"unit-tests","status":"COMPLETED","conclusion":"SUCCESS"}]}"#,
        );
        let path = crate::test_support::prepend_to_path(&gh.bin());
        let mut session = create_session(&harness);
        session.phase = phase;
        session.pr_url = pr_url.map(str::to_string);
        let manager = SessionManager::new(harness.dir.path().to_path_buf());
        let Ok(()) = manager.save(&session) else {
            panic!("save session");
        };
        Merge {
            harness,
            session,
            gh,
            _lock: lock,
            _path: path,
        }
    }

    fn completed() -> Merge {
        merge_harness(crate::session::SessionPhase::Completed, Some(URL))
    }

    fn exists(m: &Merge) -> bool {
        SessionManager::new(m.harness.dir.path().to_path_buf())
            .load(&m.session.id)
            .is_ok()
    }

    #[tokio::test]
    async fn completed_session_header_offers_merge_pr() {
        let m = completed();
        let (status, _, body) = get(&m.harness, &format!("/sessions/{}", m.session.id), &[]).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Merge PR"), "{body}");
    }

    #[tokio::test]
    async fn session_header_without_pr_has_no_merge_pr() {
        let m = merge_harness(crate::session::SessionPhase::Completed, None);
        let (_, _, body) = get(&m.harness, &format!("/sessions/{}", m.session.id), &[]).await;
        assert!(!body.contains("Merge PR"), "{body}");
    }

    #[tokio::test]
    async fn merge_preview_displays_status_before_confirm() {
        let m = completed();

        let (status, _, body) = get(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            &[],
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("MERGEABLE"), "{body}");
        assert!(body.contains("APPROVED"), "{body}");
        assert!(body.contains("unit-tests"), "{body}");
        for method in ["merge", "squash", "rebase"] {
            assert!(body.contains(method), "method {method} missing: {body}");
        }
        assert!(
            body.contains("hx-post"),
            "confirm affordance missing: {body}"
        );
        assert!(m.gh.merge_calls().is_empty(), "preview must not merge");
        assert!(exists(&m));
    }

    #[tokio::test]
    async fn merge_preview_for_non_open_pr_has_no_submit_affordance() {
        let m = completed();
        m.gh.set("view.out", &view_json("MERGED"));

        let (_, _, body) = get(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            &[],
        )
        .await;

        assert!(!body.contains("hx-post"), "{body}");
        assert_eq!(m.gh.merge_calls(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn merge_preview_without_pr_is_rejected() {
        let m = merge_harness(crate::session::SessionPhase::Completed, None);

        let (status, _, body) = get(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            &[],
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(!body.contains("hx-post"), "{body}");
    }

    #[tokio::test]
    async fn merge_post_uses_selected_method_and_removes_cleaned_session() {
        let m = completed();
        m.gh.set("view_after.out", &view_json("MERGED"));

        let (status, _, body) = post(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            "method=rebase",
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(m.gh.merge_calls(), vec![format!("pr merge {URL} --rebase")]);
        assert!(!exists(&m), "session must be cleaned");
        let (_, _, sidebar) = get(&m.harness, "/webui/sidebar", &[]).await;
        assert!(!sidebar.contains(&m.session.id), "{sidebar}");
    }

    #[tokio::test]
    async fn merge_post_pending_keeps_session() {
        let m = completed();

        let (status, _, body) = post(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            "method=squash",
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(m.gh.merge_calls(), vec![format!("pr merge {URL} --squash")]);
        assert!(exists(&m));
        let (_, _, sidebar) = get(&m.harness, "/webui/sidebar", &[]).await;
        assert!(sidebar.contains(&m.session.id), "{sidebar}");
    }

    #[tokio::test]
    async fn merge_post_rejects_unknown_or_missing_method() {
        let m = completed();
        for form in ["method=fast-forward", "method=--auto", ""] {
            let (status, _, body) = post(
                &m.harness,
                &format!("/webui/sessions/{}/merge-pr", m.session.id),
                form,
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{form}: {body}");
        }
        assert_eq!(m.gh.merge_calls(), Vec::<String>::new());
        assert!(exists(&m));
    }

    #[tokio::test]
    async fn merge_post_without_pr_is_rejected_without_merging() {
        let m = merge_harness(crate::session::SessionPhase::Completed, None);

        let (status, _, _) = post(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            "method=squash",
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(m.gh.merge_calls(), Vec::<String>::new());
        assert!(exists(&m));
    }

    #[tokio::test]
    async fn merge_post_gh_failure_keeps_session_and_reports_error() {
        let m = completed();
        m.gh.set("merge.exit", "1");

        let (status, _, body) = post(
            &m.harness,
            &format!("/webui/sessions/{}/merge-pr", m.session.id),
            "method=squash",
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(exists(&m));
    }
}
