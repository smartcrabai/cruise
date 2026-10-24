#![cfg(unix)]

//! Black-box V2 contracts. The shell executable stands in only for CLI lifetime.
//! HTTP and SSE use the pinned protocol at 74f8d4860f49c563909e6b039b0cc8b454ebce72:
//! packages/protocol/src/groups/{server,session,event}.ts and
//! packages/schema/src/{session,session-event,session-inbox,event-log}.ts.
//! No real `OpenCode` installation, provider credentials, or paid model is used.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use cruise::cancellation::CancellationToken;
use cruise::config::{PermissionMode, RetryConfig};
use cruise::executor::{Executor, PromptOutcome, PromptRun, SessionIdCallback};
use cruise::retry::{policy_for_config, with_active_policy};
use cruise::test_support::{EnvGuard, lock_process, write_executable_script};
use futures::StreamExt as _;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::broadcast;

pub(crate) const ANSWER: &str = "V2 response: \u{65e5}\u{672c}\u{8a9e}\n";
pub(crate) const MODEL: &str = "fixture/team/model:2026#precise";

#[derive(Clone, Copy)]
pub(crate) enum Scenario {
    Complete,
    Recover,
    Incomplete,
    LogUnavailable,
    Malformed,
    UncertainAcceptance,
    HandshakeFailure(StatusCode),
    Pending,
    PluginDisabled,
    ProviderFailure { status: u16, side_effect: bool },
    UndeliveredFailure { settle: bool },
    RetainedRecovery { start: bool, terminal: bool },
    BoundaryUnavailable,
}

struct ObservedRequest {
    method: Method,
    path: String,
    body: Value,
    authorization: Option<String>,
}

#[derive(Default)]
struct Observations {
    requests: Vec<ObservedRequest>,
    session_id: String,
    durable: Vec<Value>,
    checkpoint_at_prompt: Option<String>,
    pending: Option<String>,
}

struct Server {
    scenario: Scenario,
    root: PathBuf,
    repo: PathBuf,
    url: String,
    observations: Mutex<Observations>,
    events: broadcast::Sender<Option<String>>,
}

fn locked(server: &Server) -> std::sync::MutexGuard<'_, Observations> {
    server
        .observations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn server_prompt_count(server: &Server) -> usize {
    locked(server)
        .requests
        .iter()
        .filter(|request| request.method == Method::POST && request.path.ends_with("/prompt"))
        .count()
}

async fn report_tool_effect(server: &Server) {
    let overlay: Value = serde_json::from_slice(
        &std::fs::read(server.root.join("cli.overlay"))
            .unwrap_or_else(|error| panic!("CLI overlay: {error}")),
    )
    .unwrap_or_else(|error| panic!("CLI overlay JSON: {error}"));
    let plugin = overlay["plugins"]
        .as_array()
        .and_then(|plugins| plugins.last())
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("native plugin missing"));
    let bridge: Value = serde_json::from_slice(
        &std::fs::read(Path::new(plugin).join("bridge.json"))
            .unwrap_or_else(|error| panic!("bridge manifest: {error}")),
    )
    .unwrap_or_else(|error| panic!("bridge manifest JSON: {error}"));
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap_or_else(|error| panic!("bridge client: {error}"))
        .post(format!(
            "{}/effect",
            bridge["url"].as_str().unwrap_or_default()
        ))
        .header(
            "authorization",
            bridge["token"].as_str().unwrap_or_default(),
        )
        .json(&json!({"sessionID":bridge["sessionID"],"callID":"call_shell_effect","tool":"bash"}))
        .send()
        .await
        .unwrap_or_else(|error| panic!("tool effect: {error}"));
    assert!(
        response.status().is_success(),
        "tool effect was not admitted"
    );
}

fn frame(value: &Value) -> String {
    // JSON whitespace after a comma is legal across multiple SSE data lines.
    // CRLF and a comment exercise framing without changing the V2 payload.
    let json = serde_json::to_string(value).unwrap_or_else(|error| panic!("event: {error}"));
    format!(
        ": heartbeat\r\ndata: {}\r\n\r\n",
        json.replacen(',', ",\r\ndata: ", 1)
    )
}

fn event(kind: &str, session: &str, extra: Value, sequence: u64) -> Value {
    let Value::Object(extra) = extra else {
        panic!("event fields must be an object");
    };
    let mut data = json!({"sessionID": session});
    data.as_object_mut()
        .unwrap_or_else(|| panic!("event data object"))
        .extend(extra);
    let mut value = json!({
        "id": format!("evt_fixture_{sequence}"),
        "created": 1_792_749_600_000_u64,
        "type": kind,
        "data": data,
    });
    if !matches!(kind, "session.text.delta" | "session.reasoning.delta") {
        value["durable"] = json!({"aggregateID": session, "seq": sequence, "version": 1});
    }
    value
}

fn turn(session: &str, inbox: &str) -> Vec<Value> {
    vec![
        event("session.execution.started", session, json!({}), 1),
        event(
            "session.inbox.delivered",
            session,
            json!({"inboxID": inbox}),
            2,
        ),
        event(
            "session.text.started",
            session,
            json!({"assistantMessageID": "msg_answer", "ordinal": 0}),
            3,
        ),
        event(
            "session.text.delta",
            session,
            json!({"assistantMessageID": "msg_answer", "ordinal": 0, "delta": ANSWER}),
            100,
        ),
        event(
            "session.text.ended",
            session,
            json!({"assistantMessageID": "msg_answer", "ordinal": 0, "text": ANSWER}),
            4,
        ),
        event("session.execution.succeeded", session, json!({}), 5),
    ]
}

fn session_info(server: &Server, id: &str) -> Value {
    json!({
        "id": id,
        "projectID": "prj_fixture",
        "cost": 0,
        "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}},
        "time": {"created": 1_792_749_600_000_u64, "updated": 1_792_749_600_000_u64},
        "location": {"directory": server.repo},
    })
}

#[expect(
    clippy::too_many_lines,
    reason = "the pinned fixture dispatch keeps endpoint contracts together"
)]
async fn serve(State(server): State<Arc<Server>>, request: Request) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let after = request
        .uri()
        .query()
        .and_then(|query| {
            url::form_urlencoded::parse(query.as_bytes())
                .find(|(key, _)| key == "after")
                .and_then(|(_, value)| value.parse::<u64>().ok())
        })
        .unwrap_or(0);
    let authorization = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = to_bytes(request.into_body(), 1_048_576)
        .await
        .unwrap_or_default();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| panic!("request JSON: {error}"))
    };
    locked(&server).requests.push(ObservedRequest {
        method: method.clone(),
        path: path.clone(),
        body: body.clone(),
        authorization,
    });
    match (method, path.as_str()) {
        (Method::GET, "/api/plugin") => {
            if matches!(server.scenario, Scenario::PluginDisabled) {
                return Json(json!({"location":{"directory":server.repo},"data":[]}))
                    .into_response();
            }
            // V2 lazily initializes project plugins through this endpoint.
            // The fake CLI publishes its actual generated overlay, so readiness
            // still crosses the real token/session-authenticated Rust bridge.
            let overlay: Value = serde_json::from_slice(
                &std::fs::read(server.root.join("cli.overlay"))
                    .unwrap_or_else(|error| panic!("CLI overlay: {error}")),
            )
            .unwrap_or_else(|error| panic!("CLI overlay JSON: {error}"));
            let plugin = overlay["plugins"]
                .as_array()
                .and_then(|plugins| plugins.last())
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("mandatory native plugin missing from overlay"));
            let bridge: Value = serde_json::from_slice(
                &std::fs::read(Path::new(plugin).join("bridge.json"))
                    .unwrap_or_else(|error| panic!("bridge manifest: {error}")),
            )
            .unwrap_or_else(|error| panic!("bridge manifest JSON: {error}"));
            let response = reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap_or_else(|error| panic!("bridge client: {error}"))
                .post(format!(
                    "{}/ready",
                    bridge["url"].as_str().unwrap_or_default()
                ))
                .header(
                    "authorization",
                    bridge["token"].as_str().unwrap_or_default(),
                )
                .json(&json!({"sessionID":bridge["sessionID"]}))
                .send()
                .await
                .unwrap_or_else(|error| panic!("bridge readiness: {error}"));
            assert!(
                response.status().is_success(),
                "authenticated plugin readiness rejected"
            );
            Json(json!({"location":{"directory":server.repo},"data": []})).into_response()
        }
        (Method::GET, "/api/info") => {
            if let Scenario::HandshakeFailure(status) = server.scenario {
                return (
                    status,
                    Json(json!({"message": "fixture local API failure"})),
                )
                    .into_response();
            }
            let pid = std::fs::read_to_string(server.root.join("cli.pid"))
                .unwrap_or_default()
                .trim()
                .parse::<u32>()
                .unwrap_or_default();
            Json(json!({"version": "2.1.3", "pid": pid, "urls": [server.url], "paths": {"tmp": server.root.join("tmp")}})).into_response()
        }
        (Method::POST, "/api/session") => {
            let id = body["id"].as_str().unwrap_or("ses_fixture").to_owned();
            let mut observations = locked(&server);
            observations.session_id.clone_from(&id);
            observations.durable.clear();
            drop(observations);
            Json(json!({"data": session_info(&server, &id)})).into_response()
        }
        (Method::GET, "/api/event") => {
            let receiver = server.events.subscribe();
            let connected = futures::stream::once(async {
                Ok::<_, Infallible>(frame(
                    &json!({"id":"evt_connected", "type":"server.connected", "data":{}}),
                ))
            });
            let stream = futures::stream::unfold(receiver, |mut receiver| async move {
                match receiver.recv().await {
                    Ok(Some(value)) => Some((Ok::<_, Infallible>(value), receiver)),
                    _ => None,
                }
            });
            let fragmented = connected.chain(stream).flat_map(|part| {
                let mut bytes = part.unwrap_or_else(|never| match never {}).into_bytes();
                let split = bytes
                    .iter()
                    .position(|byte| !byte.is_ascii())
                    .map_or(bytes.len() / 2, |index| index + 1);
                let remainder = bytes.split_off(split);
                futures::stream::iter([Ok::<_, Infallible>(bytes), Ok(remainder)])
            });
            (
                [("content-type", "text/event-stream")],
                Body::from_stream(fragmented),
            )
                .into_response()
        }
        (method, _) => {
            if method == Method::POST
                && path.ends_with("/prompt")
                && matches!(
                    server.scenario,
                    Scenario::ProviderFailure {
                        side_effect: true,
                        ..
                    }
                )
            {
                report_tool_effect(&server).await;
            }
            session_route(&server, &method, &path, &body, after)
        }
    }
}

