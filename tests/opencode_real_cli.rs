#![cfg(unix)]
//! Opt-in installed V2 acceptance, without user credentials or paid providers.
//! Run: `cargo test --test opencode_real_cli -- --ignored --nocapture`
//! `OPENCODE_REAL_CLI` selects an installed executable (default: `/opt/homebrew/bin/opencode`).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::response::IntoResponse as _;
use axum::{Json, Router};
use cruise::ask_handler::AskHandler;
use cruise::cancellation::CancellationToken;
use cruise::config::PermissionMode;
use cruise::executor::{Executor, PromptRun};
use cruise::sdk_tools::{self, PrMetadata};
use cruise::test_support::{EnvGuard, lock_process, set_fake_home, write_executable_script};
use serde_json::{Value, json};
use tempfile::TempDir;

#[derive(Default)]
struct Model {
    requests: Mutex<Vec<Value>>,
    hang: std::sync::atomic::AtomicBool,
    calls: Mutex<VecDeque<(&'static str, Value)>>,
    next_call: std::sync::atomic::AtomicUsize,
}

struct LocalAsk {
    checkpoint: Arc<Mutex<Option<String>>>,
    questions: Mutex<Vec<String>>,
}

impl AskHandler for LocalAsk {
    fn ask_user(&self, question: &str) -> cruise::error::Result<String> {
        assert!(
            self.checkpoint
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .is_some(),
            "session must be checkpointed before asking"
        );
        self.questions
            .lock()
            .unwrap_or_else(|e| panic!("{e}"))
            .push(question.to_owned());
        Ok(format!("LOCAL_ANSWER: {question}"))
    }
}

async fn respond(
    State(model): State<Arc<Model>>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    tokio::task::yield_now().await;
    model
        .requests
        .lock()
        .unwrap_or_else(|e| panic!("{e}"))
        .push(body.clone());
    if model.hang.load(std::sync::atomic::Ordering::SeqCst) {
        return (
            [("content-type", "text/event-stream")],
            axum::body::Body::from_stream(futures::stream::pending::<
                Result<String, std::convert::Infallible>,
            >()),
        )
            .into_response();
    }
    let call = {
        let mut calls = model.calls.lock().unwrap_or_else(|e| panic!("{e}"));
        let available = calls.front().is_some_and(|(name, _)| {
            body["tools"]
                .as_array()
                .is_some_and(|tools| tools.iter().any(|tool| tool["function"]["name"] == *name))
        });
        if available { calls.pop_front() } else { None }
    };
    let (delta, finish) = if let Some((name, input)) = call {
        let index = model
            .next_call
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        (
            json!({"role":"assistant","tool_calls":[{"index":0,"id":format!("call_local_{index}"),"type":"function","function":{"name":name,"arguments":input.to_string()}}]}),
            "tool_calls",
        )
    } else {
        (
            json!({"role":"assistant","content":"LOCAL_V2_SMOKE_OK"}),
            "stop",
        )
    };
    let chunk = |delta: Value, finish: Value| {
        json!({"id":"chatcmpl_local", "object":"chat.completion.chunk", "created":1, "model":"smoke", "choices":[{"index":0,"delta":delta,"finish_reason":finish}]}).to_string()
    };
    let output = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        chunk(delta, Value::Null),
        chunk(json!({}), json!(finish))
    );
    ([("content-type", "text/event-stream")], output).into_response()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[expect(
    clippy::too_many_lines,
    reason = "keeps the ordered real CLI setup, resume, and cancellation acceptance in one scenario"
)]
#[ignore = "requires an explicitly selected installed OpenCode V2 CLI, never a provider account"]
async fn installed_v2_local_model_plugin_turn_resume_and_cancel() {
    let _lock = lock_process();
    let root = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
    let executable = std::env::var_os("OPENCODE_REAL_CLI").map_or_else(
        || PathBuf::from("/opt/homebrew/bin/opencode"),
        PathBuf::from,
    );
    assert!(
        executable.is_file(),
        "set OPENCODE_REAL_CLI to the installed V2 executable"
    );
    let home = root.path().join("home");
    std::fs::create_dir_all(&home).unwrap_or_else(|e| panic!("{e}"));
    let _home = set_fake_home(&home);
    let _xdg: Vec<_> = [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
    ]
    .into_iter()
    .map(|(key, directory)| EnvGuard::set(key, root.path().join(directory)))
    .collect();
    let bin = root.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap_or_else(|e| panic!("{e}"));
    // Record every actual server process, not the --version probe.
    write_executable_script(
        &bin.join("opencode"),
        &format!(
            "#!/bin/sh\nif [ \"$1\" = serve ]; then echo $$ >> '{}'; fi\nexec '{}' \"$@\"\n",
            root.path().join("pids").display(),
            executable.display()
        ),
    );
    for other_backend in ["jcode", "claude"] {
        write_executable_script(&bin.join(other_backend), "#!/bin/sh\nexit 97\n");
    }
    let _path = cruise::test_support::prepend_to_path(&bin);
    let model = Arc::new(Model::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let address = listener.local_addr().unwrap_or_else(|e| panic!("{e}"));
    let app = Router::new()
        .route("/chat/completions", axum::routing::post(respond))
        .with_state(model.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
    });
    let config = json!({
        "update":"disable", "share":"disabled", "snapshots":false, "compaction":{"auto":false},
        "model":"local-smoke/smoke",
        "providers":{"local-smoke":{"package":"@opencode/ai/providers/openai-compatible", "env":[],
            "settings":{"baseURL":format!("http://{address}"),"apiKey":"local-test-not-a-credential"},
            "models":{"smoke":{"name":"Deterministic local smoke", "limit":{"context":262_144,"output":1024},"capabilities":{"tools":true}}}}}
    });
    let _config = EnvGuard::set("OPENCODE_CONFIG_CONTENT", config.to_string());
    let _no_auth = EnvGuard::remove("OPENCODE_AUTH_JSON");
    let _config_path = EnvGuard::remove("OPENCODE_CONFIG");
    let _config_dir = EnvGuard::remove("OPENCODE_CONFIG_DIR");
    let repo = root.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e}"));
    cruise::test_support::init_git_repo(&repo);
    std::fs::write(repo.join("read-only-sentinel"), "unchanged").unwrap_or_else(|e| panic!("{e}"));
    let checkpoint = Arc::new(Mutex::new(None));
    let ask = Arc::new(LocalAsk {
        checkpoint: checkpoint.clone(),
        questions: Mutex::new(Vec::new()),
    });
    let plan = root.path().join("session/plan.md");
    let planning = sdk_tools::planning_tools(plan.clone(), ask.clone(), true, None);
    let title = Arc::new(Mutex::new(None::<String>));
    let metadata = Arc::new(Mutex::new(None::<PrMetadata>));
    let skip = Arc::new(Mutex::new(None::<String>));
    let mut tools = planning.tools;
    tools.extend([
        sdk_tools::generate_title_tool(title.clone()),
        sdk_tools::submit_pr_metadata_tool(metadata.clone()),
        sdk_tools::skip_step_tool(skip.clone()),
    ]);
    *model.calls.lock().unwrap_or_else(|e| panic!("{e}")) = VecDeque::from([
        (
            sdk_tools::ASK_USER_TOOL,
            json!({"question":"Which local acceptance plan?"}),
        ),
        (
            sdk_tools::SUBMIT_PLAN_TOOL,
            json!({"content":"# Local plan\nInitial implementation\n"}),
        ),
        (
            sdk_tools::GENERATE_TITLE_TOOL,
            json!({"title":"Local smoke title"}),
        ),
    ]);
    let env = HashMap::new();
    let mcp = cruise::config::McpServers::new();
    let executor = Executor::new(Some("opencode"), &[]);
    let on_session = |id: &cruise::model_selection::SdkSessionId| {
        *checkpoint.lock().unwrap_or_else(|e| panic!("{e}")) = Some(id.id.clone());
        Ok(())
    };
    let selection = cruise::config::ModelSpec::Single("local-smoke/smoke".to_owned());
    let request = |resume, cancel_token| PromptRun {
        prompt: "Run the local acceptance tools, then return LOCAL_V2_SMOKE_OK",
        model: Some(&selection),
        max_retries: 0,
        env: &env,
        mcp_servers: &mcp,
        on_notice: None,
        cancel_token,
        working_dir: Some(&repo),
        stream: None,
        tools: tools.clone(),
        on_session_id: Some(&on_session),
        resume,
        computer_use: false,
        permission: PermissionMode::ReadOnly,
    };
    let first = tokio::time::timeout(Duration::from_secs(90), executor.run(request(None, None)))
        .await
        .unwrap_or_else(|e| panic!("real V2 turn timed out: {e}"))
        .unwrap_or_else(|e| panic!("real V2 local-model turn: {e}"));
    assert_eq!(first.result.output.trim(), "LOCAL_V2_SMOKE_OK");
    assert_eq!(
        *title.lock().unwrap_or_else(|e| panic!("{e}")),
        Some("Local smoke title".to_owned())
    );
    assert!(
        planning
            .plan_persisted
            .load(std::sync::atomic::Ordering::SeqCst)
    );
    assert_eq!(
        std::fs::read_to_string(&plan).unwrap_or_else(|e| panic!("{e}")),
        "# Local plan\nInitial implementation\n"
    );
    assert!(
        model
            .calls
            .lock()
            .unwrap_or_else(|e| panic!("{e}"))
            .is_empty()
    );
    let session = first.session_id.as_ref().map_or_else(
        || panic!("real session checkpoint missing"),
        |session| session.id.as_str(),
    );
    assert_eq!(
        checkpoint
            .lock()
            .unwrap_or_else(|e| panic!("{e}"))
            .as_deref(),
        Some(session)
    );
    let before_resume = model
        .requests
        .lock()
        .unwrap_or_else(|e| panic!("{e}"))
        .len();
    planning
        .plan_persisted
        .store(false, std::sync::atomic::Ordering::SeqCst);
    *model.calls.lock().unwrap_or_else(|e| panic!("{e}")) = VecDeque::from([
        (
            sdk_tools::ASK_USER_TOOL,
            json!({"question":"Confirm the persisted local answer after restart?"}),
        ),
        (
            sdk_tools::UPDATE_PLAN_TOOL,
            json!({"old":"Initial implementation","new":"Revised after restart"}),
        ),
        (
            sdk_tools::SUBMIT_PR_METADATA_TOOL,
            json!({"title":"Local PR title","body":"## Local PR\nDeterministic acceptance only."}),
        ),
        (
            sdk_tools::SKIP_STEP_TOOL,
            json!({"reason":"Read-only acceptance deliberately leaves the repository unchanged."}),
        ),
    ]);
    let second = tokio::time::timeout(
        Duration::from_secs(60),
        executor.run(request(
            Some(cruise::model_selection::SdkSessionId {
                sdk: cruise::model_selection::Sdk::OpenCode,
                id: session.to_owned(),
            }),
            None,
        )),
    )
    .await
    .unwrap_or_else(|e| panic!("resume timeout: {e}"))
    .unwrap_or_else(|e| panic!("resume: {e}"));
    assert_eq!(
        second
            .session_id
            .as_ref()
            .map(|session| session.id.as_str()),
        Some(session)
    );
    assert_eq!(second.result.output.trim(), "LOCAL_V2_SMOKE_OK");
    assert!(
        planning
            .plan_persisted
            .load(std::sync::atomic::Ordering::SeqCst)
    );
    assert_eq!(
        std::fs::read_to_string(&plan).unwrap_or_else(|e| panic!("{e}")),
        "# Local plan\nRevised after restart\n"
    );
    assert_eq!(
        *ask.questions.lock().unwrap_or_else(|e| panic!("{e}")),
        [
            "Which local acceptance plan?",
            "Confirm the persisted local answer after restart?"
        ]
    );
    {
        let metadata = metadata.lock().unwrap_or_else(|e| panic!("{e}"));
        let metadata = metadata
            .as_ref()
            .unwrap_or_else(|| panic!("PR metadata tool was not executed"));
        assert_eq!(metadata.title, "Local PR title");
        assert_eq!(metadata.body, "## Local PR\nDeterministic acceptance only.");
    }
    assert_eq!(
        skip.lock().unwrap_or_else(|e| panic!("{e}")).as_deref(),
        Some("Read-only acceptance deliberately leaves the repository unchanged.")
    );
    assert!(
        model
            .calls
            .lock()
            .unwrap_or_else(|e| panic!("{e}"))
            .is_empty()
    );
    {
        let requests = model.requests.lock().unwrap_or_else(|e| panic!("{e}"));
        assert!(
            requests[before_resume..]
                .iter()
                .any(|request| request["messages"]
                    .as_array()
                    .is_some_and(|messages| messages.iter().any(|message| message["role"]
                        == "tool"
                        && message
                            .to_string()
                            .contains("LOCAL_ANSWER: Which local acceptance plan?")))),
            "restarted CLI must restore the prior question's durable answer"
        );
        for index in 0..7 {
            let call_id = format!("call_local_{index}");
            assert!(
                requests
                    .iter()
                    .any(|request| request["messages"]
                        .as_array()
                        .is_some_and(|messages| messages.iter().any(|message| message["role"]
                            == "tool"
                            && message["tool_call_id"] == call_id
                            && message["content"]
                                .as_str()
                                .is_some_and(|content| !content.is_empty())))),
                "native tool result missing from model history for {call_id}"
            );
        }
        let allowed = [
            "read",
            "glob",
            "grep",
            "list",
            "webfetch",
            "websearch",
            "ask_user",
            "submit_plan",
            "update_plan",
            "generate_title",
            "submit_pr_metadata",
            "skip_step",
        ];
        for request in requests.iter() {
            for tool in request["tools"].as_array().into_iter().flatten() {
                let name = tool["function"]["name"].as_str().unwrap_or_default();
                assert!(
                    allowed.contains(&name),
                    "read-only exposes an unsafe/unknown tool: {name}"
                );
            }
        }
    }
    let workflow = root.path().join("real-workflow.yaml");
    std::fs::write(&workflow, "sdk: opencode\nmodel: local-smoke/smoke\npermission: read-only\ninteractive_planning: false\nsteps:\n  implement:\n    prompt: '{input}'\n").unwrap_or_else(|e| panic!("{e}"));
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CRUISE_") {
            command.env_remove(key);
        }
    }
    command
        .args(["exec", "-c"])
        .arg(&workflow)
        .arg("local deterministic acceptance")
        .current_dir(&repo)
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(90), command.output())
        .await
        .unwrap_or_else(|e| panic!("real cruise exec timed out: {e}"))
        .unwrap_or_else(|e| panic!("real cruise exec: {e}"));
    let terminal = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "real OpenCode YAML workflow failed: {terminal}"
    );
    assert_eq!(
        terminal.matches("LOCAL_V2_SMOKE_OK").count(),
        1,
        "real workflow must print model output exactly once: {terminal}"
    );
    model.hang.store(true, std::sync::atomic::Ordering::SeqCst);
    let before = model
        .requests
        .lock()
        .unwrap_or_else(|e| panic!("{e}"))
        .len();
    let token = CancellationToken::new();
    let cancel = async {
        tokio::time::timeout(Duration::from_secs(30), async {
            while model
                .requests
                .lock()
                .unwrap_or_else(|e| panic!("{e}"))
                .len()
                == before
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|e| panic!("cancel turn never reached local model: {e}"));
        token.cancel();
    };
    let (cancelled, ()) = tokio::time::timeout(Duration::from_secs(45), async {
        tokio::join!(
            executor.run(request(
                Some(cruise::model_selection::SdkSessionId {
                    sdk: cruise::model_selection::Sdk::OpenCode,
                    id: session.to_owned()
                }),
                Some(&token)
            )),
            cancel
        )
    })
    .await
    .unwrap_or_else(|e| panic!("cancel did not drain installed CLI: {e}"));
    assert!(cancelled.is_err(), "cancelled model stream cannot succeed");
    let pids = std::fs::read_to_string(root.path().join("pids")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        pids.lines().count(),
        4,
        "one owned CLI per attempt, including cruise exec"
    );
    for pid in pids.lines() {
        let pid: i32 = pid.parse().unwrap_or_else(|e| panic!("{e}"));
        // SAFETY: signal zero checks existence without signalling any process.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "real CLI leaked: {pid}");
    }
    assert!(!repo.join(".opencode").exists());
    assert_eq!(
        std::fs::read_to_string(repo.join("read-only-sentinel")).unwrap_or_else(|e| panic!("{e}")),
        "unchanged"
    );
    server.abort();
}
