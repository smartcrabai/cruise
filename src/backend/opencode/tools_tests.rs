use super::*;
use crate::{ask_handler::ScriptedAskHandler, backend::tool::CruiseTool, sdk_tools};
use std::sync::atomic::AtomicUsize;

struct ReleaseOnDrop(Arc<AtomicBool>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[test]
fn pending_secret_removes_failed_writes_without_removing_retained_data() {
    use std::io::Write;

    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let plugin = root.path().join("plugin");
    private_dir(&plugin).unwrap_or_else(|error| panic!("{error}"));
    for name in ["manifest.json", "opencode.db"] {
        private_write(&root.path().join(name), b"retained")
            .unwrap_or_else(|error| panic!("{error}"));
    }
    let secret = plugin.join("bridge.json");
    // Inject errors after creation, a partial write, and a full write before sync.
    for bytes in [
        b"".as_slice(),
        b"{\"token\":".as_slice(),
        b"{\"token\":\"secret\"}".as_slice(),
    ] {
        let result = (|| -> std::io::Result<()> {
            let _pending = PendingSecret(Some(secret.clone()));
            let mut file = std::fs::File::create(&secret)?;
            file.write_all(bytes)?;
            Err(std::io::Error::other("injected write or sync failure"))
        })();
        assert!(result.is_err());
        assert!(!secret.exists(), "failed write left an attempt secret");
        for name in ["manifest.json", "opencode.db"] {
            assert_eq!(
                std::fs::read(root.path().join(name)).unwrap_or_else(|error| panic!("{error}")),
                b"retained"
            );
        }
    }
}

fn runner(tools: Vec<CruiseTool>, permission: PermissionMode) -> OpenCodeRunnerConfig {
    OpenCodeRunnerConfig {
        tools,
        permission,
        env: HashMap::from([("OPENCODE_CONFIG_CONTENT".to_string(), "{}".to_string())]),
        ..OpenCodeRunnerConfig::default()
    }
}

struct Fixture {
    root: tempfile::TempDir,
    bridge: Bridge,
    connection: Value,
    client: reqwest::Client,
    _chunks: std::sync::mpsc::Receiver<StreamChunk>,
}

impl Fixture {
    async fn new(config: &OpenCodeRunnerConfig, cancel: CancellationToken) -> Self {
        let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let (tx, chunks) = std::sync::mpsc::sync_channel(128);
        let bridge = Bridge::start(config, root.path(), "ses_tools", cancel, tx)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let connection = serde_json::from_slice(
            &std::fs::read(root.path().join("plugin/bridge.json"))
                .unwrap_or_else(|e| panic!("{e}")),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        Self {
            root,
            bridge,
            connection,
            _chunks: chunks,
            client: reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap_or_else(|e| panic!("{e}")),
        }
    }

    async fn request(&self, path: &str, body: Value) -> reqwest::Response {
        self.client
            .post(format!(
                "{}/{path}",
                self.connection["url"].as_str().unwrap_or_default()
            ))
            .header(
                "authorization",
                self.connection["token"].as_str().unwrap_or_default(),
            )
            .json(&body)
            .send()
            .await
            .unwrap_or_else(|e| panic!("{e}"))
    }

    async fn call(&self, name: &str, id: &str, input: Value) -> Value {
        let response = self
            .request(
                "call",
                json!({"sessionID":"ses_tools", "callID":id, "tool":name, "input":input}),
            )
            .await;
        assert!(response.status().is_success());
        response.json().await.unwrap_or_else(|e| panic!("{e}"))
    }
}

#[tokio::test]
async fn native_bridge_reuses_all_six_shared_tools_and_their_schemas() {
    let root = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let plan_path = root.path().join("plan.md");
    let persisted = Arc::new(AtomicBool::new(false));
    let title = Arc::new(Mutex::new(None));
    let metadata = Arc::new(Mutex::new(None));
    let skipped = Arc::new(Mutex::new(None));
    let ask = Arc::new(ScriptedAskHandler::new(["selected".to_string()]));
    let config = runner(
        vec![
            sdk_tools::ask_user_tool(ask, None),
            sdk_tools::submit_plan_tool(plan_path.clone(), Arc::clone(&persisted)),
            sdk_tools::update_plan_tool(plan_path.clone(), Arc::clone(&persisted)),
            sdk_tools::generate_title_tool(Arc::clone(&title)),
            sdk_tools::submit_pr_metadata_tool(Arc::clone(&metadata)),
            sdk_tools::skip_step_tool(Arc::clone(&skipped)),
        ],
        PermissionMode::Full,
    );
    let mut fixture = Fixture::new(&config, CancellationToken::new()).await;
    for tool in &config.tools {
        let published = fixture.connection["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools array"))
            .iter()
            .find(|value| value["name"] == tool.name)
            .unwrap_or_else(|| panic!("missing tool"));
        assert_eq!(published["parameters"], tool.parameters);
    }
    let answer = fixture
        .call("ask_user", "question", json!({"question":"Which?"}))
        .await;
    assert!(answer.get("content").is_some());
    fixture
        .call("submit_plan", "submit", json!({"content":"old plan"}))
        .await;
    fixture
        .call("update_plan", "update", json!({"old":"old", "new":"new"}))
        .await;
    fixture
        .call("generate_title", "title", json!({"title":"native title"}))
        .await;
    fixture
        .call(
            "submit_pr_metadata",
            "pr",
            json!({"title":"native PR", "body":"description"}),
        )
        .await;
    fixture
        .call(
            "skip_step",
            "skip",
            json!({"reason":"explicitly unchanged"}),
        )
        .await;
    assert_eq!(
        std::fs::read_to_string(plan_path).unwrap_or_else(|e| panic!("{e}")),
        "new plan"
    );
    assert!(persisted.load(Ordering::SeqCst));
    assert_eq!(
        title.lock().unwrap_or_else(|e| panic!("{e}")).as_deref(),
        Some("native title")
    );
    assert_eq!(
        metadata
            .lock()
            .unwrap_or_else(|e| panic!("{e}"))
            .as_ref()
            .map(|m| m.title.as_str()),
        Some("native PR")
    );
    assert_eq!(
        skipped.lock().unwrap_or_else(|e| panic!("{e}")).as_deref(),
        Some("explicitly unchanged")
    );
    assert!(fixture.bridge.had_side_effects());
    fixture
        .bridge
        .stop()
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!fixture.root.path().join("plugin/bridge.json").exists());
}