fn prompt_events(server: &Server, session: &str, inbox: &str) -> Vec<Value> {
    if let Scenario::RetainedRecovery { start, terminal } = server.scenario
        && server_prompt_count(server) > 1
    {
        let mut events = turn(session, inbox);
        if !start {
            events.clear();
        } else if !terminal {
            events.retain(|value| value["type"] != "session.execution.succeeded");
        }
        return events;
    }
    if matches!(server.scenario, Scenario::UndeliveredFailure { .. })
        && server_prompt_count(server) == 1
    {
        locked(server).pending = Some(inbox.to_owned());
        return vec![
            event("session.execution.started", session, json!({}), 1),
            event(
                "session.execution.failed",
                session,
                json!({"error":{
                    "type":"provider.api", "message":"context preparation failed", "status":503,
                }}),
                2,
            ),
        ];
    }
    if let Scenario::ProviderFailure { status, .. } = server.scenario
        && server_prompt_count(server) == 1
    {
        vec![
            event("session.execution.started", session, json!({}), 1),
            event(
                "session.inbox.delivered",
                session,
                json!({"inboxID":inbox}),
                2,
            ),
            event(
                "session.execution.failed",
                session,
                json!({"error":{
                    "type":if status == 429 {"provider.rate-limit"} else {"provider.api"},
                    "message":"fixture provider failure", "status":status, "retryAfterMs":1,
                }}),
                3,
            ),
        ]
    } else {
        let mut events = turn(session, inbox);
        if matches!(server.scenario, Scenario::Incomplete) {
            // Acceptance, full text and a successful durable synchronization
            // still cannot replace execution terminal evidence.
            events.retain(|value| value["type"] != "session.execution.succeeded");
        }
        events
    }
}

fn session_route(
    server: &Server,
    method: &Method,
    path: &str,
    body: &Value,
    after: u64,
) -> Response {
    let id = locked(server).session_id.clone();
    let session_path = format!("/api/session/{id}");
    if method == Method::GET && path == session_path {
        return Json(json!({"data": session_info(server, &id)})).into_response();
    }
    if method == Method::PATCH && path == session_path {
        return StatusCode::NO_CONTENT.into_response();
    }
    if (method == Method::POST && path == format!("{session_path}/model"))
        || (method == Method::PUT && path == format!("{session_path}/environment"))
    {
        return StatusCode::NO_CONTENT.into_response();
    }
    if method == Method::POST && path == format!("{session_path}/interrupt") {
        return interrupt_response(server, &id);
    }
    if method == Method::GET && path == format!("/api/experimental/session/{id}/log") {
        return durable_response(server, &id, after);
    }
    if method == Method::GET && path == format!("{session_path}/inbox") {
        if let Some(inbox) = &locked(server).pending {
            return Json(json!({"data": [{"id":inbox}]})).into_response();
        }
        return Json(json!({"data": []})).into_response();
    }
    if method == Method::DELETE && path.starts_with(&format!("{session_path}/inbox/")) {
        return cancel_inbox_response(server, &id);
    }
    if method == Method::POST && path == format!("{session_path}/prompt") {
        return prompt_response(server, &id, body);
    }
    (
        StatusCode::NOT_FOUND,
        Json(json!({"_tag": "UnknownError", "message": "unsupported fixture endpoint"})),
    )
        .into_response()
}

fn prompt_response(server: &Server, id: &str, body: &Value) -> Response {
    locked(server).checkpoint_at_prompt =
        std::fs::read_to_string(server.root.join("checkpoint")).ok();
    if matches!(server.scenario, Scenario::UncertainAcceptance) {
        // The server has durably accepted the input but the HTTP response
        // body is lost. A retry here would duplicate an external side effect.
        let broken = futures::stream::once(async {
            Err::<String, _>(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "accepted response lost",
            ))
        });
        return (
            [("content-type", "application/json")],
            Body::from_stream(broken),
        )
            .into_response();
    }
    let inbox = body["id"].as_str().unwrap_or("msg_input");
    let mut events = prompt_events(server, id, inbox);
    let boundary = locked(server)
        .durable
        .last()
        .and_then(|value| value["durable"]["seq"].as_u64())
        .unwrap_or(0);
    for value in &mut events {
        if let Some(seq) = value["durable"]["seq"].as_u64() {
            value["durable"]["seq"] = json!(seq + boundary);
        }
    }
    locked(server).durable.extend(
        events
            .iter()
            .filter(|value| {
                value.get("durable").is_some()
                    && !(matches!(server.scenario, Scenario::Pending)
                        && matches!(
                            value["type"].as_str(),
                            Some("session.text.ended" | "session.execution.succeeded")
                        ))
            })
            .cloned(),
    );
    send_non_answer_events(server, id);
    for value in &events {
        if matches!(server.scenario, Scenario::RetainedRecovery { .. })
            && server_prompt_count(server) > 1
        {
            continue;
        }
        let kind = value["type"].as_str().unwrap_or_default();
        if matches!(server.scenario, Scenario::Malformed) && kind == "session.execution.succeeded" {
            continue;
        }
        if matches!(
            server.scenario,
            Scenario::Recover | Scenario::LogUnavailable | Scenario::Pending
        ) && matches!(kind, "session.text.ended" | "session.execution.succeeded")
        {
            continue;
        }
        let mut live = value.clone();
        if matches!(server.scenario, Scenario::Recover) && kind == "session.text.delta" {
            live["data"]["delta"] = json!("V2 response: \u{65e5}");
        }
        let _ = server.events.send(Some(frame(&live)));
    }
    if matches!(server.scenario, Scenario::Malformed) {
        // Use a known mandatory event with invalid JSON, not an unknown event.
        let _ = server.events.send(Some(
            "data: {\"type\":\"session.execution.succeeded\",broken}\n\n".to_owned(),
        ));
    }
    if !matches!(
        server.scenario,
        Scenario::Complete | Scenario::Pending | Scenario::UndeliveredFailure { .. }
    ) {
        let _ = server.events.send(None);
    }
    Json(json!({"data": {
        "id": inbox, "sessionID": id, "time": {"created": 1_792_749_600_000_u64},
        "type": "user", "payload": {"text": body["text"]}, "delivery": "queue",
    }}))
    .into_response()
}

