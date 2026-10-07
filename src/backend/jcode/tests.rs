#![cfg(unix)]

use super::*;
use std::ffi::OsStr;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use jcode_sdk::api::{
    API_VERSION_MAJOR, ApiRequest, ClientFrame, ErrorCode, ModelRouteInfo, ServerFrame,
    SessionInfo, read_frame, write_frame,
};
use jcode_sdk::{ConnectOptions, Error, ErrorKind, JcodeClient, Transport};
use serde_json::json;
use tempfile::TempDir;

struct PairTransport(UnixStream);

impl Transport for PairTransport {
    fn split(
        self: Box<Self>,
    ) -> jcode_sdk::Result<(Box<dyn BufRead + Send>, Box<dyn Write + Send>)> {
        let writer = self
            .0
            .try_clone()
            .map_err(|error| Error::new(ErrorKind::Transport, error.to_string()))?;
        Ok((Box::new(BufReader::new(self.0)), Box::new(writer)))
    }
}

fn fake_harness(handle: impl FnMut(&ClientFrame, &mut UnixStream) + Send + 'static) -> JcodeClient {
    fake_harness_with_capabilities(
        vec!["sessions".to_string(), "session_tools".to_string()],
        handle,
    )
}

fn fake_harness_with_capabilities(
    capabilities: Vec<String>,
    mut handle: impl FnMut(&ClientFrame, &mut UnixStream) + Send + 'static,
) -> JcodeClient {
    let (client, server) = UnixStream::pair().unwrap_or_else(|error| panic!("{error}"));
    std::thread::spawn(move || {
        let mut reader =
            BufReader::new(server.try_clone().unwrap_or_else(|error| panic!("{error}")));
        let mut writer = server;
        while let Ok(frame) = read_frame::<_, ClientFrame>(&mut reader) {
            if matches!(&frame.request, ApiRequest::Hello { .. }) {
                send_reply(
                    &frame,
                    jcode_sdk::api::ApiEvent::HelloOk {
                        version: API_VERSION_MAJOR,
                        server: "jcode-harness-api-bridge/0.1.0".to_string(),
                        capabilities: capabilities.clone(),
                    },
                    &mut writer,
                );
                continue;
            }
            handle(&frame, &mut writer);
        }
    });
    JcodeClient::connect_with(
        Box::new(PairTransport(client)),
        ConnectOptions {
            request_timeout: Some(Duration::from_secs(5)),
            ensure_runtime: false,
            ..Default::default()
        },
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

fn send_reply<W: Write>(frame: &ClientFrame, event: jcode_sdk::ApiEvent, writer: &mut W) {
    write_frame(
        writer,
        &ServerFrame {
            v: API_VERSION_MAJOR,
            reply_to: Some(frame.id),
            event,
        },
    )
    .unwrap_or_else(|error| panic!("{error}"));
}

fn send_event<W: Write>(event: jcode_sdk::ApiEvent, writer: &mut W) {
    write_frame(
        writer,
        &ServerFrame {
            v: API_VERSION_MAJOR,
            reply_to: None,
            event,
        },
    )
    .unwrap_or_else(|error| panic!("{error}"));
}

fn session(id: &str) -> SessionInfo {
    SessionInfo {
        edit_stats: None,
        parent_session_id: None,
        agent_label: None,
        swarm_status: None,
        session_id: id.to_string(),
        working_dir: None,
        title: None,
        status: "idle".to_string(),
        transcript_bytes: None,
        saved: false,
        updated_at_ms: None,
        last_active_at_ms: None,
        archived: false,
        archived_at_ms: None,
        save_label: None,
    }
}

fn session_home() -> (TempDir, SessionHome) {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source_home = temp.path().join("source");
    let storage_root = source_home.join(SESSION_HOME_DIR);
    let storage_home = storage_root.join("test-session");
    fs::create_dir_all(&storage_home).unwrap_or_else(|error| panic!("{error}"));
    (
        temp,
        SessionHome {
            source_home,
            storage_root,
            storage_home: storage_home.clone(),
            expected_home: None,
            sdk_home: storage_home,
            temporary_alias: None,
            stable_alias_root: None,
            fresh: true,
            resume_session_found: false,
            _active_lock: None,
        },
    )
}

type TurnWorkerResult = (
    std::result::Result<StreamChunk, String>,
    Option<String>,
    TempDir,
);
type TurnWorker = JoinHandle<TurnWorkerResult>;

fn spawn_turn(
    client: JcodeClient,
    config: JcodeRunnerConfig,
) -> (Receiver<StreamChunk>, TurnWorker) {
    let (tx, rx) = sync_channel(0);
    let (temp, mut home) = session_home();
    let worker = std::thread::spawn(move || {
        let mut session_id = None;
        let result = run_client_turn(
            &client,
            &config,
            "do the task",
            &tx,
            &mut home,
            &mut session_id,
        );
        (result, session_id, temp)
    });
    (rx, worker)
}

fn send_text_after_accept<W: Write>(writer: &mut W, session_id: &str) {
    send_event(
        jcode_sdk::ApiEvent::TextDelta {
            session_id: session_id.to_string(),
            text: "finished".to_string(),
            message_id: Some("message-1".to_string()),
        },
        writer,
    );
    send_event(
        jcode_sdk::ApiEvent::TextDone {
            session_id: session_id.to_string(),
            message_id: Some("message-1".to_string()),
        },
        writer,
    );
    send_event(
        jcode_sdk::ApiEvent::TurnDone {
            session_id: session_id.to_string(),
        },
        writer,
    );
}

fn send_message_accepted<W: Write>(writer: &mut W, session_id: &str) {
    send_event(
        jcode_sdk::ApiEvent::MessageAccepted {
            session_id: session_id.to_string(),
        },
        writer,
    );
}
#[test]
fn jcode_capability_errors_name_the_required_runtime_floor() {
    let client =
        fake_harness_with_capabilities(vec!["session_tools".to_string()], |frame, _writer| {
            panic!("unexpected request: {:?}", frame.request)
        });
    let (_chunks, worker) = spawn_turn(client, JcodeRunnerConfig::default());
    let (result, session_id, _home) = worker
        .join()
        .unwrap_or_else(|_| panic!("turn worker panicked"));
    assert!(session_id.is_none());
    assert!(
        matches!(result, Err(message) if message.contains("sessions") && message.contains("0.88.0"))
    );

    let client = fake_harness_with_capabilities(vec!["sessions".to_string()], |frame, _writer| {
        panic!("unexpected request: {:?}", frame.request)
    });
    let (_chunks, worker) = spawn_turn(
        client,
        JcodeRunnerConfig {
            tools: vec![skip_tool(Arc::new(Mutex::new(None)))],
            ..Default::default()
        },
    );
    let (result, session_id, _home) = worker
        .join()
        .unwrap_or_else(|_| panic!("turn worker panicked"));
    assert!(session_id.is_none());
    assert!(
        matches!(result, Err(message) if message.contains("session tools") && message.contains("0.88.0"))
    );
}

fn skip_tool(reason_store: Arc<Mutex<Option<String>>>) -> CruiseTool {
    crate::sdk_tools::skip_step_tool(reason_store)
}

#[test]
fn session_tool_round_trip_records_skip_step_and_final_text() {
    let reason_store = Arc::new(Mutex::new(None));
    let configured_tools = Arc::new(Mutex::new(Vec::new()));
    let configured_tools_for_harness = Arc::clone(&configured_tools);
    let client = fake_harness(move |frame, writer| match &frame.request {
        ApiRequest::CreateSession { .. } => {
            send_reply(
                frame,
                jcode_sdk::ApiEvent::Attached {
                    session: session("s1"),
                },
                writer,
            );
        }
        ApiRequest::ConfigureTools { tools, .. } => {
            let names = tools.custom.iter().map(|tool| tool.name.clone()).collect();
            *configured_tools_for_harness
                .lock()
                .unwrap_or_else(|error| panic!("{error}")) = names;
            send_reply(frame, jcode_sdk::ApiEvent::Ok, writer);
        }
        ApiRequest::SendMessage { session_id, .. } => {
            send_message_accepted(writer, session_id);
            send_event(
                jcode_sdk::ApiEvent::ToolCall {
                    session_id: session_id.clone(),
                    call_id: "call-1".to_string(),
                    name: "skip_step".to_string(),
                    input: json!({"reason": "the workflow requires no changes"}),
                },
                writer,
            );
        }
        ApiRequest::ToolResult { .. } => {
            send_reply(frame, jcode_sdk::ApiEvent::Ok, writer);
            send_text_after_accept(writer, "s1");
        }
        request => panic!("unexpected request: {request:?}"),
    });
    let config = JcodeRunnerConfig {
        tools: vec![skip_tool(Arc::clone(&reason_store))],
        ..Default::default()
    };
    let (chunks, worker) = spawn_turn(client, config);

    assert!(
        matches!(chunks.recv().unwrap_or_else(|error| panic!("{error}")), StreamChunk::Session(id) if id == "s1")
    );
    assert!(
        matches!(chunks.recv().unwrap_or_else(|error| panic!("{error}")), StreamChunk::Delta(text) if text == "finished")
    );
    let (result, id, _home) = worker
        .join()
        .unwrap_or_else(|_| panic!("turn worker panicked"));
    assert_eq!(id.as_deref(), Some("s1"));
    assert!(matches!(result, Ok(StreamChunk::Done(text)) if text == "finished"));
    assert_eq!(
        reason_store
            .lock()
            .unwrap_or_else(|error| panic!("{error}"))
            .as_deref(),
        Some("the workflow requires no changes")
    );
    assert_eq!(
        *configured_tools
            .lock()
            .unwrap_or_else(|error| panic!("{error}")),
        ["skip_step"]
    );
}

#[test]
fn attaching_a_session_reapplies_tools_and_dispatches_them() {
    let reason_store = Arc::new(Mutex::new(None));
    let configured = Arc::new(AtomicBool::new(false));
    let configured_in_harness = Arc::clone(&configured);
    let client = fake_harness(move |frame, writer| match &frame.request {
        ApiRequest::AttachSession { session_id } => {
            assert_eq!(session_id, "persisted-session");
            send_reply(
                frame,
                jcode_sdk::ApiEvent::Attached {
                    session: session(session_id),
                },
                writer,
            );
        }
        ApiRequest::ConfigureTools { tools, .. } => {
            assert_eq!(
                tools.custom.first().map(|tool| tool.name.as_str()),
                Some("skip_step")
            );
            configured_in_harness.store(true, Ordering::SeqCst);
            send_reply(frame, jcode_sdk::ApiEvent::Ok, writer);
        }
        ApiRequest::SendMessage { session_id, .. } => {
            send_message_accepted(writer, session_id);
            send_event(
                jcode_sdk::ApiEvent::ToolCall {
                    session_id: session_id.clone(),
                    call_id: "resume-call".to_string(),
                    name: "skip_step".to_string(),
                    input: json!({"reason": "the resumed workflow also needs no changes"}),
                },
                writer,
            );
        }
        ApiRequest::ToolResult { .. } => {
            send_reply(frame, jcode_sdk::ApiEvent::Ok, writer);
            send_text_after_accept(writer, "persisted-session");
        }
        request => panic!("unexpected request: {request:?}"),
    });
    let config = JcodeRunnerConfig {
        resume_session_id: Some("persisted-session".to_string()),
        tools: vec![skip_tool(Arc::clone(&reason_store))],
        ..Default::default()
    };
    let (chunks, worker) = spawn_turn(client, config);
    assert!(
        matches!(chunks.recv().unwrap_or_else(|error| panic!("{error}")), StreamChunk::Session(id) if id == "persisted-session")
    );
    assert!(
        matches!(chunks.recv().unwrap_or_else(|error| panic!("{error}")), StreamChunk::Delta(text) if text == "finished")
    );
    let (result, id, _home) = worker
        .join()
        .unwrap_or_else(|_| panic!("turn worker panicked"));
    assert_eq!(id.as_deref(), Some("persisted-session"));
    assert!(matches!(result, Ok(StreamChunk::Done(text)) if text == "finished"));
    assert!(configured.load(Ordering::SeqCst));
    assert_eq!(
        reason_store
            .lock()
            .unwrap_or_else(|error| panic!("{error}"))
            .as_deref(),
        Some("the resumed workflow also needs no changes")
    );
}
#[test]
fn missing_saved_session_starts_a_fresh_session() {
    let client = fake_harness(|frame, writer| match &frame.request {
        ApiRequest::AttachSession { session_id } => {
            assert_eq!(session_id, "legacy-plan-session");
            send_reply(
                frame,
                jcode_sdk::ApiEvent::Error {
                    code: ErrorCode::UnknownSession,
                    message: "Unknown session 'legacy-plan-session'".to_string(),
                },
                writer,
            );
        }
        ApiRequest::CreateSession { .. } => send_reply(
            frame,
            jcode_sdk::ApiEvent::Attached {
                session: session("fresh-plan-session"),
            },
            writer,
        ),
        ApiRequest::SendMessage { session_id, .. } => {
            send_message_accepted(writer, session_id);
            send_text_after_accept(writer, session_id);
        }
        request => panic!("unexpected request: {request:?}"),
    });
    let (chunks, worker) = spawn_turn(
        client,
        JcodeRunnerConfig {
            resume_session_id: Some("legacy-plan-session".to_string()),
            ..Default::default()
        },
    );
    assert!(
        matches!(chunks.recv().unwrap_or_else(|error| panic!("{error}")), StreamChunk::Session(id) if id == "fresh-plan-session")
    );
    assert!(
        matches!(chunks.recv().unwrap_or_else(|error| panic!("{error}")), StreamChunk::Delta(text) if text == "finished")
    );
    let (result, session_id, _home) = worker
        .join()
        .unwrap_or_else(|_| panic!("turn worker panicked"));
    assert_eq!(session_id.as_deref(), Some("fresh-plan-session"));
    assert!(matches!(result, Ok(StreamChunk::Done(text)) if text == "finished"));
}
#[test]
fn context_limit_error_event_is_not_a_rate_limit() {
    let client = fake_harness(|frame, writer| match &frame.request {
        ApiRequest::CreateSession { .. } => send_reply(
            frame,
            jcode_sdk::ApiEvent::Attached {
                session: session("context-limit"),
            },
            writer,
        ),
        ApiRequest::SendMessage { session_id, .. } => {
            send_message_accepted(writer, session_id);
            send_event(
                jcode_sdk::ApiEvent::TurnStopped {
                    session_id: session_id.clone(),
                    reason: TurnStopReason::LimitReached,
                    message: "Context limit exceeded after 3 compaction retries".to_string(),
                    provider_stop_reason: None,
                },
                writer,
            );
            send_event(
                jcode_sdk::ApiEvent::Error {
                    code: ErrorCode::Internal,
                    message: "provider rejected the turn".to_string(),
                },
                writer,
            );
        }
        request => panic!("unexpected request: {request:?}"),
    });
    let (chunks, worker) = spawn_turn(client, JcodeRunnerConfig::default());
    assert!(
        matches!(chunks.recv().unwrap_or_else(|error| panic!("{error}")), StreamChunk::Session(id) if id == "context-limit")
    );
    let (result, _, _home) = worker
        .join()
        .unwrap_or_else(|_| panic!("turn worker panicked"));
    assert!(
        matches!(result, Ok(StreamChunk::Error(message)) if message.contains("Context limit exceeded"))
    );
}

#[test]
fn turn_stop_reason_preserves_limit_classification() {
    let client = fake_harness(|frame, writer| match &frame.request {
        ApiRequest::CreateSession { .. } => {
            send_reply(
                frame,
                jcode_sdk::ApiEvent::Attached {
                    session: session("limit"),
                },
                writer,
            );
        }
        ApiRequest::SendMessage { session_id, .. } => {
            send_message_accepted(writer, session_id);
            send_event(
                jcode_sdk::ApiEvent::TurnStopped {
                    session_id: session_id.clone(),
                    reason: TurnStopReason::LimitReached,
                    message: "HTTP 429 rate limit exceeded".to_string(),
                    provider_stop_reason: None,
                },
                writer,
            );
            send_event(
                jcode_sdk::ApiEvent::Error {
                    code: ErrorCode::Internal,
                    message: "HTTP 429 Too Many Requests".to_string(),
                },
                writer,
            );
        }
        request => panic!("unexpected request: {request:?}"),
    });
    let (chunks, worker) = spawn_turn(client, JcodeRunnerConfig::default());
    assert!(
        matches!(chunks.recv().unwrap_or_else(|error| panic!("{error}")), StreamChunk::Session(id) if id == "limit")
    );
    let (result, _, _home) = worker
        .join()
        .unwrap_or_else(|_| panic!("turn worker panicked"));
    assert!(
        matches!(result, Ok(StreamChunk::Limit(LimitError { provider, .. })) if provider == "jcode")
    );
}
#[test]
fn context_limit_stop_is_not_a_retryable_limit() {
    let result = stop_chunk(
        TurnStopReason::LimitReached,
        "Context limit exceeded after 3 compaction retries".to_string(),
        None,
    );
    assert!(
        matches!(result, StreamChunk::Error(message) if message.contains("Context limit exceeded"))
    );
}

#[test]
fn provider_error_text_is_classified_for_fallback() {
    let client = fake_harness(|frame, writer| match &frame.request {
        ApiRequest::CreateSession { .. } => {
            send_reply(
                frame,
                jcode_sdk::ApiEvent::Attached {
                    session: session("missing"),
                },
                writer,
            );
        }
        ApiRequest::SendMessage { session_id, .. } => {
            send_message_accepted(writer, session_id);
            send_event(
                jcode_sdk::ApiEvent::Error {
                    code: ErrorCode::Internal,
                    message: "HTTP 404 model not found".to_string(),
                },
                writer,
            );
        }
        request => panic!("unexpected request: {request:?}"),
    });
    let (chunks, worker) = spawn_turn(client, JcodeRunnerConfig::default());
    assert!(
        matches!(chunks.recv().unwrap_or_else(|error| panic!("{error}")), StreamChunk::Session(id) if id == "missing")
    );
    let (result, _, _home) = worker
        .join()
        .unwrap_or_else(|_| panic!("turn worker panicked"));
    let Ok(StreamChunk::Error(message)) = result else {
        panic!("expected the provider error");
    };
    assert_eq!(
        crate::retry::classify_retryable(&message),
        Some(crate::retry::RetryClass::ModelMissing)
    );
}

#[test]
fn cancellation_requests_session_cancel_before_returning() {
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let cancel_in_harness = Arc::clone(&saw_cancel);
    let client = fake_harness(move |frame, writer| match &frame.request {
        ApiRequest::CreateSession { .. } => {
            send_reply(
                frame,
                jcode_sdk::ApiEvent::Attached {
                    session: session("cancel-me"),
                },
                writer,
            );
        }
        ApiRequest::SendMessage { session_id, .. } => send_message_accepted(writer, session_id),
        ApiRequest::Cancel { session_id } => {
            cancel_in_harness.store(true, Ordering::SeqCst);
            send_reply(frame, jcode_sdk::ApiEvent::Ok, writer);
            send_event(
                jcode_sdk::ApiEvent::TurnStopped {
                    session_id: session_id.clone(),
                    reason: TurnStopReason::Interrupted,
                    message: "cancelled by client".to_string(),
                    provider_stop_reason: None,
                },
                writer,
            );
            send_event(
                jcode_sdk::ApiEvent::TurnDone {
                    session_id: session_id.clone(),
                },
                writer,
            );
        }
        request => panic!("unexpected request: {request:?}"),
    });
    let token = CancellationToken::new();
    let (chunks, worker) = spawn_turn(
        client,
        JcodeRunnerConfig {
            cancel: Some(token.clone()),
            ..Default::default()
        },
    );
    assert!(
        matches!(chunks.recv().unwrap_or_else(|error| panic!("{error}")), StreamChunk::Session(id) if id == "cancel-me")
    );
    token.cancel();
    let (result, _, _home) = worker
        .join()
        .unwrap_or_else(|_| panic!("turn worker panicked"));
    assert!(saw_cancel.load(Ordering::SeqCst));
    assert!(matches!(result, Ok(StreamChunk::Error(message)) if message.contains("Interrupted")));
}
#[test]
fn abandoned_delta_stream_cancels_the_active_session() {
    let saw_cancel = Arc::new(AtomicBool::new(false));
    let cancel_in_harness = Arc::clone(&saw_cancel);
    let client = fake_harness(move |frame, writer| match &frame.request {
        ApiRequest::CreateSession { .. } => send_reply(
            frame,
            jcode_sdk::ApiEvent::Attached {
                session: session("abandoned"),
            },
            writer,
        ),
        ApiRequest::SendMessage { session_id, .. } => {
            send_message_accepted(writer, session_id);
            send_event(
                jcode_sdk::ApiEvent::TextDelta {
                    session_id: session_id.clone(),
                    text: "unconsumed".to_string(),
                    message_id: Some("message-1".to_string()),
                },
                writer,
            );
        }
        ApiRequest::Cancel { .. } => {
            cancel_in_harness.store(true, Ordering::SeqCst);
            send_reply(frame, jcode_sdk::ApiEvent::Ok, writer);
        }
        request => panic!("unexpected request: {request:?}"),
    });
    let (chunks, worker) = spawn_turn(client, JcodeRunnerConfig::default());
    assert!(
        matches!(chunks.recv().unwrap_or_else(|error| panic!("{error}")), StreamChunk::Session(id) if id == "abandoned")
    );
    drop(chunks);
    let (result, _, _home) = worker
        .join()
        .unwrap_or_else(|_| panic!("turn worker panicked"));
    assert!(saw_cancel.load(Ordering::SeqCst));
    assert!(matches!(result, Err(message) if message.contains("output stream was closed")));
}

#[test]
fn model_routes_keep_provider_prefixes_and_resolve_profiles() {
    let route = ModelRouteInfo {
        model: "cheap-model".to_string(),
        provider: "OpenAI-compatible".to_string(),
        api_method: "openai-compatible:minimax".to_string(),
        available: true,
        detail: String::new(),
        usage: None,
    };
    assert_eq!(
        model_switch_request(Some("anthropic-api"), "claude-opus", &[]),
        "claude-api:claude-opus"
    );
    assert_eq!(
        model_switch_request(Some("openai"), "gpt-5-mini", &[]),
        "openai:gpt-5-mini"
    );
    assert_eq!(
        model_switch_request(Some("minimax"), "cheap-model", &[route]),
        "minimax:cheap-model"
    );
    let parsed = parse_model_ref(Some("openrouter/meta-llama/model:free"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(parsed.0.as_deref(), Some("openrouter"));
    assert_eq!(parsed.1.as_deref(), Some("meta-llama/model:free"));
    assert_eq!(parsed.2, None);
}

#[test]
fn sdk_child_environment_keeps_workflow_env_and_forces_required_overrides() {
    let workflow = HashMap::from([
        (
            "CRUISE_GUARD_COMMON_DIR".to_string(),
            "/tmp/guard".to_string(),
        ),
        ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
        (JCODE_HOME_ENV.to_string(), "/tmp/action-jcode".to_string()),
        (NO_TELEMETRY_ENV.to_string(), "0".to_string()),
        (OPENAI_SERVICE_TIER_ENV.to_string(), String::new()),
        (JCODE_CHECK_UPDATES_ENV.to_string(), "1".to_string()),
    ]);
    let env = launch_environment(&workflow, None, None, true, None, None);
    assert_eq!(
        env.get(OsStr::new("CRUISE_GUARD_COMMON_DIR"))
            .and_then(|value| value.to_str()),
        Some("/tmp/guard")
    );
    assert_eq!(
        env.get(OsStr::new("GIT_CONFIG_COUNT"))
            .and_then(|value| value.to_str()),
        Some("1")
    );
    assert!(!env.contains_key(OsStr::new(JCODE_HOME_ENV)));
    assert_eq!(
        env.get(OsStr::new(NO_TELEMETRY_ENV))
            .and_then(|value| value.to_str()),
        Some("1")
    );
    assert_eq!(
        env.get(OsStr::new(OPENAI_SERVICE_TIER_ENV))
            .and_then(|value| value.to_str()),
        Some("")
    );
}

#[test]
fn launch_environment_preserves_the_workflow_disabled_tools_policy() {
    let expected = "bash,macos_computer_use";
    let env = launch_environment(
        &HashMap::from([("JCODE_DISABLED_TOOLS".to_string(), expected.to_string())]),
        None,
        None,
        true,
        None,
        None,
    );

    assert_eq!(
        env.get(OsStr::new("JCODE_DISABLED_TOOLS"))
            .and_then(|value| value.to_str()),
        Some(expected)
    );
}

#[test]
fn launch_environment_disables_computer_use_by_default() {
    let env = launch_environment(&HashMap::new(), None, None, false, None, None);

    assert_eq!(
        env.get(OsStr::new(JCODE_DISABLED_TOOLS_ENV))
            .and_then(|value| value.to_str()),
        Some(COMPUTER_USE_TOOL)
    );
}

#[test]
fn launch_environment_merges_jcode_config_disabled_tools() {
    let env = launch_environment(
        &HashMap::new(),
        None,
        None,
        false,
        None,
        Some("[tools]\ndisabled = [\"bash\"]"),
    );

    assert_eq!(
        env.get(OsStr::new(JCODE_DISABLED_TOOLS_ENV))
            .and_then(|value| value.to_str()),
        Some("bash,macos_computer_use")
    );
}

#[test]
fn computer_use_disabled_tools_uses_workflow_then_process_then_config_precedence() {
    let config = "[tools]\ndisabled = [\"config-tool\"]";

    assert_eq!(
        computer_use_disabled_tools(Some("workflow-tool"), Some("process-tool"), Some(config)),
        "workflow-tool,macos_computer_use"
    );
    assert_eq!(
        computer_use_disabled_tools(None, Some("process-tool"), Some(config)),
        "process-tool,macos_computer_use"
    );
    assert_eq!(
        computer_use_disabled_tools(None, Some(""), Some(config)),
        "macos_computer_use"
    );
}

#[test]
fn computer_use_disabled_tools_deduplicates_and_ignores_invalid_toml() {
    assert_eq!(
        computer_use_disabled_tools(
            Some(" bash, macos_computer_use\nbash "),
            None,
            Some("not valid toml = ["),
        ),
        "bash,macos_computer_use"
    );
    assert_eq!(
        computer_use_disabled_tools(None, None, Some("not valid toml = [")),
        "macos_computer_use"
    );
}

#[test]
fn launch_environment_leaves_disabled_tool_policy_unchanged_when_enabled() {
    let workflow = HashMap::from([(
        JCODE_DISABLED_TOOLS_ENV.to_string(),
        "bash, macos_computer_use".to_string(),
    )]);
    let env = launch_environment(&workflow, None, None, true, Some("process-tool"), None);

    assert_eq!(
        env.get(OsStr::new(JCODE_DISABLED_TOOLS_ENV))
            .and_then(|value| value.to_str()),
        Some("bash, macos_computer_use")
    );
    let env = launch_environment(&HashMap::new(), None, None, true, None, None);
    assert!(!env.contains_key(OsStr::new(JCODE_DISABLED_TOOLS_ENV)));
}

#[test]
fn private_runtime_disables_auto_update_with_environment_override() {
    let env = launch_environment(
        &HashMap::from([(JCODE_CHECK_UPDATES_ENV.to_string(), "1".to_string())]),
        None,
        None,
        true,
        None,
        None,
    );
    assert_eq!(
        env.get(OsStr::new(JCODE_CHECK_UPDATES_ENV))
            .and_then(|value| value.to_str()),
        Some("0")
    );
}

fn assert_private_copied_file(path: &Path, contents: &[u8]) {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::symlink_metadata(path).unwrap_or_else(|error| panic!("{error}"));
    assert!(metadata.file_type().is_file());
    assert!(!metadata.file_type().is_symlink());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        fs::read(path).unwrap_or_else(|error| panic!("{error}")),
        contents
    );
}

fn assert_private_model_catalog_copies(storage_home: &Path, fixtures: &[(&str, &[u8])]) {
    let catalog_dir = storage_home.join("config/jcode");
    for (name, contents) in fixtures {
        assert_private_copied_file(&catalog_dir.join(name), contents);
    }
    assert!(
        !catalog_dir.join("gemini_models_cache.json").exists(),
        "catalog caches absent from the source must stay absent"
    );
}

fn assert_external_auth_trust(
    storage_home: &Path,
    trusted_hosts_entry: &str,
    untrusted_apps_entry: &str,
    expected_hosts_entries: usize,
) {
    let config_path = storage_home.join(JCODE_CONFIG_FILE);
    let config: toml::Table =
        toml::from_str(&fs::read_to_string(&config_path).unwrap_or_else(|error| panic!("{error}")))
            .unwrap_or_else(|error| panic!("{error}"));
    let trusted_paths = config
        .get("auth")
        .and_then(toml::Value::as_table)
        .and_then(|auth| auth.get("trusted_external_source_paths"))
        .and_then(toml::Value::as_array)
        .unwrap_or_else(|| panic!("private config trust list missing"));
    assert_eq!(
        trusted_paths
            .iter()
            .filter(|entry| {
                entry
                    .as_str()
                    .is_some_and(|entry| entry.eq_ignore_ascii_case(trusted_hosts_entry))
            })
            .count(),
        expected_hosts_entries
    );
    assert!(!trusted_paths.iter().any(|entry| {
        entry
            .as_str()
            .is_some_and(|entry| entry.eq_ignore_ascii_case(untrusted_apps_entry))
    }));
    assert!(
        trusted_paths
            .iter()
            .any(|entry| entry.as_str() == Some("manual-source"))
    );
    assert_eq!(
        config
            .get("tools")
            .and_then(toml::Value::as_table)
            .and_then(|tools| tools.get("disabled"))
            .and_then(toml::Value::as_array)
            .and_then(|tools| tools.first())
            .and_then(toml::Value::as_str),
        Some("bash")
    );
}

fn path_trust_entry(source_id: &str, path: &Path) -> String {
    format!(
        "{source_id}|{}",
        fs::canonicalize(path)
            .unwrap_or_else(|error| panic!("{error}"))
            .to_string_lossy()
            .to_ascii_lowercase()
    )
}

fn write_trusted_external_config(source_home: &Path, hosts_path: &Path) {
    let source_entry = path_trust_entry("copilot_hosts_json", hosts_path);
    let config = format!(
        "[tools]\ndisabled = [\"bash\"]\n[auth]\ntrusted_external_source_paths = [\
         {source_entry:?}, \"manual-source\"]\n"
    );
    fs::write(source_home.join(JCODE_CONFIG_FILE), config)
        .unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn private_home_copies_catalogs_and_external_auth_idempotently() {
    let _process_lock = crate::test_support::lock_process();
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home_dir = temp.path().join("user-home");
    let source_home = temp.path().join("source-home");
    let app_config_dir = source_home.join("config/jcode");
    let github_config_dir = home_dir.join(".config/github-copilot");
    fs::create_dir_all(&app_config_dir).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&github_config_dir).unwrap_or_else(|error| panic!("{error}"));
    let _home = crate::test_support::EnvGuard::set(
        "HOME",
        home_dir
            .to_str()
            .unwrap_or_else(|| panic!("test home path is not UTF-8")),
    );
    let _jcode_home = crate::test_support::EnvGuard::set(
        JCODE_HOME_ENV,
        source_home
            .to_str()
            .unwrap_or_else(|| panic!("test jcode home path is not UTF-8")),
    );

    let catalog_fixtures: [(&str, &[u8]); 3] = [
        ("openai_model_catalog_cache.json", b"openai-cache"),
        ("anthropic_model_catalog_cache.json", b"anthropic-cache"),
        ("copilot_models_cache.json", b"copilot-cache"),
    ];
    for (name, contents) in catalog_fixtures {
        fs::write(app_config_dir.join(name), contents).unwrap_or_else(|error| panic!("{error}"));
    }
    let hosts_path = github_config_dir.join("hosts.json");
    let apps_path = github_config_dir.join("apps.json");
    fs::write(&hosts_path, b"trusted copilot credentials")
        .unwrap_or_else(|error| panic!("{error}"));
    fs::write(&apps_path, b"untrusted copilot credentials")
        .unwrap_or_else(|error| panic!("{error}"));
    write_trusted_external_config(&source_home, &hosts_path);

    let session_id = "external-auth-copy-test";
    let session_key = session_storage_key(session_id);
    let mcp_servers = crate::config::McpServers::default();
    let mut home = prepare_session_home(source_home.clone(), &session_key, None, &mcp_servers)
        .unwrap_or_else(|error| panic!("{error}"));
    let storage_home = home.storage_home.clone();
    let private_hosts = storage_home.join("external/.config/github-copilot/hosts.json");
    let private_apps = storage_home.join("external/.config/github-copilot/apps.json");
    assert_private_copied_file(&private_hosts, b"trusted copilot credentials");
    assert_private_copied_file(&private_apps, b"untrusted copilot credentials");
    assert_private_model_catalog_copies(&storage_home, &catalog_fixtures);
    let private_hosts_entry = path_trust_entry("copilot_hosts_json", &private_hosts);
    let private_apps_entry = path_trust_entry("copilot_apps_json", &private_apps);
    assert_external_auth_trust(&storage_home, &private_hosts_entry, &private_apps_entry, 1);

    fs::write(
        app_config_dir.join("openai_model_catalog_cache.json"),
        b"updated openai cache",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    fs::write(&hosts_path, b"updated trusted credentials")
        .unwrap_or_else(|error| panic!("{error}"));
    fs::write(&apps_path, b"updated untrusted credentials")
        .unwrap_or_else(|error| panic!("{error}"));
    finish_session_home(&mut home, Some(session_id), true)
        .unwrap_or_else(|error| panic!("{error}"));
    drop(home);

    let mut resumed =
        prepare_session_home(source_home, &session_key, Some(session_id), &mcp_servers)
            .unwrap_or_else(|error| panic!("{error}"));
    let updated_catalog_fixtures = [
        (
            "openai_model_catalog_cache.json",
            &b"updated openai cache"[..],
        ),
        (
            "anthropic_model_catalog_cache.json",
            &b"anthropic-cache"[..],
        ),
        ("copilot_models_cache.json", &b"copilot-cache"[..]),
    ];
    assert_private_copied_file(&private_hosts, b"updated trusted credentials");
    assert_private_copied_file(&private_apps, b"updated untrusted credentials");
    assert_private_model_catalog_copies(&storage_home, &updated_catalog_fixtures);
    assert_external_auth_trust(&storage_home, &private_hosts_entry, &private_apps_entry, 1);
    finish_session_home(&mut resumed, None, false).unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn fresh_and_resumed_session_alias_socket_paths_are_valid() {
    let _process_lock = crate::test_support::lock_process();

    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source_home = temp.path().join("Users").join("cruise-user").join(".jcode");
    fs::create_dir_all(&source_home).unwrap_or_else(|error| panic!("{error}"));
    let user_home = temp.path().join("user-home");
    let _home = crate::test_support::EnvGuard::set(
        "HOME",
        user_home.to_str().unwrap_or_else(|| panic!("utf8")),
    );
    let _jcode_home = crate::test_support::EnvGuard::set(
        JCODE_HOME_ENV,
        source_home.to_str().unwrap_or_else(|| panic!("utf8")),
    );
    let mcp_servers = crate::config::McpServers::default();
    let assert_alias = |sdk_home: &Path| {
        let socket = sdk_home.join("run/jcode-api.sock");
        assert!(sdk_home.starts_with(short_home_alias_root()));
        assert_eq!(
            sdk_home.file_name().and_then(OsStr::to_str).map(str::len),
            Some(16)
        );
        assert!(
            std::os::unix::net::SocketAddr::from_pathname(&socket).is_ok(),
            "{} is too long",
            socket.display()
        );
    };
    let fresh_key = uuid::Uuid::new_v4().simple().to_string();
    let mut fresh = prepare_session_home(source_home.clone(), &fresh_key, None, &mcp_servers)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_alias(&fresh.sdk_home);
    finish_session_home(&mut fresh, None, false).unwrap_or_else(|error| panic!("{error}"));

    let session_id = "resumed-session-for-socket-path-regression";
    let storage_home = source_home
        .join(SESSION_HOME_DIR)
        .join(session_storage_key(session_id));
    fs::create_dir_all(&storage_home).unwrap_or_else(|error| panic!("{error}"));
    fs::write(storage_home.join(SESSION_ID_FILE), session_id)
        .unwrap_or_else(|error| panic!("{error}"));
    let mut resumed = prepare_session_home(
        source_home,
        &session_storage_key(session_id),
        Some(session_id),
        &mcp_servers,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(resumed.resume_session_found);
    assert_alias(&resumed.sdk_home);
    finish_session_home(&mut resumed, None, false).unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn sdk_socket_path_preflight_rejects_an_overlong_home() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let sdk_home = temp.path().join("x".repeat(120));
    let Err(error) = validate_sdk_socket_paths(&sdk_home) else {
        panic!("expected overlong socket path to fail")
    };
    let debug_socket = sdk_home.join("run/jcode-debug.sock");
    assert!(error.contains("jcode SDK socket path"));
    assert!(error.contains(&debug_socket.display().to_string()));
}

#[test]
fn non_resumable_prompt_home_is_removed_after_the_turn() {
    let (_temp, mut home) = session_home();
    let storage_home = home.storage_home.clone();
    finish_session_home(&mut home, Some("transient-session"), false)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!storage_home.exists());
}

#[test]
fn deleting_cruise_session_removes_its_private_plan_home() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let session_id = "persisted-plan-session";
    let home = temp
        .path()
        .join(SESSION_HOME_DIR)
        .join(session_storage_key(session_id));
    fs::create_dir_all(&home).unwrap_or_else(|error| panic!("{error}"));
    fs::write(home.join(SESSION_ID_FILE), session_id).unwrap_or_else(|error| panic!("{error}"));
    cleanup_session_home_at(temp.path(), session_id).unwrap_or_else(|error| panic!("{error}"));
    assert!(!home.exists());
}
#[test]
fn stale_registry_pid_is_not_signalled_when_process_is_not_jcode_daemon() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let home = temp.path().join("session-home");
    let runtime = home.join("run");
    fs::create_dir_all(&runtime).unwrap_or_else(|error| panic!("{error}"));
    let socket = runtime.join("jcode.sock");
    let mut unrelated = Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap_or_else(|error| panic!("{error}"));
    fs::write(
        home.join("servers.json"),
        serde_json::json!({
            "unrelated": {
                "pid": unrelated.id(),
                "socket": socket,
            }
        })
        .to_string(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let cleanup = clear_private_daemon(&home);
    let still_running = unrelated
        .try_wait()
        .unwrap_or_else(|error| panic!("{error}"))
        .is_none();
    let _ = unrelated.kill();
    let _ = unrelated.wait();
    cleanup.unwrap_or_else(|error| panic!("{error}"));
    assert!(still_running, "the stale registry PID was not jcode serve");
    assert!(!home.join("servers.json").exists());
}
#[test]
fn private_daemon_identity_matches_its_private_runtime() {
    let home = PathBuf::from("/tmp/cruise-private-home");
    let runtime = home.join("run");
    let socket = runtime.join("jcode.sock");
    let command = "/opt/homebrew/bin/jcode --provider auto serve";
    let environment = format!(
        "JCODE_HOME={} JCODE_RUNTIME_DIR={} JCODE_SOCKET={}",
        home.display(),
        runtime.display(),
        socket.display()
    );

    assert!(private_daemon_identity_matches(
        command,
        &environment,
        &socket
    ));
    assert!(!private_daemon_identity_matches(
        command,
        &environment,
        Path::new("/tmp/another-home/run/jcode.sock")
    ));
    assert!(!private_daemon_identity_matches(
        "/bin/sleep 30",
        &environment,
        &socket
    ));
}
#[test]
fn private_daemon_without_registry_is_stopped_by_its_socket_identity() {
    use std::os::unix::process::CommandExt as _;

    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let socket = temp.path().join("run/jcode.sock");
    fs::create_dir_all(socket.parent().unwrap_or_else(|| panic!("socket parent")))
        .unwrap_or_else(|error| panic!("{error}"));
    let mut command = Command::new("sleep");
    command
        .arg("30")
        .arg0(format!("jcode --socket {} serve", socket.display()));
    let mut daemon = command.spawn().unwrap_or_else(|error| panic!("{error}"));

    let cleanup = stop_private_daemons(std::slice::from_ref(&socket));
    let stopped = daemon
        .try_wait()
        .unwrap_or_else(|error| panic!("{error}"))
        .is_some();
    if !stopped {
        let _ = daemon.kill();
        let _ = daemon.wait();
    }
    cleanup.unwrap_or_else(|error| panic!("{error}"));
    assert!(
        stopped,
        "the daemon matching this private socket should stop"
    );
}
#[test]
fn relative_jcode_home_is_resolved_against_the_run_working_directory() {
    let _process_lock = crate::test_support::lock_process();
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let _jcode_home = crate::test_support::EnvGuard::set(JCODE_HOME_ENV, "relative-jcode-home");
    let working_dir = temp.path().join("checkout");
    fs::create_dir_all(&working_dir).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        resolve_source_home(Some(&working_dir)),
        working_dir.join("relative-jcode-home")
    );
}

#[test]
fn stale_unclaimed_homes_are_pruned_only_when_their_lock_is_free() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let abandoned = temp.path().join("abandoned");
    let retained = temp.path().join("retained");
    let active = temp.path().join("active");
    fs::create_dir_all(&abandoned).unwrap_or_else(|error| panic!("{error}"));
    fs::write(abandoned.join(HOME_LOCK_FILE), []).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&retained).unwrap_or_else(|error| panic!("{error}"));
    fs::write(retained.join(SESSION_ID_FILE), "persisted")
        .unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&active).unwrap_or_else(|error| panic!("{error}"));
    let _active_lock = lock_session_home(&active)
        .unwrap_or_else(|error| panic!("{error}"))
        .unwrap_or_else(|| panic!("unix session home lock"));
    let now = SystemTime::now() + STALE_SESSION_HOME_AGE + Duration::from_secs(1);
    prune_unclaimed_session_homes_at(temp.path(), now).unwrap_or_else(|error| panic!("{error}"));
    assert!(!abandoned.exists());
    assert!(retained.exists());
    assert!(active.exists());
}

#[test]
fn explicit_provider_and_model_override_environment_defaults_at_launch() {
    let workflow = HashMap::from([
        (JCODE_PROVIDER_ENV.to_string(), "wrong-provider".to_string()),
        (JCODE_MODEL_ENV.to_string(), "wrong-model".to_string()),
    ]);
    let env = launch_environment(
        &workflow,
        Some("anthropic-api"),
        Some("claude-opus"),
        true,
        None,
        None,
    );
    assert_eq!(
        env.get(OsStr::new(JCODE_PROVIDER_ENV))
            .and_then(|value| value.to_str()),
        Some("anthropic-api")
    );
    assert_eq!(
        env.get(OsStr::new(JCODE_MODEL_ENV))
            .and_then(|value| value.to_str()),
        Some("claude-api:claude-opus")
    );
}

#[test]
fn missing_global_mcp_leaves_absent_session_copy_absent() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source_home = temp.path().join("source");
    let storage_home = temp.path().join("session");
    fs::create_dir_all(&source_home).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&storage_home).unwrap_or_else(|error| panic!("{error}"));
    let destination = storage_home.join("mcp.json");

    write_session_mcp(
        &source_home,
        &storage_home,
        &crate::config::McpServers::new(),
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert!(
        !destination.exists(),
        "no session MCP file should be created"
    );
}