#[tokio::test]
async fn duplicate_call_ids_are_cached_and_cannot_change_identity() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let tool = CruiseTool::new(
        "count",
        "counter",
        json!({"type":"object"}),
        Arc::new(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            Err("correctable tool failure".to_string())
        }),
    );
    let mut fixture = Fixture::new(
        &runner(vec![tool], PermissionMode::Full),
        CancellationToken::new(),
    )
    .await;
    let (first, second) = tokio::join!(
        fixture.call("count", "same", json!({})),
        fixture.call("count", "same", json!({}))
    );
    assert!(first.get("error").is_some());
    assert_eq!(first, second);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let changed = fixture.request("call", json!({"sessionID":"ses_tools","callID":"same","tool":"count","input":{"changed":true}})).await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    let foreign = fixture
        .request(
            "call",
            json!({"sessionID":"ses_foreign","callID":"other","tool":"count","input":{}}),
        )
        .await;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    let unknown = fixture
        .request(
            "call",
            json!({"sessionID":"ses_tools","callID":"other","tool":"unknown","input":{}}),
        )
        .await;
    assert_eq!(unknown.status(), StatusCode::FORBIDDEN);
    let unauthorized = fixture
        .client
        .post(format!(
            "{}/ready",
            fixture.connection["url"].as_str().unwrap_or_default()
        ))
        .json(&json!({"sessionID":"ses_tools"}))
        .send()
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(unauthorized.status(), StatusCode::FORBIDDEN);
    fixture
        .bridge
        .stop()
        .await
        .unwrap_or_else(|e| panic!("{e}"));
}

