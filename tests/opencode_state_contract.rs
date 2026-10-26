//! Observable `OpenCode` state and planning contracts. No CLI or model is started.
use std::collections::HashMap;
use std::fs;

use cruise::config::{PermissionMode, WorkflowConfig};
use cruise::error::CruiseError;
use cruise::executor::{Executor, PromptRun};
use cruise::planning::{self, plan_conversation_key};
use cruise::session::{SessionManager, SessionState};
use cruise::session_config::SessionConfigRef;
use serde_json::{Value, json};
use tempfile::TempDir;

fn config(sdk: &str) -> WorkflowConfig {
    WorkflowConfig::from_yaml(&format!(
        "sdk: {sdk}\nmodel: provider/model:tag#variant\nplan_model: provider/planner#deep\ninteractive_planning: true\npermission: full\nenv:\n  OPENCODE_CONFIG_CONTENT: '{{\"theme\":\"dark\"}}'\nsteps:\n  implement:\n    prompt: '{{input}}'\n"
    ))
    .unwrap_or_else(|error| panic!("deserialize workflow: {error}"))
}

fn fixture(manager: &SessionManager, temp: &TempDir, id: &str) -> Value {
    let state = SessionState::new(
        id.to_string(),
        temp.path().join("repo"),
        SessionConfigRef::inline_snapshot(),
        "implement contract".to_string(),
    );
    manager
        .create(&state)
        .unwrap_or_else(|error| panic!("create fixture session: {error}"));
    serde_json::to_value(state).unwrap_or_else(|error| panic!("serialize fixture: {error}"))
}

fn persist_fixture(manager: &SessionManager, id: &str, value: &Value) {
    fs::write(
        manager.state_path(id),
        serde_json::to_vec(value)
            .unwrap_or_else(|error| panic!("contract fixture failed: {error}")),
    )
    .unwrap_or_else(|error| panic!("contract fixture failed: {error}"));
}

fn saved(manager: &SessionManager, id: &str) -> Value {
    serde_json::from_slice(
        &fs::read(manager.state_path(id))
            .unwrap_or_else(|error| panic!("contract fixture failed: {error}")),
    )
    .unwrap_or_else(|error| panic!("contract fixture failed: {error}"))
}

#[test]
fn opencode_conversation_owner_id_and_key_survive_manager_load_save() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("contract fixture failed: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let id = "20261023000101";
    let mut value = fixture(&manager, &temp, id);
    value["plan_conversation_backend"] = json!("opencode");
    value["plan_conversation_id"] = json!("ses_contract");
    value["plan_conversation_key"] = json!("opencode-config-identity");
    value
        .as_object_mut()
        .unwrap_or_else(|| panic!("state fixture must be an object"))
        .remove("plan_conversation_home");
    persist_fixture(&manager, id, &value);

    let mut state = manager
        .load(id)
        .unwrap_or_else(|error| panic!("load OpenCode checkpoint: {error}"));
    state.pending_ask_question = Some("Which implementation?".to_string());
    state.awaiting_input = true;
    manager
        .save(&state)
        .unwrap_or_else(|error| panic!("save question checkpoint: {error}"));
    let reloaded = manager
        .load(id)
        .unwrap_or_else(|error| panic!("reload question checkpoint: {error}"));
    manager
        .save(&reloaded)
        .unwrap_or_else(|error| panic!("save checkpoint again: {error}"));
    let actual = saved(&manager, id);
    assert_eq!(actual["plan_conversation_backend"], "opencode");
    assert_eq!(actual["plan_conversation_id"], "ses_contract");
    assert_eq!(actual["plan_conversation_key"], "opencode-config-identity");
    assert!(
        actual
            .get("plan_conversation_home")
            .is_none_or(Value::is_null)
    );
    assert_eq!(actual["pending_ask_question"], "Which implementation?");
    assert_eq!(actual["awaiting_input"], true);
}

