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
    let env = launch_environment(&workflow, None, None);
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
fn private_runtime_disables_auto_update_with_environment_override() {
    let env = launch_environment(
        &HashMap::from([(JCODE_CHECK_UPDATES_ENV.to_string(), "1".to_string())]),
        None,
        None,
    );
    assert_eq!(
        env.get(OsStr::new(JCODE_CHECK_UPDATES_ENV))
            .and_then(|value| value.to_str()),
        Some("0")
    );
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
    let env = launch_environment(&workflow, Some("anthropic-api"), Some("claude-opus"));
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