#[test]
fn missing_global_mcp_removes_a_stale_session_copy() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source_home = temp.path().join("source");
    let storage_home = temp.path().join("session");
    fs::create_dir_all(&source_home).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&storage_home).unwrap_or_else(|error| panic!("{error}"));
    let destination = storage_home.join("mcp.json");
    fs::write(&destination, b"stale").unwrap_or_else(|error| panic!("{error}"));

    write_session_mcp(
        &source_home,
        &storage_home,
        &crate::config::McpServers::new(),
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert!(
        !destination.exists(),
        "stale session MCP config must be removed"
    );
}

#[test]
fn invalid_global_mcp_is_copied_verbatim_when_no_workflow_mcp_is_configured() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source_home = temp.path().join("source");
    let storage_home = temp.path().join("session");
    fs::create_dir_all(&source_home).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&storage_home).unwrap_or_else(|error| panic!("{error}"));
    let source = source_home.join("mcp.json");
    let destination = storage_home.join("mcp.json");
    let contents = b"not valid JSON\n";
    fs::write(&source, contents).unwrap_or_else(|error| panic!("{error}"));

    write_session_mcp(
        &source_home,
        &storage_home,
        &crate::config::McpServers::new(),
    )
    .unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(
        fs::read(destination).unwrap_or_else(|error| panic!("{error}")),
        contents,
        "an invalid source file must retain today's verbatim-copy behavior"
    );
}