fn cancel_inbox_response(server: &Server, id: &str) -> Response {
    if matches!(
        server.scenario,
        Scenario::UndeliveredFailure { settle: true }
    ) {
        let mut observations = locked(server);
        if let Some(inbox) = observations.pending.take() {
            observations.durable.push(event(
                "session.inbox.cancelled",
                id,
                json!({"inboxID":inbox}),
                3,
            ));
        }
        return StatusCode::NO_CONTENT.into_response();
    }
    StatusCode::SERVICE_UNAVAILABLE.into_response()
}

fn interrupt_response(server: &Server, id: &str) -> Response {
    if matches!(server.scenario, Scenario::Pending) {
        let interrupted = event(
            "session.execution.interrupted",
            id,
            json!({"reason": "user"}),
            4,
        );
        locked(server).durable.push(interrupted.clone());
        let _ = server.events.send(Some(frame(&interrupted)));
        return Json(json!({"interrupted": true})).into_response();
    }
    Json(json!({"interrupted": false})).into_response()
}

fn send_non_answer_events(server: &Server, id: &str) {
    let foreign = event(
        "session.text.delta",
        "ses_unrelated",
        json!({"assistantMessageID": "msg_foreign", "ordinal": 0, "delta": "FOREIGN_TEXT"}),
        9,
    );
    let _ = server.events.send(Some(frame(&foreign)));
    let _ = server.events.send(Some(frame(&json!({
        "id": "evt_future", "created": 1_792_749_600_000_u64,
        "type": "future.fixture.notification", "data": {"text": "UNKNOWN_EVENT_TEXT"}
    }))));
    let reasoning = event(
        "session.reasoning.delta",
        id,
        json!({"assistantMessageID": "msg_answer", "ordinal": 0, "delta": "PRIVATE_REASONING"}),
        101,
    );
    let _ = server.events.send(Some(frame(&reasoning)));
}