#[tokio::test]
async fn question_cancellation_rejects_http_but_shutdown_drains_the_worker() {
    for disconnect in [false, true] {
        let release = Arc::new(AtomicBool::new(false));
        let _release_on_failure = ReleaseOnDrop(release.clone());
        let gate = Arc::clone(&release);
        let started = Arc::new(AtomicBool::new(false));
        let entered = Arc::clone(&started);
        let finished = Arc::new(AtomicBool::new(false));
        let mutation = finished.clone();
        let tool = CruiseTool::new(
            "ask_user",
            "wait",
            json!({"type":"object"}),
            Arc::new(move |_| {
                entered.store(true, Ordering::Release);
                while !gate.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                mutation.store(true, Ordering::Release);
                Ok("answer".to_string())
            }),
        );
        let cancel = CancellationToken::new();
        let mut fixture =
            Fixture::new(&runner(vec![tool], PermissionMode::Full), cancel.clone()).await;
        let url = format!(
            "{}/call",
            fixture.connection["url"].as_str().unwrap_or_default()
        );
        let token = fixture.connection["token"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let client = fixture.client.clone();
        let request = tokio::spawn(async move {
            client
            .post(url)
            .header("authorization", token)
            .json(
                &json!({"sessionID":"ses_tools","callID":"question","tool":"ask_user","input":{}}),
            )
            .send()
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !started.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|e| panic!("{e}"));
        if disconnect {
            request.abort();
            let _ = request.await;
        } else {
            cancel.cancel();
            let response = tokio::time::timeout(Duration::from_secs(2), request)
                .await
                .unwrap_or_else(|e| panic!("{e}"))
                .unwrap_or_else(|e| panic!("{e}"))
                .unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(response.status(), StatusCode::CONFLICT);
        }
        let stop = fixture.bridge.stop();
        tokio::pin!(stop);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), &mut stop)
                .await
                .is_err(),
            "HTTP completion must not count as handler completion"
        );
        assert!(!finished.load(Ordering::Acquire));
        assert!(fixture.root.path().join("plugin/bridge.json").exists());
        release.store(true, Ordering::Release);
        tokio::time::timeout(Duration::from_secs(2), &mut stop)
            .await
            .unwrap_or_else(|e| panic!("worker drain: {e}"))
            .unwrap_or_else(|e| panic!("bridge stop: {e}"));
        assert!(finished.load(Ordering::Acquire));
        assert!(!fixture.root.path().join("plugin/bridge.json").exists());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admission_racing_shutdown_cannot_lose_a_registered_worker() {
    let release = Arc::new(AtomicBool::new(false));
    let _release_on_failure = ReleaseOnDrop(release.clone());
    let entered = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let gate = release.clone();
    let starts = entered.clone();
    let ends = finished.clone();
    let tool = CruiseTool::new(
        "gate",
        "gate",
        json!({}),
        Arc::new(move |_| {
            starts.fetch_add(1, Ordering::SeqCst);
            while !gate.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(2));
            }
            ends.fetch_add(1, Ordering::SeqCst);
            Ok("finished".into())
        }),
    );
    let mut fixture = Fixture::new(
        &runner(vec![tool], PermissionMode::Full),
        CancellationToken::new(),
    )
    .await;
    let mut requests = vec![];
    for index in 0..32 {
        let state = fixture.bridge.state.clone();
        requests.push(tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            headers.insert(
                "authorization",
                state.token.parse().unwrap_or_else(|e| panic!("{e}")),
            );
            callback(
                State(state),
                headers,
                Json(Call {
                    session: "ses_tools".into(),
                    id: format!("race_{index}"),
                    tool: "gate".into(),
                    input: json!({}),
                }),
            )
            .await
        }));
    }
    tokio::time::timeout(Duration::from_secs(2), async {
        while entered.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|e| panic!("worker entry: {e}"));
    let state = fixture.bridge.state.clone();
    let stop = fixture.bridge.stop();
    tokio::pin!(stop);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut stop)
            .await
            .is_err()
    );
    assert!(
        state
            .admission
            .lock()
            .unwrap_or_else(|e| panic!("{e}"))
            .closed
    );
    release.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(2), &mut stop)
        .await
        .unwrap_or_else(|e| panic!("drain: {e}"))
        .unwrap_or_else(|e| panic!("stop: {e}"));
    for request in requests {
        let _ = request.await;
    }
    assert_eq!(
        entered.load(Ordering::SeqCst),
        finished.load(Ordering::SeqCst)
    );
    assert!(
        state
            .admission
            .lock()
            .unwrap_or_else(|e| panic!("{e}"))
            .workers
            .is_empty()
    );
}