#[test]
fn global_mcp_copy_strips_the_cruise_server_without_modifying_the_source() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source_home = temp.path().join("source");
    let storage_home = temp.path().join("session");
    fs::create_dir_all(&source_home).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&storage_home).unwrap_or_else(|error| panic!("{error}"));
    let source = source_home.join("mcp.json");
    let destination = storage_home.join("mcp.json");
    let contents =
        br#"{"mcpServers":{"cruise":{"command":"cruise"},"external":{"command":"node"}}}"#;
    fs::write(&source, contents).unwrap_or_else(|error| panic!("{error}"));

    write_session_mcp(
        &source_home,
        &storage_home,
        &crate::config::McpServers::new(),
    )
    .unwrap_or_else(|error| panic!("{error}"));

    let copied: serde_json::Value =
        serde_json::from_slice(&fs::read(destination).unwrap_or_else(|error| panic!("{error}")))
            .unwrap_or_else(|error| panic!("{error}"));
    assert!(copied["mcpServers"]["cruise"].is_null());
    assert_eq!(
        copied["mcpServers"]["external"]["command"].as_str(),
        Some("node")
    );
    assert_eq!(
        fs::read(source).unwrap_or_else(|error| panic!("{error}")),
        contents,
        "copying session MCP config must not modify the user's source"
    );
}