#[test]
fn legacy_conversation_without_owner_remains_readable_without_inventing_owner() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("contract fixture failed: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    for (id, home) in [
        ("20261023000102", None),
        ("20261023000103", Some(temp.path().join("old-jcode-home"))),
    ] {
        let mut value = fixture(&manager, &temp, id);
        value
            .as_object_mut()
            .unwrap_or_else(|| panic!("state fixture must be an object"))
            .remove("plan_conversation_backend");
        value["plan_conversation_id"] = json!("legacy-conversation");
        value["plan_conversation_key"] = json!("legacy-key");
        if let Some(home) = &home {
            value["plan_conversation_home"] = json!(home);
        }
        persist_fixture(&manager, id, &value);
        let state = manager
            .load(id)
            .unwrap_or_else(|error| panic!("load ownerless legacy state: {error}"));
        manager
            .save(&state)
            .unwrap_or_else(|error| panic!("save legacy state: {error}"));
        let actual = saved(&manager, id);
        assert!(
            actual
                .get("plan_conversation_backend")
                .is_none_or(Value::is_null)
        );
        assert_eq!(actual["plan_conversation_id"], "legacy-conversation");
        assert_eq!(actual["plan_conversation_key"], "legacy-key");
        if let Some(home) = home {
            assert_eq!(actual["plan_conversation_home"], json!(home));
        }
    }
}

#[test]
fn session_snapshot_retains_opencode_configuration_across_progress_save() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("contract fixture failed: {error}"));
    let manager = SessionManager::new(temp.path().join("data"));
    let config = config("opencode");
    let state = SessionState::new(
        "20261023000104".to_string(),
        temp.path().join("repo"),
        SessionConfigRef::inline_snapshot(),
        "snapshot".to_string(),
    );
    manager
        .create_with_config(&state, &config)
        .unwrap_or_else(|error| panic!("snapshot config: {error}"));
    let snapshot_path = manager.sessions_dir().join(&state.id).join("config.yaml");
    let before =
        fs::read(&snapshot_path).unwrap_or_else(|error| panic!("contract fixture failed: {error}"));
    let mut loaded = manager
        .load(&state.id)
        .unwrap_or_else(|error| panic!("contract fixture failed: {error}"));
    loaded.title = Some("progress".to_string());
    manager
        .save(&loaded)
        .unwrap_or_else(|error| panic!("contract fixture failed: {error}"));
    assert_eq!(
        fs::read(snapshot_path).unwrap_or_else(|error| panic!("contract fixture failed: {error}")),
        before
    );
    let restored = manager
        .load_config(
            &manager
                .load(&state.id)
                .unwrap_or_else(|error| panic!("contract fixture failed: {error}")),
        )
        .unwrap_or_else(|error| panic!("load snapshot: {error}"));
    assert_eq!(
        serde_json::to_value(restored)
            .unwrap_or_else(|error| panic!("contract fixture failed: {error}")),
        serde_json::to_value(config)
            .unwrap_or_else(|error| panic!("contract fixture failed: {error}"))
    );
}