fn durable_response(server: &Server, id: &str, after: u64) -> Response {
    if matches!(server.scenario, Scenario::BoundaryUnavailable)
        || (server_prompt_count(server) > 0
            && matches!(
                server.scenario,
                Scenario::LogUnavailable | Scenario::UncertainAcceptance | Scenario::Malformed
            ))
    {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let observations = locked(server);
    let mut content: String = observations
        .durable
        .iter()
        .filter(|value| {
            value["durable"]["seq"]
                .as_u64()
                .is_some_and(|seq| seq > after)
        })
        .map(frame)
        .collect();
    let sequence = observations
        .durable
        .last()
        .map(|value| &value["durable"]["seq"]);
    let mut synced = json!({"type": "log.synced", "aggregateID": id});
    if let Some(sequence) = sequence {
        synced["seq"] = sequence.clone();
    }
    content.push_str(&frame(&synced));
    ([("content-type", "text/event-stream")], content).into_response()
}

pub(crate) struct Fixture {
    root: TempDir,
    server: Arc<Server>,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    pub(crate) async fn new(scenario: Scenario) -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap_or_else(|error| panic!("repo: {error}"));
        cruise::test_support::init_git_repo(&repo);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap_or_else(|error| panic!("listen: {error}"));
        let url = format!(
            "http://{}",
            listener
                .local_addr()
                .unwrap_or_else(|error| panic!("local address: {error}"))
        );
        let (events, _) = broadcast::channel(64);
        let server = Arc::new(Server {
            scenario,
            root: root.path().to_owned(),
            repo,
            url,
            observations: Mutex::new(Observations::default()),
            events,
        });
        let router = Router::new()
            .fallback(serve)
            .with_state(Arc::clone(&server));
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .unwrap_or_else(|error| panic!("fixture server: {error}"));
        });
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap_or_else(|error| panic!("bin: {error}"));
        let script = format!(
            "#!/bin/sh\nif [ \"${{1-}}\" = --version ]; then printf 'opencode v2.1.3\\n'; exit 0; fi\numask 077\nif [ -f {pids} ]; then while IFS= read -r previous; do if kill -0 \"$previous\" 2>/dev/null; then printf '%s\\n' \"$previous $$\" >> {overlap}; fi; done < {pids}; fi\nprintf '%s\\n' \"$$\" >> {pids}\nprintf '%s\\n' \"$$\" > {pid}\nprintf '%s\\n' \"$@\" > {args}\nprintf '%s' \"${{OPENCODE_SERVER_PASSWORD-}}\" > {password}\nprintf '%s' \"${{OPENCODE_CONFIG_CONTENT-}}\" > {overlay}\ntrap 'exit 0' INT TERM\nprintf '%s\\n' '{startup}'\nwhile IFS= read -r line; do :; done\n",
            pid = quote(&root.path().join("cli.pid")),
            pids = quote(&root.path().join("cli.pids")),
            overlap = quote(&root.path().join("cli.overlap")),
            args = quote(&root.path().join("cli.args")),
            password = quote(&root.path().join("cli.password")),
            overlay = quote(&root.path().join("cli.overlay")),
            startup = json!({"url": server.url}),
        );
        write_executable_script(&bin.join("opencode"), &script);
        for agent in ["jcode", "claude"] {
            write_executable_script(&bin.join(agent), "#!/bin/sh\nexit 97\n");
        }
        Self { root, server, task }
    }

    fn path(&self) -> std::ffi::OsString {
        let mut paths = vec![self.root.path().join("bin")];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        std::env::join_paths(paths).unwrap_or_else(|error| panic!("PATH: {error}"))
    }

    pub(crate) fn guards(&self) -> Vec<EnvGuard> {
        vec![
            EnvGuard::set("PATH", self.path()),
            EnvGuard::set("HOME", self.root.path().join("home")),
            EnvGuard::set("JCODE_HOME", self.root.path().join("jcode-home")),
            EnvGuard::set("CLAUDE_CONFIG_DIR", self.root.path().join("claude-home")),
            EnvGuard::set("XDG_CONFIG_HOME", self.root.path().join("config")),
            EnvGuard::set("XDG_DATA_HOME", self.root.path().join("data")),
            EnvGuard::set("XDG_STATE_HOME", self.root.path().join("state")),
            EnvGuard::remove("OPENCODE_CONFIG_CONTENT"),
            EnvGuard::remove("OPENCODE_CONFIG"),
            EnvGuard::remove("OPENCODE_CONFIG_DIR"),
            EnvGuard::remove("OPENCODE_SERVER_PASSWORD"),
        ]
    }

    pub(crate) async fn execute(
        &self,
        model: Option<&str>,
        max_retries: usize,
    ) -> cruise::error::Result<PromptOutcome> {
        self.execute_with_checkpoint(model, max_retries, None, None)
            .await
    }

    pub(crate) async fn execute_with_checkpoint(
        &self,
        model: Option<&str>,
        max_retries: usize,
        on_session_id: Option<&SessionIdCallback<'_>>,
        cancel_token: Option<&CancellationToken>,
    ) -> cruise::error::Result<PromptOutcome> {
        let env = HashMap::from([("WORKFLOW_VALUE".to_owned(), "step-value".to_owned())]);
        let mcp_servers = cruise::config::McpServers::new();
        let model = model.map(|model| cruise::config::ModelSpec::Single(model.to_owned()));
        let request = PromptRun {
            prompt: "fixture prompt",
            model: model.as_ref(),
            max_retries,
            env: &env,
            mcp_servers: &mcp_servers,
            on_notice: None,
            cancel_token,
            working_dir: Some(&self.server.repo),
            stream: None,
            tools: vec![],
            on_session_id,
            resume: None,
            computer_use: false,
            permission: PermissionMode::Full,
        };
        let config: RetryConfig = serde_yaml::from_str("base_delay_ms: 1\nmodel_fallback: true\nfallback_chains:\n  default: [fixture/fallback]\n").unwrap_or_else(|error| panic!("retry config: {error}"));
        tokio::time::timeout(
            Duration::from_secs(15),
            with_active_policy(
                policy_for_config((max_retries > 0).then_some(config)),
                Executor::new(Some("opencode"), &[]).run(request),
            ),
        )
        .await
        .unwrap_or_else(|_| panic!("OpenCode request did not finish within fixture deadline"))
    }

    pub(crate) fn assert_stopped(&self) {
        assert!(self.launch_count() > 0, "OpenCode CLI must have launched");
        assert!(
            !self.root.path().join("cli.overlap").exists(),
            "a successor CLI started before its predecessor was reaped"
        );
        let pids = std::fs::read_to_string(self.root.path().join("cli.pids"))
            .unwrap_or_else(|error| panic!("OpenCode CLI must have launched: {error}"));
        assert_ne!(pids.trim(), "");
        for pid in pids.lines() {
            let pid: i32 = pid
                .parse()
                .unwrap_or_else(|error| panic!("CLI pid: {error}"));
            // SAFETY: signal zero only observes the fixture-owned process.
            let status = unsafe { libc::kill(pid, 0) };
            assert_eq!(
                status, -1,
                "every attempt's CLI must be reaped before execution returns"
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    }

    pub(crate) fn prompt_count(&self) -> usize {
        server_prompt_count(&self.server)
    }

    pub(crate) fn launch_count(&self) -> usize {
        std::fs::read_to_string(self.root.path().join("cli.pids"))
            .unwrap_or_else(|error| panic!("CLI attempt ledger: {error}"))
            .lines()
            .count()
    }

    pub(crate) fn assert_interrupted(&self) {
        assert!(
            locked(&self.server)
                .requests
                .iter()
                .any(|request| request.method == Method::POST
                    && request.path.ends_with("/interrupt")),
            "cancellation must interrupt the owned V2 session"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        if let Ok(pids) = std::fs::read_to_string(self.root.path().join("cli.pids")) {
            for pid in pids.lines().filter_map(|pid| pid.parse::<i32>().ok()) {
                // SAFETY: only PIDs recorded by this test's private fake CLI.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
    }
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn started_failure_with_open_sse_settles_pending_input_before_safe_fallback() {
    let _lock = lock_process();
    for settle in [true, false] {
        let fixture = Fixture::new(Scenario::UndeliveredFailure { settle }).await;
        let _guards = fixture.guards();
        let model = format!("fixture/pre-delivery-{settle}");
        let result = tokio::time::timeout(Duration::from_secs(8), fixture.execute(Some(&model), 2))
            .await
            .unwrap_or_else(|error| panic!("pre-delivery failure hung on open SSE: {error}"));
        assert_eq!(result.is_ok(), settle, "{result:?}");
        assert_eq!(fixture.prompt_count(), if settle { 2 } else { 1 });
        assert_eq!(locked(&fixture.server).pending.is_none(), settle);
        assert!(locked(&fixture.server).requests.iter().any(|request| {
            request.method == Method::DELETE && request.path.contains("/inbox/")
        }));
        fixture.assert_interrupted();
        fixture.assert_stopped();
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_sigint_drains_each_planning_future_before_return_or_discard() {
    let _lock = lock_process();
    for action in ["planning", "title", "fix", "ask"] {
        let fixture = Fixture::new(Scenario::Pending).await;
        let cli = fixture.root.path().join("bin/opencode");
        let script = std::fs::read_to_string(&cli).unwrap_or_else(|error| panic!("{error}"));
        let delayed = format!(
            "trap 'touch {}; sleep 0.35; touch {}; exit 0' INT TERM",
            quote(&fixture.root.path().join("shutdown.started")),
            quote(&fixture.root.path().join("shutdown.finished")),
        );
        write_executable_script(&cli, &script.replace("trap 'exit 0' INT TERM", &delayed));
        let config = fixture.root.path().join("planning.yaml");
        std::fs::write(&config, format!("sdk: opencode\nmodel: '{MODEL}'\ncreate_pr: false\nsteps:\n  check:\n    prompt: check\n"))
            .unwrap_or_else(|error| panic!("{error}"));
        let runtime_pid = fixture.root.path().join("cruise.pid");
        let transcript = fixture.root.path().join("pty.output");
        let shell = format!(
            "stty cols 160 rows 40; echo $$ > {}; exec {} plan --no-force-exec {} -c {} 'fixture plan'",
            quote(&runtime_pid),
            quote(Path::new(env!("CARGO_BIN_EXE_cruise"))),
            if action == "planning" {
                ""
            } else {
                "--skip-planning"
            },
            quote(&config),
        );
        let mut command = tokio::process::Command::new("script");
        #[cfg(target_os = "macos")]
        command.args(["-q", "/dev/null", "/bin/sh", "-c", &shell]);
        #[cfg(target_os = "linux")]
        command.args(["-q", "-e", "-c", &shell, "/dev/null"]);
        for (name, _) in std::env::vars_os() {
            // The inherited Git hook and its guarded repository identity must
            // stay together, even when isolating workflow configuration overrides.
            if (name.to_string_lossy().starts_with("CRUISE_") && name != "CRUISE_GUARD_COMMON_DIR")
                || name.to_string_lossy().starts_with("OPENCODE_")
            {
                command.env_remove(name);
            }
        }
        command
            .current_dir(&fixture.server.repo)
            .env("PATH", fixture.path())
            .env("HOME", fixture.root.path().join("home"))
            .env("XDG_CONFIG_HOME", fixture.root.path().join("config"))
            .env("XDG_DATA_HOME", fixture.root.path().join("data"))
            .env("XDG_STATE_HOME", fixture.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::from(
                std::fs::File::create(&transcript).unwrap_or_else(|error| panic!("{error}")),
            ))
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .unwrap_or_else(|error| panic!("PTY: {error}"));
        let mut input = child.stdin.take().unwrap_or_else(|| panic!("PTY stdin"));
        drive_planning_prompt(&fixture, action, &transcript, &mut input).await;
        assert_planning_interrupt(&fixture, action, &runtime_pid, &transcript, &mut child).await;
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn drive_planning_prompt(
    fixture: &Fixture,
    action: &str,
    transcript: &Path,
    input: &mut tokio::process::ChildStdin,
) {
    use tokio::io::AsyncWriteExt as _;
    let mut queries = [0; 3];
    let mut selected = false;
    let mut submitted = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let raw = std::fs::read_to_string(transcript).unwrap_or_default();
        for (index, (query, answer)) in [
            ("\x1b[?u", "\x1b[?1u"),
            ("\x1b[c", "\x1b[?1;2c"),
            ("\x1b[6n", "\x1b[1;1R"),
        ]
        .into_iter()
        .enumerate()
        {
            let count = raw.matches(query).count();
            while queries[index] < count {
                input
                    .write_all(answer.as_bytes())
                    .await
                    .unwrap_or_else(|error| panic!("{error}"));
                queries[index] += 1;
            }
        }
        if action != "planning" && !selected && raw.contains("Publish as Issue") {
            let keys: &[u8] = match action {
                "title" => b"\r",
                "fix" => b"\x1b[B\r",
                _ => b"\x1b[B\x1b[B\r",
            };
            input
                .write_all(keys)
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            selected = true;
        }
        if selected
            && !submitted
            && ((action == "title" && raw.contains("Steps to skip"))
                || (action == "fix" && raw.contains("Describe the changes needed:"))
                || (action == "ask" && raw.contains("Your question:")))
        {
            // Allow the line editor to finish terminal capability negotiation.
            tokio::time::sleep(Duration::from_millis(100)).await;
            input
                .write_all(if action == "title" {
                    b"\r"
                } else {
                    b"fixture follow-up\r"
                })
                .await
                .unwrap_or_else(|error| panic!("{error}"));
            submitted = true;
        }
        if fixture.prompt_count() == 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{action} never reached prompt: {raw}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn assert_planning_interrupt(
    fixture: &Fixture,
    action: &str,
    runtime_pid: &Path,
    transcript: &Path,
    child: &mut tokio::process::Child,
) {
    let pid: i32 = std::fs::read_to_string(runtime_pid)
        .unwrap_or_else(|error| panic!("{error}"))
        .trim()
        .parse()
        .unwrap_or_else(|error| panic!("{error}"));
    // SAFETY: this PID belongs to the still-running CLI created above.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap_or_else(|error| panic!("{action} did not return: {error}"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!status.success(), "{action} must report interruption");
    let raw = std::fs::read_to_string(transcript).unwrap_or_default();
    assert!(
        fixture.root.path().join("shutdown.finished").is_file(),
        "{action} returned before shutdown: {raw}"
    );
    assert!(!raw.contains("conversation is active"), "{raw}");
    fixture.assert_stopped();
    fixture.assert_interrupted();
    if matches!(action, "planning" | "title") {
        let sessions = fixture.root.path().join("data/cruise/sessions");
        assert!(
            !sessions.exists()
                || std::fs::read_dir(sessions)
                    .unwrap_or_else(|error| panic!("{error}"))
                    .next()
                    .is_none(),
            "discarded session retained"
        );
        let private = fixture.root.path().join("data/cruise/opencode");
        assert!(
            !private.exists()
                || std::fs::read_dir(private)
                    .unwrap_or_else(|error| panic!("{error}"))
                    .next()
                    .is_none(),
            "discarded private storage retained"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exec_selects_opencode_and_completes_v2_before_reaping_cli() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Complete).await;
    let config = fixture.root.path().join("workflow.yaml");
    std::fs::write(&config, format!("sdk: opencode\nmodel: '{MODEL}'\npermission: full\nenv:\n  WORKFLOW_VALUE: workflow-value\nsteps:\n  implement:\n    env:\n      WORKFLOW_VALUE: step-value\n    prompt: '{{input}}'\n")).unwrap_or_else(|error| panic!("workflow: {error}"));
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CRUISE_")
            || name.to_string_lossy().starts_with("OPENCODE_")
        {
            command.env_remove(name);
        }
    }
    command
        .args(["exec", "-c"])
        .arg(&config)
        .arg("fixture prompt")
        .current_dir(&fixture.server.repo)
        .env("PATH", fixture.path())
        .env("HOME", fixture.root.path().join("home"))
        .env("JCODE_HOME", fixture.root.path().join("jcode-home"))
        .env("CLAUDE_CONFIG_DIR", fixture.root.path().join("claude-home"))
        .env("XDG_CONFIG_HOME", fixture.root.path().join("config"))
        .env("XDG_DATA_HOME", fixture.root.path().join("data"))
        .env("XDG_STATE_HOME", fixture.root.path().join("state"))
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env("NO_COLOR", "1")
        .env_remove("OPENCODE_CONFIG_CONTENT")
        .env_remove("OPENCODE_SERVER_PASSWORD")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(20), command.output())
        .await
        .unwrap_or_else(|_| panic!("cruise exec timed out"))
        .unwrap_or_else(|error| panic!("cruise exec: {error}"));
    let terminal = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{terminal}");
    assert_eq!(
        terminal.matches(ANSWER.trim()).count(),
        1,
        "assistant text must appear exactly once: {terminal}"
    );
    assert!(
        !terminal.contains("FOREIGN_TEXT"),
        "another session leaked: {terminal}"
    );
    assert!(
        !terminal.contains("PRIVATE_REASONING") && !terminal.contains("UNKNOWN_EVENT_TEXT"),
        "non-answer events must not become assistant text: {terminal}"
    );
    assert_eq!(fixture.prompt_count(), 1);
    fixture.assert_stopped();
    assert_v2_request_contract(&fixture, &terminal);
}

fn assert_v2_request_contract(fixture: &Fixture, terminal: &str) {
    let arguments = std::fs::read_to_string(fixture.root.path().join("cli.args"))
        .unwrap_or_else(|error| panic!("CLI arguments: {error}"));
    let arguments: Vec<_> = arguments.lines().collect();
    assert!(arguments.contains(&"serve") && arguments.contains(&"--stdio"));
    assert!(
        arguments
            .windows(2)
            .any(|pair| pair == ["--hostname", "127.0.0.1"])
    );
    assert!(arguments.windows(2).any(|pair| pair == ["--port", "0"]));
    assert!(
        !arguments.contains(&"--service"),
        "must not attach to a user's daemon"
    );
    let observations = locked(&fixture.server);
    let create = observations
        .requests
        .iter()
        .find(|request| request.method == Method::POST && request.path == "/api/session")
        .unwrap_or_else(|| panic!("V2 session was not created"));
    assert!(
        create.body["id"]
            .as_str()
            .is_some_and(|id| id.starts_with("ses")),
        "a client-generated V2 session ID is required"
    );
    let expected_model =
        json!({"providerID": "fixture", "id": "team/model:2026", "variant": "precise"});
    assert!(
        create.body.get("model") == Some(&expected_model)
            || observations
                .requests
                .iter()
                .any(|request| request.method == Method::POST
                    && request.path.ends_with("/model")
                    && request.body["model"] == expected_model),
        "model ID punctuation and variant must reach V2 unchanged"
    );
    let environment = observations
        .requests
        .iter()
        .find(|request| request.method == Method::PUT && request.path.ends_with("/environment"))
        .unwrap_or_else(|| panic!("V2 shell environment was not set"));
    assert_eq!(
        environment.body["variables"]["WORKFLOW_VALUE"],
        "step-value"
    );
    let prompt = observations
        .requests
        .iter()
        .find(|request| request.method == Method::POST && request.path.ends_with("/prompt"))
        .unwrap_or_else(|| panic!("V2 prompt missing"));
    assert!(
        prompt.body["text"]
            .as_str()
            .is_some_and(|text| text.contains("fixture prompt")),
        "the workflow input must reach the V2 prompt"
    );
    assert!(
        environment.body["variables"]["CRUISE_GUARD_COMMON_DIR"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "replacement shell environment must retain commit-guard protection"
    );
    assert!(
        environment.body["variables"]["PATH"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "replacement environment must retain PATH"
    );
    assert!(
        environment.body["variables"]
            .get("OPENCODE_SERVER_PASSWORD")
            .is_none(),
        "server credentials must not reach model shell tools"
    );
    assert!(
        observations.requests.iter().all(|request| request
            .authorization
            .as_deref()
            .is_some_and(|value| value.starts_with("Basic "))),
        "all V2 requests must authenticate"
    );
    let password =
        std::fs::read_to_string(fixture.root.path().join("cli.password")).unwrap_or_default();
    assert!(
        !password.is_empty(),
        "CLI must receive a private server password"
    );
    assert!(
        !terminal.contains(&password),
        "server secret leaked into terminal"
    );
    assert!(
        !fixture.server.repo.join(".opencode").exists(),
        "runtime artifacts must stay outside the repository"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_resume_target_is_not_replaced_with_a_new_conversation() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Complete).await;
    let _guards = fixture.guards();
    let env = HashMap::new();
    let mcp = cruise::config::McpServers::new();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        Executor::new(Some("opencode"), &[]).run(PromptRun {
            prompt: "resume input",
            model: Some(&cruise::config::ModelSpec::Single(MODEL.to_owned())),
            max_retries: 2,
            env: &env,
            mcp_servers: &mcp,
            on_notice: None,
            cancel_token: None,
            working_dir: Some(&fixture.server.repo),
            stream: None,
            tools: vec![],
            on_session_id: None,
            resume: Some(cruise::model_selection::SdkSessionId {
                sdk: cruise::model_selection::Sdk::OpenCode,
                id: "ses_missing_resume_target".to_owned(),
            }),
            computer_use: false,
            permission: PermissionMode::Full,
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("resume lookup exceeded fixture deadline"));
    let error = result
        .err()
        .unwrap_or_else(|| panic!("missing resume must fail, not start fresh"));
    let diagnostic = error.to_string().to_ascii_lowercase();
    assert!(
        diagnostic.contains("ses_missing_resume_target")
            || (diagnostic.contains("resume")
                && ["missing", "not found", "unavailable"]
                    .iter()
                    .any(|fragment| diagnostic.contains(fragment))),
        "diagnostic must identify the missing conversation: {diagnostic}"
    );
    assert_eq!(fixture.prompt_count(), 0);
    assert!(
        !locked(&fixture.server)
            .requests
            .iter()
            .any(|request| request.method == Method::POST && request.path == "/api/session"),
        "resume must not create a replacement conversation"
    );
    if fixture.root.path().join("cli.pid").exists() {
        fixture.assert_stopped();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_interrupts_active_execution_and_reaps_cli_before_return() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Pending).await;
    let _guards = fixture.guards();
    let token = CancellationToken::new();
    let run = fixture.execute_with_checkpoint(Some(MODEL), 2, None, Some(&token));
    tokio::pin!(run);
    tokio::select! {
        result = &mut run => panic!("turn must reach the active V2 execution before cancellation: {result:?}"),
        () = async {
            while fixture.prompt_count() == 0 { tokio::time::sleep(Duration::from_millis(1)).await; }
        } => token.cancel(),
    }
    let result = run.await;
    assert!(
        matches!(result, Err(cruise::error::CruiseError::Interrupted)),
        "cancellation must remain an interruption: {result:?}"
    );
    assert_eq!(
        fixture.prompt_count(),
        1,
        "cancelled input must not be retried"
    );
    fixture.assert_interrupted();
    fixture.assert_stopped();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_id_is_checkpointed_before_prompt_can_block_on_tools() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Complete).await;
    let _guards = fixture.guards();
    let callback = |id: &cruise::model_selection::SdkSessionId| -> cruise::error::Result<()> {
        assert_eq!(id.sdk, cruise::model_selection::Sdk::OpenCode);
        std::fs::write(fixture.root.path().join("checkpoint"), &id.id)?;
        Ok(())
    };
    let outcome = fixture
        .execute_with_checkpoint(Some(MODEL), 0, Some(&callback), None)
        .await
        .unwrap_or_else(|error| panic!("checkpointed turn must complete: {error}"));
    let observations = locked(&fixture.server);
    assert_eq!(
        outcome
            .session_id
            .as_ref()
            .map(|session| session.id.as_str()),
        Some(observations.session_id.as_str())
    );
    assert_eq!(
        observations.checkpoint_at_prompt.as_deref(),
        Some(observations.session_id.as_str()),
        "the resumable ID must already be persisted when execution can call ask_user"
    );
    fixture.assert_stopped();
}

async fn resume_fixture(fixture: &Fixture, id: &str) -> cruise::error::Result<PromptOutcome> {
    resume_fixture_with_retries(fixture, id, 0).await
}

async fn resume_fixture_with_retries(
    fixture: &Fixture,
    id: &str,
    retries: usize,
) -> cruise::error::Result<PromptOutcome> {
    let env = HashMap::new();
    let mcp_servers = cruise::config::McpServers::new();
    let config: RetryConfig = serde_yaml::from_str(
        "base_delay_ms: 1\nmodel_fallback: true\nfallback_chains:\n  default: [fixture/fallback]\n",
    )
    .unwrap_or_else(|error| panic!("retry config: {error}"));
    with_active_policy(
        policy_for_config(Some(config)),
        Executor::new(Some("opencode"), &[]).run(PromptRun {
            prompt: "resume fixture",
            model: Some(&cruise::config::ModelSpec::Single(MODEL.to_owned())),
            max_retries: retries,
            env: &env,
            mcp_servers: &mcp_servers,
            on_notice: None,
            cancel_token: None,
            working_dir: Some(&fixture.server.repo),
            stream: None,
            tools: vec![],
            on_session_id: None,
            resume: Some(cruise::model_selection::SdkSessionId {
                sdk: cruise::model_selection::Sdk::OpenCode,
                id: id.to_owned(),
            }),
            computer_use: false,
            permission: PermissionMode::Full,
        }),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retained_history_is_fenced_before_current_turn_recovery() {
    let _lock = lock_process();
    for (start, terminal) in [(true, false), (true, true), (false, false)] {
        let fixture = Fixture::new(Scenario::RetainedRecovery { start, terminal }).await;
        let _guards = fixture.guards();
        let checkpoint = |_: &cruise::model_selection::SdkSessionId| Ok(());
        let first = fixture
            .execute_with_checkpoint(Some(MODEL), 0, Some(&checkpoint), None)
            .await
            .unwrap_or_else(|error| panic!("retained first turn: {error}"));
        let id = first
            .session_id
            .map_or_else(|| panic!("retained session ID"), |session| session.id);
        // The real log is cumulative, including failed and interrupted executions.
        locked(&fixture.server).durable.extend([
            event("session.execution.started", &id, json!({}), 6),
            event(
                "session.execution.failed",
                &id,
                json!({"error":{
                    "type":"provider.api", "message":"historical failure", "status":503
                }}),
                7,
            ),
            event("session.execution.started", &id, json!({}), 8),
            event("session.execution.interrupted", &id, json!({}), 9),
        ]);
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            resume_fixture_with_retries(&fixture, &id, 2),
        )
        .await
        .unwrap_or_else(|error| panic!("retained recovery: {error}"));
        if terminal {
            assert_eq!(
                result
                    .unwrap_or_else(|error| panic!("current terminal: {error}"))
                    .result
                    .output,
                ANSWER
            );
        } else {
            let error = result
                .err()
                .unwrap_or_else(|| panic!("missing current terminal accepted"));
            assert!(error.to_string().contains("uncertain"), "{error}");
            let manifest = std::fs::read_dir(fixture.root.path().join("data/cruise/opencode"))
                .unwrap_or_else(|error| panic!("storage directory: {error}"))
                .next()
                .unwrap_or_else(|| panic!("retained storage missing"))
                .unwrap_or_else(|error| panic!("storage entry: {error}"))
                .path()
                .join("manifest.json");
            let value: Value = serde_json::from_slice(
                &std::fs::read(manifest).unwrap_or_else(|error| panic!("manifest: {error}")),
            )
            .unwrap_or_else(|error| panic!("manifest JSON: {error}"));
            assert_eq!(
                value["clean"], false,
                "failed settlement must remain unclean"
            );
        }
        assert_eq!(
            fixture.prompt_count(),
            2,
            "one prompt per turn, no historical fallback"
        );
        assert_eq!(fixture.launch_count(), 2);
        fixture.assert_stopped();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_boundary_admits_no_prompt() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::BoundaryUnavailable).await;
    let _guards = fixture.guards();
    let error = fixture
        .execute(Some(MODEL), 2)
        .await
        .err()
        .unwrap_or_else(|| panic!("unavailable boundary accepted"));
    assert!(error.to_string().contains("boundary"), "{error}");
    assert_eq!(fixture.prompt_count(), 0);
    assert_eq!(fixture.launch_count(), 1);
    fixture.assert_stopped();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn executor_cancellation_awaits_admitted_handler_after_peer_disconnect() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Release(Arc<AtomicBool>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Pending).await;
    let _guards = fixture.guards();
    let release = Arc::new(AtomicBool::new(false));
    let _release_on_failure = Release(release.clone());
    let entered = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let gate = release.clone();
    let start = entered.clone();
    let mutation = finished.clone();
    let tool = cruise::backend::tool::CruiseTool::new(
        "gate",
        "held handler",
        json!({}),
        Arc::new(move |_| {
            start.store(true, Ordering::Release);
            while !gate.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(5));
            }
            mutation.store(true, Ordering::Release);
            Ok("final mutation complete".into())
        }),
    );
    let cancel = CancellationToken::new();
    let env = HashMap::new();
    let mcp_servers = cruise::config::McpServers::new();
    let executor = Executor::new(Some("opencode"), &[]);
    let selection = cruise::config::ModelSpec::Single(MODEL.to_owned());
    let run = executor.run(PromptRun {
        prompt: "held handler",
        model: Some(&selection),
        max_retries: 0,
        env: &env,
        mcp_servers: &mcp_servers,
        on_notice: None,
        cancel_token: Some(&cancel),
        working_dir: Some(&fixture.server.repo),
        stream: None,
        tools: vec![tool],
        on_session_id: None,
        resume: None,
        computer_use: false,
        permission: PermissionMode::Full,
    });
    tokio::pin!(run);
    let admit = async {
        while fixture.prompt_count() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let (path, connection) = fixture_bridge_connection(&fixture);
        let request = tokio::spawn(async move {
            reqwest::Client::builder().no_proxy().build().unwrap_or_else(|error| panic!("{error}"))
                .post(format!("{}/call", connection["url"].as_str().unwrap_or_default()))
                .header("authorization", connection["token"].as_str().unwrap_or_default())
                .json(&json!({"sessionID":connection["sessionID"],"callID":"held","tool":"gate","input":{}}))
                .send().await
        });
        while !entered.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        request.abort();
        let _ = request.await;
        path
    };
    let secret = tokio::select! {
        result = &mut run => panic!("Executor completed before handler admission: {result:?}"),
        result = tokio::time::timeout(Duration::from_secs(5), admit) => result.unwrap_or_else(|error| panic!("admission: {error}")),
    };
    cancel.cancel();
    assert!(
        tokio::time::timeout(Duration::from_millis(1500), &mut run)
            .await
            .is_err(),
        "Executor returned while a synchronous handler could still mutate state"
    );
    assert!(!finished.load(Ordering::Acquire));
    assert!(secret.exists(), "secret removed before handler drain");
    release.store(true, Ordering::Release);
    let result = tokio::time::timeout(Duration::from_secs(5), &mut run)
        .await
        .unwrap_or_else(|error| panic!("Executor drain: {error}"));
    assert!(result.is_err());
    assert!(finished.load(Ordering::Acquire));
    assert!(!secret.exists());
    fixture.assert_stopped();
}

fn fixture_bridge_connection(fixture: &Fixture) -> (PathBuf, Value) {
    let overlay: Value = serde_json::from_slice(
        &std::fs::read(fixture.root.path().join("cli.overlay"))
            .unwrap_or_else(|error| panic!("overlay: {error}")),
    )
    .unwrap_or_else(|error| panic!("overlay JSON: {error}"));
    let plugin = overlay["plugins"]
        .as_array()
        .and_then(|values| values.last())
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("plugin path"));
    let path = Path::new(plugin).join("bridge.json");
    let connection: Value = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|error| panic!("bridge: {error}")),
    )
    .unwrap_or_else(|error| panic!("bridge JSON: {error}"));
    (path, connection)
}

async fn configured_fixture_run(
    fixture: &Fixture,
    config: &cruise::config::WorkflowConfig,
    planning: bool,
    budget: usize,
) -> cruise::error::Result<PromptOutcome> {
    let executor = Executor::new(config.sdk.as_deref(), &config.command);
    let model = if planning {
        config
            .plan_model
            .as_ref()
            .or(config.model.as_ref())
            .cloned()
    } else {
        executor.step_model(None, config.model.as_ref())
    };
    with_active_policy(
        policy_for_config(config.retry.clone()),
        executor.run(PromptRun {
            prompt: "configured fixture prompt",
            model: model.as_ref(),
            max_retries: budget,
            env: &config.env,
            mcp_servers: &config.mcp_servers,
            on_notice: None,
            cancel_token: None,
            working_dir: Some(&fixture.server.repo),
            stream: None,
            tools: vec![],
            on_session_id: None,
            resume: None,
            computer_use: false,
            permission: PermissionMode::Full,
        }),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_and_plan_arrays_use_effective_opencode_keys_and_explicit_precedence() {
    let _lock = lock_process();
    for planning in [false, true] {
        for variant in ["", "#precise"] {
            for precedence in ["array", "raw", "base", "provider", "default", "override"] {
                let fixture = Fixture::new(Scenario::ProviderFailure {
                    status: 503,
                    side_effect: false,
                })
                .await;
                let _guards = fixture.guards();
                let _sdk = EnvGuard::set("CRUISE_SDK", "opencode");
                let field = if planning { "plan_model" } else { "model" };
                let model_id = format!("team/{field}-{precedence}-{}:high", variant.is_empty());
                let base = format!("fixture/{model_id}");
                let primary = format!("{base}{variant}");
                let sdk = if precedence == "override" {
                    "claude"
                } else {
                    "opencode"
                };
                let mut chains = Vec::new();
                if precedence == "raw" {
                    chains.push(format!("    '{primary}': [explicit/raw]\n"));
                }
                if matches!(precedence, "raw" | "base")
                    && (precedence != "raw" || !variant.is_empty())
                {
                    chains.push(format!("    '{base}': [explicit/base]\n"));
                }
                if matches!(precedence, "raw" | "base" | "provider") {
                    chains.push("    'fixture/*': [explicit/provider]\n".to_owned());
                }
                if matches!(precedence, "raw" | "base" | "provider" | "default") {
                    chains.push("    default: [explicit/default]\n".to_owned());
                }
                let retry = if chains.is_empty() {
                    String::new()
                } else {
                    format!("retry:\n  fallback_chains:\n{}", chains.concat())
                };
                let mut config = cruise::config::WorkflowConfig::from_yaml(&format!(
                    "sdk: {sdk}\n{field}: ['{primary}', array/fallback]\n{retry}steps:\n  implement:\n    prompt: implement\n",
                )).unwrap_or_else(|error| panic!("configuration: {error}"));
                config
                    .apply_env_overrides()
                    .unwrap_or_else(|error| panic!("effective config: {error}"));
                let outcome = configured_fixture_run(&fixture, &config, planning, 1)
                    .await
                    .unwrap_or_else(|error| panic!("{field} {primary} {precedence}: {error}"));
                assert_eq!(outcome.result.output, ANSWER);
                let expected = match precedence {
                    "raw" => ("explicit", "raw"),
                    "base" => ("explicit", "base"),
                    "provider" => ("explicit", "provider"),
                    "default" => ("explicit", "default"),
                    _ => ("array", "fallback"),
                };
                let observations = locked(&fixture.server);
                let creates: Vec<_> = observations
                    .requests
                    .iter()
                    .filter(|request| {
                        request.method == Method::POST && request.path == "/api/session"
                    })
                    .collect();
                assert_eq!(creates.len(), 2);
                assert_eq!(creates[0].body["model"]["id"], model_id);
                assert_eq!(creates[1].body["model"]["providerID"], expected.0);
                assert_eq!(creates[1].body["model"]["id"], expected.1);
                fixture.assert_stopped();
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_opencode_primary_never_launches_or_falls_back_at_any_budget() {
    let _lock = lock_process();
    for budget in [0, 2] {
        let fixture = Fixture::new(Scenario::Complete).await;
        let _guards = fixture.guards();
        let _sdk = EnvGuard::set("CRUISE_SDK", "opencode");
        let mut config = cruise::config::WorkflowConfig::from_yaml(
            "sdk: opencode\nmodel: [invalid-primary, fixture/valid-fallback]\nsteps:\n  implement:\n    prompt: implement\n",
        ).unwrap_or_else(|error| panic!("invalid primary fixture: {error}"));
        config
            .apply_env_overrides()
            .unwrap_or_else(|error| panic!("effective config: {error}"));
        let error = configured_fixture_run(&fixture, &config, false, budget)
            .await
            .err()
            .unwrap_or_else(|| panic!("invalid primary must fail even with valid fallback"));
        let diagnostic = error.to_string().to_ascii_lowercase();
        assert!(
            diagnostic.contains("model") && diagnostic.contains("provider"),
            "{error}"
        );
        assert!(!fixture.root.path().join("cli.pid").exists());
        assert_eq!(fixture.prompt_count(), 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clean_resume_survives_prelaunch_overlay_and_missing_executable_failures() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Complete).await;
    let _guards = fixture.guards();
    let checkpoint = |_: &cruise::model_selection::SdkSessionId| Ok(());
    let outcome = fixture
        .execute_with_checkpoint(Some(MODEL), 0, Some(&checkpoint), None)
        .await
        .unwrap_or_else(|error| panic!("initial retained turn: {error}"));
    let id = outcome
        .session_id
        .map_or_else(|| panic!("retained ID missing"), |session| session.id);
    let baseline = fixture.prompt_count();
    let overlay: Value = serde_json::from_slice(
        &std::fs::read(fixture.root.path().join("cli.overlay"))
            .unwrap_or_else(|error| panic!("overlay: {error}")),
    )
    .unwrap_or_else(|error| panic!("overlay JSON: {error}"));
    let plugin = overlay["plugins"]
        .as_array()
        .and_then(|values| values.last())
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("plugin path"));
    let secret = Path::new(plugin).join("bridge.json");
    {
        let _overlay = EnvGuard::set("OPENCODE_CONFIG_CONTENT", "{broken");
        assert!(resume_fixture(&fixture, &id).await.is_err());
    }
    assert!(
        !secret.exists(),
        "malformed overlay must not leave a bridge secret"
    );
    assert_eq!(fixture.prompt_count(), baseline);
    assert_eq!(fixture.launch_count(), 1);
    let cli = fixture.root.path().join("bin/opencode");
    let saved = fixture.root.path().join("bin/saved-opencode");
    std::fs::rename(&cli, &saved).unwrap_or_else(|error| panic!("hide CLI: {error}"));
    {
        let _path = EnvGuard::set("PATH", fixture.root.path().join("bin"));
        assert!(resume_fixture(&fixture, &id).await.is_err());
    }
    assert!(
        !secret.exists(),
        "spawn failure must remove the bridge secret"
    );
    std::fs::rename(&saved, &cli).unwrap_or_else(|error| panic!("restore CLI: {error}"));
    assert_eq!(fixture.prompt_count(), baseline);
    assert_eq!(fixture.launch_count(), 1);
    let resumed = resume_fixture(&fixture, &id)
        .await
        .unwrap_or_else(|error| panic!("clean ID must remain resumable: {error}"));
    assert_eq!(
        resumed
            .session_id
            .as_ref()
            .map(|session| session.id.as_str()),
        Some(id.as_str())
    );
    assert_eq!(fixture.prompt_count(), baseline + 1);
    fixture.assert_stopped();

    let script =
        std::fs::read_to_string(&cli).unwrap_or_else(|error| panic!("CLI script: {error}"));
    write_executable_script(&cli, "#!/bin/sh\nprintf 'not-json\\n'\n");
    assert!(resume_fixture(&fixture, &id).await.is_err());
    write_executable_script(&cli, &script);
    let error = resume_fixture(&fixture, &id)
        .await
        .err()
        .unwrap_or_else(|| panic!("post-launch uncertainty must block resume"));
    assert!(error.to_string().contains("uncertain"), "{error}");
    assert_eq!(fixture.prompt_count(), baseline + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checkpoint_callback_failure_stops_cli_without_sending_prompt() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Complete).await;
    let _guards = fixture.guards();
    let callback = |_: &cruise::model_selection::SdkSessionId| -> cruise::error::Result<()> {
        Err(cruise::error::CruiseError::Other(
            "checkpoint-refused".to_owned(),
        ))
    };
    let result = fixture
        .execute_with_checkpoint(Some(MODEL), 2, Some(&callback), None)
        .await;
    let error = result
        .err()
        .unwrap_or_else(|| panic!("callback failure must propagate"));
    assert!(
        error.to_string().contains("checkpoint-refused"),
        "original callback error was lost: {error}"
    );
    assert_eq!(
        fixture.prompt_count(),
        0,
        "execution must not start after checkpoint failure"
    );
    assert_eq!(
        locked(&fixture.server)
            .requests
            .iter()
            .filter(|request| request.method == Method::POST && request.path == "/api/session")
            .count(),
        1,
        "a local checkpoint failure must not trigger model fallback"
    );
    fixture.assert_stopped();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn omitted_model_uses_the_v2_default_without_a_synthetic_model_reference() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Complete).await;
    let _guards = fixture.guards();
    let outcome = fixture
        .execute(None, 0)
        .await
        .unwrap_or_else(|error| panic!("OpenCode default model must be usable: {error}"));
    assert_eq!(outcome.result.output, ANSWER);
    assert_eq!(fixture.prompt_count(), 1);
    let observations = locked(&fixture.server);
    let create = observations
        .requests
        .iter()
        .find(|request| request.method == Method::POST && request.path == "/api/session")
        .unwrap_or_else(|| panic!("V2 session missing"));
    assert!(
        create.body.get("model").is_none_or(Value::is_null),
        "an omitted model must stay omitted"
    );
    assert!(
        !observations
            .requests
            .iter()
            .any(|request| request.path.ends_with("/model")),
        "do not invent a default model ID"
    );
    fixture.assert_stopped();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_provider_in_model_reference_is_rejected_before_cli_launch() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Complete).await;
    let _guards = fixture.guards();
    let error = fixture
        .execute(Some("model-without-provider"), 0)
        .await
        .err()
        .unwrap_or_else(|| panic!("OpenCode models require provider/model"));
    let diagnostic = error.to_string().to_ascii_lowercase();
    assert!(
        diagnostic.contains("model") && diagnostic.contains("provider"),
        "invalid model diagnostic must identify its format: {diagnostic}"
    );
    assert!(
        !fixture.root.path().join("cli.pid").exists(),
        "invalid input must not start OpenCode"
    );
    assert_eq!(fixture.prompt_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_api_http_failures_keep_status_and_never_become_model_fallback() {
    let _lock = lock_process();
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::NOT_FOUND,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        let fixture = Fixture::new(Scenario::HandshakeFailure(status)).await;
        let _guards = fixture.guards();
        let result = fixture.execute(Some(MODEL), 2).await;
        assert!(
            locked(&fixture.server)
                .requests
                .iter()
                .any(|request| request.path == "/api/info"),
            "must reach the local V2 API before classifying its failure: {result:?}"
        );
        let error = result
            .err()
            .unwrap_or_else(|| panic!("local API {status} must fail"));
        let diagnostic = error.to_string();
        assert!(
            diagnostic.contains(status.as_str()),
            "HTTP status was lost: {diagnostic}"
        );
        assert_eq!(
            fixture.prompt_count(),
            0,
            "server failure must not dispatch a provider prompt"
        );
        assert!(
            !diagnostic.to_ascii_lowercase().contains("model not found"),
            "API failure misclassified as provider model absence: {diagnostic}"
        );
        fixture.assert_stopped();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_disconnect_recovers_durable_text_without_resending_prompt() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Recover).await;
    let _guards = fixture.guards();
    let outcome = fixture
        .execute(Some(MODEL), 2)
        .await
        .unwrap_or_else(|error| panic!("durable recovery must succeed: {error}"));
    assert_eq!(outcome.result.output, ANSWER);
    assert_eq!(
        outcome
            .session_id
            .as_ref()
            .map(|session| session.id.as_str()),
        Some(locked(&fixture.server).session_id.as_str())
    );
    assert_eq!(
        fixture.prompt_count(),
        1,
        "recovery must not replay the input"
    );
    assert!(
        locked(&fixture.server)
            .requests
            .iter()
            .any(
                |request| request.path.starts_with("/api/experimental/session/")
                    && request.path.ends_with("/log")
            ),
        "disconnected live events require durable reconciliation"
    );
    fixture.assert_stopped();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_and_prompt_acceptance_without_execution_terminal_are_not_success() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Incomplete).await;
    let _guards = fixture.guards();
    let result = fixture.execute(Some(MODEL), 2).await;
    assert_eq!(
        fixture.prompt_count(),
        1,
        "an uncertain execution cannot be replayed even without an output callback"
    );
    assert!(
        result.is_err(),
        "text plus HTTP acceptance is not execution success"
    );
    assert!(
        result
            .err()
            .unwrap_or_else(|| panic!("terminal evidence is required"))
            .to_string()
            .contains("execution outcome uncertain")
    );
    fixture.assert_stopped();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unavailable_durable_log_fails_without_replaying_the_input() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::LogUnavailable).await;
    let _guards = fixture.guards();
    let error = fixture
        .execute(Some(MODEL), 2)
        .await
        .err()
        .unwrap_or_else(|| panic!("unrecoverable event loss cannot report success"));
    assert!(
        error
            .to_string()
            .contains("durable reconciliation failed: OpenCode local API HTTP 503")
    );
    assert_eq!(fixture.prompt_count(), 1);
    assert_eq!(fixture.launch_count(), 1);
    fixture.assert_stopped();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_prompt_acceptance_response_prevents_retry_and_model_fallback() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::UncertainAcceptance).await;
    let _guards = fixture.guards();
    let result = fixture.execute(Some(MODEL), 2).await;
    assert_eq!(
        fixture.prompt_count(),
        1,
        "a transport failure after acceptance must set the retry barrier"
    );
    assert!(
        result.is_err(),
        "lost acceptance is an uncertain result, not success"
    );
    fixture.assert_stopped();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_required_event_fails_instead_of_reporting_success() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::Malformed).await;
    let _guards = fixture.guards();
    let result = fixture.execute(Some(MODEL), 2).await;
    assert_eq!(
        fixture.prompt_count(),
        1,
        "malformed protocol cannot be replayed"
    );
    assert!(result.is_err(), "a malformed mandatory event must fail");
    fixture.assert_stopped();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn safe_provider_failures_use_retry_and_fallback_without_corrupting_model_ids() {
    let _lock = lock_process();
    for status in [429, 404] {
        let fixture = Fixture::new(Scenario::ProviderFailure {
            status,
            side_effect: false,
        })
        .await;
        let _guards = fixture.guards();
        let outcome = fixture
            .execute(Some(MODEL), 2)
            .await
            .unwrap_or_else(|error| panic!("safe provider {status}: {error}"));
        assert_eq!(outcome.result.output, ANSWER);
        assert_eq!(fixture.prompt_count(), 2);
        let observations = locked(&fixture.server);
        let models: Vec<_> = observations
            .requests
            .iter()
            .filter(|request| request.method == Method::POST && request.path == "/api/session")
            .map(|request| &request.body["model"])
            .collect();
        assert_eq!(models.len(), 2);
        assert_eq!(
            models[0],
            &json!({"providerID":"fixture","id":"team/model:2026","variant":"precise"})
        );
        assert_eq!(
            models[1],
            &if status == 429 {
                json!({"providerID":"fixture","id":"team/model:2026","variant":"precise"})
            } else {
                json!({"providerID":"fixture","id":"fallback"})
            }
        );
        fixture.assert_stopped();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_failure_after_a_tool_effect_never_replays_without_visible_text() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::ProviderFailure {
        status: 429,
        side_effect: true,
    })
    .await;
    let _guards = fixture.guards();
    assert!(fixture.execute(Some(MODEL), 2).await.is_err());
    assert_eq!(fixture.prompt_count(), 1);
    fixture.assert_stopped();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_required_plugin_stops_before_prompt_and_does_not_fallback() {
    let _lock = lock_process();
    let fixture = Fixture::new(Scenario::PluginDisabled).await;
    let _guards = fixture.guards();
    assert!(fixture.execute(Some(MODEL), 2).await.is_err());
    assert_eq!(fixture.prompt_count(), 0);
    assert_eq!(
        locked(&fixture.server)
            .requests
            .iter()
            .filter(|request| request.method == Method::POST && request.path == "/api/session")
            .count(),
        1
    );
    fixture.assert_stopped();
}