fn workflow_mcp_servers() -> crate::config::McpServers {
    let mut servers = crate::config::McpServers::new();
    servers.insert(
        "workflow_tool".to_string(),
        crate::config::McpServerConfig {
            command: Some("workflow-command".to_string()),
            args: vec!["server.js".to_string()],
            ..Default::default()
        },
    );
    servers
}

#[test]
fn workflow_mcp_is_written_when_global_config_is_missing() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source_home = temp.path().join("source");
    let storage_home = temp.path().join("session");
    fs::create_dir_all(&source_home).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&storage_home).unwrap_or_else(|error| panic!("{error}"));
    let destination = storage_home.join("mcp.json");

    write_session_mcp(&source_home, &storage_home, &workflow_mcp_servers())
        .unwrap_or_else(|error| panic!("{error}"));

    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(&destination).unwrap_or_else(|error| panic!("{error}")))
            .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(written.as_object().map(serde_json::Map::len), Some(1));
    assert_eq!(
        written["mcpServers"]["workflow_tool"]["type"].as_str(),
        Some("stdio")
    );
    assert_eq!(
        written["mcpServers"]["workflow_tool"]["command"].as_str(),
        Some("workflow-command")
    );
    assert_eq!(
        fs::metadata(destination)
            .unwrap_or_else(|error| panic!("{error}"))
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn workflow_mcp_merges_over_global_servers_and_strips_cruise_without_modifying_source() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source_home = temp.path().join("source");
    let storage_home = temp.path().join("session");
    fs::create_dir_all(&source_home).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&storage_home).unwrap_or_else(|error| panic!("{error}"));
    let source = source_home.join("mcp.json");
    let destination = storage_home.join("mcp.json");
    let source_contents = br#"{"mcpServers":{"cruise":{"command":"old-cruise"},"workflow_tool":{"command":"global-command"},"global_tool":{"command":"global"}},"other":"preserved"}"#;
    fs::write(&source, source_contents).unwrap_or_else(|error| panic!("{error}"));

    write_session_mcp(&source_home, &storage_home, &workflow_mcp_servers())
        .unwrap_or_else(|error| panic!("{error}"));

    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(destination).unwrap_or_else(|error| panic!("{error}")))
            .unwrap_or_else(|error| panic!("{error}"));
    assert!(written["mcpServers"]["cruise"].is_null());
    assert_eq!(
        written["mcpServers"]["workflow_tool"]["command"].as_str(),
        Some("workflow-command")
    );
    assert_eq!(
        written["mcpServers"]["global_tool"]["command"].as_str(),
        Some("global")
    );
    assert_eq!(written["other"].as_str(), Some("preserved"));
    assert!(written.get("servers").is_none());
    assert_eq!(
        fs::read(source).unwrap_or_else(|error| panic!("{error}")),
        source_contents
    );
}