#[test]
fn opencode_selects_sdk_planning_templates_and_preserves_model_references() {
    let config = config("opencode");
    let executor = Executor::new(config.sdk.as_deref(), &config.command);
    assert!(executor.is_sdk(), "OpenCode must select an SDK backend");
    assert!(planning::sdk_plan_tools_enabled(&config));
    assert_eq!(
        planning::plan_template(&config),
        planning::PLAN_PROMPT_TEMPLATE_SDK
    );
    assert_eq!(
        planning::fix_plan_template(&config),
        planning::FIX_PLAN_PROMPT_TEMPLATE_SDK
    );
    assert_eq!(
        planning::ask_plan_template(&config),
        planning::ASK_PLAN_PROMPT_TEMPLATE_SDK
    );
    assert_eq!(
        planning::initial_plan_template(&config, true),
        planning::PLAN_GRILL_PROMPT_TEMPLATE_SDK
    );
    assert_eq!(
        executor.step_model(
            Some("provider/step:tag#fast"),
            Some(&cruise::config::ModelSpec::Single(
                "provider/default".to_owned()
            ))
        ),
        Some(cruise::config::ModelSpec::Single(
            "provider/step:tag#fast".to_string()
        ))
    );
    assert_eq!(
        executor.step_model(
            None,
            Some(&cruise::config::ModelSpec::Single(
                "provider/model:tag#variant".to_owned()
            ))
        ),
        Some(cruise::config::ModelSpec::Single(
            "provider/model:tag#variant".to_string()
        ))
    );
    let mut noninteractive = config.clone();
    noninteractive.interactive_planning = false;
    assert!(!planning::sdk_plan_tools_enabled(&noninteractive));
    assert_eq!(
        planning::plan_template(&noninteractive),
        planning::PLAN_PROMPT_TEMPLATE
    );
}

#[test]
fn conversation_key_is_stable_and_separates_opencode_from_other_backends() {
    let opencode = config("opencode");
    let identity = "owner/repo::.cruise/workflow.yaml";
    let key = plan_conversation_key(&opencode, identity);
    assert_eq!(key, plan_conversation_key(&opencode, identity));
    for backend in ["jcode", "claude"] {
        assert_ne!(key, plan_conversation_key(&config(backend), identity));
    }
    assert_ne!(key, plan_conversation_key(&opencode, "different-reference"));
}

#[tokio::test]
async fn direct_opencode_executor_rejects_computer_use_without_starting_backend() {
    const CHILD: &str = "CRUISE_STATE_COMPUTER_USE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        // Run the public executor in a child so environment isolation cannot race
        // with the other contracts. An empty PATH prevents starting a real CLI,
        // and fresh HOME/XDG roots contain no user configuration or credentials.
        let temp =
            TempDir::new().unwrap_or_else(|error| panic!("create isolated executor root: {error}"));
        let bin = temp.path().join("empty-bin");
        fs::create_dir(&bin)
            .unwrap_or_else(|error| panic!("create empty executable directory: {error}"));
        let executable = std::env::current_exe()
            .unwrap_or_else(|error| panic!("locate test executable: {error}"));
        let output = std::process::Command::new(executable)
            .args([
                "--exact",
                "direct_opencode_executor_rejects_computer_use_without_starting_backend",
                "--nocapture",
            ])
            .env_clear()
            .env(CHILD, "1")
            .env("PATH", bin)
            .env("HOME", temp.path())
            .env("USERPROFILE", temp.path())
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("XDG_DATA_HOME", temp.path().join("data"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env("XDG_CACHE_HOME", temp.path().join("cache"))
            .current_dir(temp.path())
            .output()
            .unwrap_or_else(|error| panic!("run isolated executor contract: {error}"));
        assert!(
            output.status.success(),
            "isolated executor contract failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let env = HashMap::new();
    let mcp_servers = cruise::config::McpServers::default();
    let result = Executor::new(Some("opencode"), &[])
        .run(PromptRun {
            prompt: "must not execute",
            model: None,
            subagent_model: None,
            max_retries: 0,
            env: &env,
            mcp_servers: &mcp_servers,
            on_notice: None,
            cancel_token: None,
            working_dir: None,
            stream: None,
            tools: vec![],
            on_session_id: None,
            resume: None,
            computer_use: true,
            permission: PermissionMode::Full,
        })
        .await;
    match result {
        Err(CruiseError::InvalidStepConfig(message)) => {
            let message = message.to_lowercase();
            assert!(
                message.contains("computer"),
                "must diagnose computer_use: {message}"
            );
            assert!(
                message.contains("jcode"),
                "must identify the supported backend: {message}"
            );
        }
        other => panic!("expected computer_use configuration rejection, got {other:?}"),
    }
}