#[test]
fn local_mcp_overlay_preserves_command_arguments_environment_and_diagnostic() {
    for args in [
        vec![],
        vec![
            "--flag=a,b".to_owned(),
            "space and 'quotes' \u{65e5}\u{672c}\u{8a9e}".to_owned(),
        ],
    ] {
        let mut config = runner(vec![], PermissionMode::Full);
        config.env.insert(
            "OPENCODE_CONFIG_CONTENT".into(),
            "{mcp:{local:{enabled:false,command:['overridden']}}}".into(),
        );
        let server = crate::config::McpServerConfig {
            command: Some("/path with spaces/executable:2026".into()),
            args: args.clone(),
            env: indexmap::IndexMap::from([("PUNCTUATION".into(), "a; b, 'c'".into())]),
            ..Default::default()
        };
        config.mcp_servers.insert("local".into(), server);
        let overlay: Value = serde_json::from_str(
            &configuration_overlay(&config, Path::new("/plugin"))
                .unwrap_or_else(|error| panic!("overlay: {error}")),
        )
        .unwrap_or_else(|error| panic!("JSON: {error}"));
        let mut expected = vec!["/path with spaces/executable:2026".to_owned()];
        expected.extend(args);
        assert_eq!(
            overlay["mcp"]["local"],
            json!({
                "type":"local", "command":expected, "environment":{"PUNCTUATION":"a; b, 'c'"}, "enabled":true
            })
        );
        config
            .mcp_servers
            .get_mut("local")
            .unwrap_or_else(|| panic!("server"))
            .command = None;
        assert_eq!(
            configuration_overlay(&config, Path::new("/plugin"))
                .err()
                .as_deref(),
            Some("OpenCode MCP local command missing")
        );
    }
}