#[test]
fn workflow_mcp_preserves_the_existing_servers_alias() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source_home = temp.path().join("source");
    let storage_home = temp.path().join("session");
    fs::create_dir_all(&source_home).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&storage_home).unwrap_or_else(|error| panic!("{error}"));
    let source = source_home.join("mcp.json");
    let destination = storage_home.join("mcp.json");
    fs::write(
        &source,
        br#"{"servers":{"global_tool":{"command":"global"}}}"#,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    write_session_mcp(&source_home, &storage_home, &workflow_mcp_servers())
        .unwrap_or_else(|error| panic!("{error}"));

    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(destination).unwrap_or_else(|error| panic!("{error}")))
            .unwrap_or_else(|error| panic!("{error}"));
    assert!(written.get("mcpServers").is_none());
    assert_eq!(
        written["servers"]["workflow_tool"]["command"].as_str(),
        Some("workflow-command")
    );
    assert_eq!(
        written["servers"]["global_tool"]["command"].as_str(),
        Some("global")
    );
}

#[test]
fn invalid_global_mcp_errors_when_workflow_servers_need_merging() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source_home = temp.path().join("source");
    let storage_home = temp.path().join("session");
    fs::create_dir_all(&source_home).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&storage_home).unwrap_or_else(|error| panic!("{error}"));
    let source = source_home.join("mcp.json");
    let source_contents = b"invalid source JSON\n";
    fs::write(&source, source_contents).unwrap_or_else(|error| panic!("{error}"));

    let Err(error) = write_session_mcp(&source_home, &storage_home, &workflow_mcp_servers()) else {
        panic!("invalid source JSON must reject a workflow MCP merge");
    };

    assert!(
        error
            .to_string()
            .contains("cannot merge workflow mcp_servers")
    );
    assert_eq!(
        fs::read(source).unwrap_or_else(|error| panic!("{error}")),
        source_contents
    );
}

#[test]
fn workflow_mcp_merge_rejects_non_object_server_maps() {
    let temp = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let source_home = temp.path().join("source");
    let storage_home = temp.path().join("session");
    fs::create_dir_all(&source_home).unwrap_or_else(|error| panic!("{error}"));
    fs::create_dir_all(&storage_home).unwrap_or_else(|error| panic!("{error}"));
    fs::write(source_home.join("mcp.json"), br#"{"mcpServers":[]}"#)
        .unwrap_or_else(|error| panic!("{error}"));

    let Err(error) = write_session_mcp(&source_home, &storage_home, &workflow_mcp_servers()) else {
        panic!("a non-object mcpServers value must reject a workflow MCP merge");
    };

    assert!(
        error
            .to_string()
            .contains("`mcpServers` must be a JSON object")
    );
}