#[test]
fn restricted_permissions_deny_execution_bypasses_and_overlay_preserves_jsonc() {
    for permission in [PermissionMode::ReadOnly, PermissionMode::Edit] {
        let mut config = runner(vec![], permission);
        config.env.insert("OPENCODE_CONFIG_CONTENT".to_string(), "{ /* comment */ plugins: ['existing'], mcp: {existing: {enabled:false}}, theme: 'dark', }".to_string());
        let allowed = allowed_tools(&config);
        for tool in [
            "bash",
            "shell",
            "task",
            "codemode",
            "unknown",
            "mcp__untrusted",
        ] {
            assert!(!allowed.contains(tool));
        }
        assert_eq!(
            allowed.contains("write"),
            permission == PermissionMode::Edit
        );
        let rules = permissions(&config);
        assert_eq!(rules[0]["effect"], "deny");
        let overlay: Value = serde_json::from_str(
            &configuration_overlay(&config, Path::new("/private/plugin"))
                .unwrap_or_else(|e| panic!("{e}")),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(overlay["plugins"][0], "existing");
        assert_eq!(overlay["mcp"]["existing"]["enabled"], false);
        assert_eq!(overlay["theme"], "dark");
        config.env.insert(
            "OPENCODE_CONFIG_CONTENT".to_string(),
            "{broken secret".to_string(),
        );
        let error = configuration_overlay(&config, Path::new("/private/plugin"))
            .err()
            .unwrap_or_else(|| panic!("invalid config accepted"));
        assert!(!error.contains("secret"));
    }
}

fn explicit_tool_policy() -> crate::tool_policy::ToolPolicy {
    use crate::tool_policy::{RuleSource, ShellRule, ToolPolicy};
    ToolPolicy {
        deny_web: true,
        deny_subagent: true,
        shell_rules: vec![
            ShellRule {
                prefix: "git commit".into(),
                source: RuleSource::AllowCommit,
            },
            ShellRule {
                prefix: "git -c core.hooksPath=".into(),
                source: RuleSource::AllowCommit,
            },
        ],
        ..Default::default()
    }
}

#[test]
fn tool_policy_permissions_append_native_denies_after_full_allow() {
    let mut config = runner(vec![], PermissionMode::Full);
    config.tool_policy = explicit_tool_policy();
    let rules = permissions(&config);
    assert_eq!(
        rules[0],
        json!({"action":"*", "resource":"*", "effect":"allow"})
    );
    let rules = rules
        .as_array()
        .unwrap_or_else(|| panic!("permission rules array"));
    for (action, resource) in [
        ("webfetch", "*"),
        ("websearch", "*"),
        ("task", "*"),
        ("bash", "git commit *"),
        ("bash", "git -c core.hooksPath=*"),
    ] {
        assert!(rules.iter().skip(1).any(|rule| *rule == json!({"action":action, "resource":resource, "effect":"deny"})), "missing deny {action}: {resource}");
    }
}

#[tokio::test]
async fn native_v2_tool_names_enforce_the_same_neutral_policy() {
    let mut config = runner(vec![], PermissionMode::Full);
    config.tool_policy = explicit_tool_policy();
    let rules = permissions(&config);
    let rules = rules
        .as_array()
        .unwrap_or_else(|| panic!("permission rules array"));
    for (action, resource) in [("subagent", "*"), ("shell", "git commit *")] {
        assert!(rules.iter().any(|rule| {
            *rule == json!({"action": action, "resource": resource, "effect": "deny"})
        }));
    }
    let mut fixture = Fixture::new(&config, CancellationToken::new()).await;
    for (tool, input) in [
        ("subagent", json!({"prompt": "delegate"})),
        ("shell", json!({"command": "git -C . commit -m blocked"})),
    ] {
        let response = fixture
            .request(
                "effect",
                json!({
                    "sessionID": "ses_tools", "callID": format!("native_{tool}"),
                    "tool": tool, "input": input,
                }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body: Value = response
            .json()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(body.get("error").is_some());
        assert!(!fixture.bridge.had_side_effects());
    }
    fixture
        .bridge
        .stop()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

#[test]
fn restricted_tool_policy_removes_web_without_removing_read_or_edit_tools() {
    for permission in [PermissionMode::ReadOnly, PermissionMode::Edit] {
        let mut config = runner(vec![], permission);
        config.tool_policy = crate::tool_policy::ToolPolicy {
            deny_web: true,
            deny_subagent: true,
            ..Default::default()
        };
        let allowed = allowed_tools(&config);
        for name in ["webfetch", "websearch", "task", "bash"] {
            assert!(!allowed.contains(name), "{permission:?}: allowed {name}");
        }
        for name in ["read", "glob", "grep", "list"] {
            assert!(allowed.contains(name), "{permission:?}: missing {name}");
        }
        assert_eq!(allowed.contains("edit"), permission == PermissionMode::Edit);
        let rules = permissions(&config);
        let rules = rules
            .as_array()
            .unwrap_or_else(|| panic!("permission rules array"));
        assert!(
            !rules
                .iter()
                .any(|rule| rule["action"] == "bash" && rule["resource"] != "*")
        );
    }
}

#[tokio::test]
async fn tool_policy_effect_rejects_denied_tools_and_structured_shell_commands() {
    let mut config = runner(vec![], PermissionMode::Full);
    config.tool_policy = explicit_tool_policy();
    let mut fixture = Fixture::new(&config, CancellationToken::new()).await;
    for (index, (tool, input, denied)) in [
        ("webfetch", json!({"url":"https://example.com"}), true),
        ("websearch", json!({"query":"example"}), true),
        ("task", json!({"prompt":"delegate"}), true),
        ("bash", json!({"command":"git commit -m blocked"}), true),
        (
            "bash",
            json!({"command":"git -C . commit -m blocked"}),
            true,
        ),
        (
            "bash",
            json!({"command":"sh -c 'git commit -m blocked'"}),
            true,
        ),
        (
            "bash",
            json!({"command":"git -c core.hooksPath=/dev/null status"}),
            true,
        ),
        ("bash", json!({"command":"git status"}), false),
        ("bash", json!({"command":"echo 'git commit'"}), false),
        ("bash", json!({"command":"git commit-tree"}), false),
        ("read", json!({"filePath":"README.md"}), false),
    ]
    .into_iter()
    .enumerate()
    {
        let response = fixture
            .request(
                "effect",
                json!({
                    "sessionID":"ses_tools", "callID":format!("policy_{index}"),
                    "tool":tool, "input":input
                }),
            )
            .await;
        assert_eq!(
            response.status(),
            if denied {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::OK
            },
            "{tool}: {input}"
        );
        let body: Value = response
            .json()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        if denied {
            assert!(body.get("error").is_some(), "structured rejection: {body}");
            assert!(
                !fixture.bridge.had_side_effects(),
                "denial must precede the retry barrier"
            );
        } else {
            assert_eq!(body["accepted"], true);
        }
    }
    fixture
        .bridge
        .stop()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn tool_policy_effect_env_search_path_denies_before_admission() {
    use crate::tool_policy::{ToolPolicy, ToolRestriction};

    for explicit in [false, true] {
        let mut config = runner(vec![], PermissionMode::Full);
        let restrictions = if explicit {
            vec![ToolRestriction::ShellPrefix("git push".into())]
        } else {
            vec![]
        };
        config.tool_policy =
            ToolPolicy::for_prompt_step(&restrictions, &[], explicit, PermissionMode::Full);
        let mut fixture = Fixture::new(&config, CancellationToken::new()).await;
        for (call, command, expected) in [
            ("push", "env -P /usr/bin git push", StatusCode::FORBIDDEN),
            ("status", "env -P /usr/bin git status", StatusCode::OK),
        ] {
            let response = fixture.request("effect", json!({
                "sessionID":"ses_tools", "callID":call, "tool":"bash", "input":{"command":command}
            })).await;
            assert_eq!(response.status(), expected, "{command}");
            if expected == StatusCode::FORBIDDEN {
                assert!(
                    !fixture.bridge.had_side_effects(),
                    "denial must precede admission"
                );
            }
        }
        fixture
            .bridge
            .stop()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
    }
}

#[tokio::test]
async fn tool_policy_effect_git_config_operands_deny_before_admission() {
    use crate::tool_policy::{ToolPolicy, ToolRestriction};

    for explicit in [false, true] {
        let mut config = runner(vec![], PermissionMode::Full);
        let restrictions = if explicit {
            vec![ToolRestriction::ShellPrefix(
                "git -c core.hooksPath=".into(),
            )]
        } else {
            vec![]
        };
        config.tool_policy =
            ToolPolicy::for_prompt_step(&restrictions, &[], explicit, PermissionMode::Full);
        for tool in ["bash", "shell"] {
            let mut fixture = Fixture::new(&config, CancellationToken::new()).await;
            for (index, command) in [
                "git -C . -c core.hooksPath=/dev/null status",
                "git -c color.ui=false -c core.hooksPath=/dev/null status",
            ]
            .into_iter()
            .enumerate()
            {
                let response = fixture
                    .request(
                        "effect",
                        json!({
                            "sessionID":"ses_tools", "callID":format!("hooks_{index}"),
                            "tool":tool, "input":{"command":command}
                        }),
                    )
                    .await;
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{command}");
                let body: Value = response
                    .json()
                    .await
                    .unwrap_or_else(|error| panic!("{error}"));
                assert!(body.get("error").is_some(), "structured rejection: {body}");
                assert!(
                    !fixture.bridge.had_side_effects(),
                    "denial must precede admission"
                );
            }
            for (index, command) in [
                "git -C . -c color.ui=false status",
                "git -c color.ui=false -c advice.statusHints=false status",
            ]
            .into_iter()
            .enumerate()
            {
                let response = fixture
                    .request(
                        "effect",
                        json!({
                            "sessionID":"ses_tools", "callID":format!("ordinary_{index}"),
                            "tool":tool, "input":{"command":command}
                        }),
                    )
                    .await;
                assert_eq!(response.status(), StatusCode::OK, "{command}");
                let body: Value = response
                    .json()
                    .await
                    .unwrap_or_else(|error| panic!("{error}"));
                assert_eq!(body["accepted"], true);
            }
            fixture
                .bridge
                .stop()
                .await
                .unwrap_or_else(|error| panic!("{error}"));
        }
    }
}
