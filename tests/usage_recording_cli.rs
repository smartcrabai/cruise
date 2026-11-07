#![cfg(unix)]
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "isolated executable contract fixtures"
)]

//! Real executable contracts for local usage recording. The fake Claude CLI
//! speaks stream-json, so no provider credentials or network are needed.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use cruise::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
use serde_json::Value;
use tempfile::TempDir;

const ID: &str = "20260921070000_00000000000000000000000000000000";
const SECRET: &str = "PRIVATE_PROMPT_SENTINEL_9472";
const OUTPUT_SECRET: &str = "PRIVATE_OUTPUT_SENTINEL_8321";

struct Fixture {
    root: TempDir,
    repo: PathBuf,
}

impl Fixture {
    fn new(workflow: &str) -> Self {
        let root = TempDir::new().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        cruise::test_support::init_git_repo(&repo);
        let config = root.path().join("workflow.yaml");
        std::fs::write(&config, workflow).unwrap();
        let manager = SessionManager::new(root.path().join("data/cruise"));
        let mut state = SessionState::new(
            ID.to_string(),
            repo.clone(),
            cruise::session_config::SessionConfigRef::File { path: config },
            SECRET.to_string(),
        );
        state.phase = SessionPhase::Planned;
        state.workspace_mode = WorkspaceMode::CurrentBranch;
        state.target_branch = Some("main".to_string());
        state.has_dag = true;
        manager.create(&state).unwrap();
        std::fs::write(state.plan_path(&manager.sessions_dir()), SECRET).unwrap();
        let compiled =
            cruise::workflow::compile(cruise::config::WorkflowConfig::from_yaml(workflow).unwrap())
                .unwrap();
        let dag = cruise::dag::build_dag(&compiled, 0).unwrap();
        cruise::dag::save_dag(&dag, &manager.dag_path(ID)).unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        cruise::test_support::write_executable_script(
            &bin.join("claude"),
            concat!(
                "#!/bin/sh\nread -r user_frame\n",
                "printf '%s\\n' '",
                "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"fake-session\"}\n",
                "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"PRIVATE_OUTPUT_SENTINEL_8321\"}]},\"session_id\":\"fake-session\"}\n",
                "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"session_id\":\"fake-session\",\"result\":\"PRIVATE_OUTPUT_SENTINEL_8321\",\"usage\":{\"input_tokens\":999,\"output_tokens\":888},\"total_cost_usd\":9.0,\"modelUsage\":{\"actual-model\":{\"inputTokens\":12,\"outputTokens\":5,\"cacheReadInputTokens\":0,\"cacheCreationInputTokens\":3,\"costUSD\":0.25}}}\n'\n"
            ),
        );
        Self { root, repo }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
    fn usage_path(&self) -> PathBuf {
        self.path("state/cruise/usage.ndjson")
    }
    fn run(&self, recording: Option<&str>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (key, _) in std::env::vars_os() {
            let key_str = key.to_string_lossy();
            if key_str.starts_with("CRUISE_")
                || key_str.starts_with("CLAUDE")
                || key_str.starts_with("ANTHROPIC_")
                || key_str.starts_with("JCODE_")
            {
                cmd.env_remove(key);
            }
        }
        let mut paths = vec![self.path("bin")];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        cmd.current_dir(&self.repo)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("home"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_STATE_HOME", self.path("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("CRUISE_SKIP_AGENT_AUTH_CHECK", "1")
            .env("PRIVATE_ENV_SENTINEL", "PRIVATE_ENV_VALUE_31415")
            .env("GIT_CONFIG_COUNT", "0")
            .env("NO_COLOR", "1")
            .args(["run", ID])
            .stdin(Stdio::null());
        if let Some(value) = recording {
            cmd.env("CRUISE_USAGE_RECORDING", value);
        }
        cmd.output().unwrap()
    }
    fn successful_run(&self, recording: Option<&str>) {
        let output = self.run(recording);
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn record(&self) -> Value {
        let text = std::fs::read_to_string(self.usage_path())
            .expect("usage NDJSON must exist in XDG state");
        assert_eq!(
            text.lines().count(),
            1,
            "one attempt must produce exactly one record: {text}"
        );
        assert!(text.ends_with('\n'));
        for secret in [
            SECRET,
            OUTPUT_SECRET,
            "PRIVATE_ENV_VALUE_31415",
            self.repo.to_str().unwrap(),
        ] {
            assert!(!text.contains(secret), "private content leaked: {secret}");
        }
        serde_json::from_str(&text).unwrap()
    }
}

fn assert_allowed_keys(value: &Value, allowed: &[&str]) {
    for key in value.as_object().expect("object").keys() {
        assert!(
            allowed.contains(&key.as_str()),
            "unexpected private field {key}: {value}"
        );
    }
}

const CLAUDE_WORKFLOW: &str = "sdk: claude\nmodel: requested-model\nsteps:\n  answer:\n    prompt: PRIVATE_PROMPT_SENTINEL_9472\n";

#[test]
fn claude_sdk_run_appends_one_reported_record() {
    let f = Fixture::new(CLAUDE_WORKFLOW);
    f.successful_run(None);
    let record = f.record();
    assert_allowed_keys(
        &record,
        &[
            "v",
            "ts",
            "session_id",
            "step",
            "purpose",
            "backend",
            "model",
            "attempt",
            "outcome",
            "usage",
        ],
    );
    assert_eq!(record["v"], 1);
    assert_eq!(record["session_id"], ID);
    assert_eq!(record["step"], "answer");
    assert_eq!(record["purpose"], "workflow_step");
    assert_eq!(record["backend"], "claude");
    assert_eq!(record["model"], "requested-model");
    assert_eq!(record["attempt"], 1);
    assert_eq!(record["outcome"], "success");
    let usage = &record["usage"];
    assert_allowed_keys(
        usage,
        &[
            "status",
            "input_tokens",
            "output_tokens",
            "reasoning_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
            "cost_usd",
            "by_model",
        ],
    );
    assert_eq!(usage["status"], "reported");
    assert_eq!(usage["input_tokens"], 12);
    assert_eq!(usage["output_tokens"], 5);
    assert_eq!(usage["cache_read_input_tokens"], 0);
    assert_eq!(usage["cache_creation_input_tokens"], 3);
    assert_eq!(usage["cost_usd"], 0.25);
    assert!(usage.get("reasoning_tokens").is_none());
    let models = usage["by_model"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_allowed_keys(
        &models[0],
        &[
            "model",
            "input_tokens",
            "output_tokens",
            "reasoning_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
            "cost_usd",
        ],
    );
    assert_eq!(models[0]["model"], "actual-model");
    assert_eq!(models[0]["input_tokens"], 12);
    assert!(!f.repo.join("usage.ndjson").exists());
    assert!(!f.path("data/cruise/usage.ndjson").exists());
}

#[test]
fn usage_recording_disabled_by_env_writes_nothing() {
    for value in ["0", " false ", "FALSE"] {
        let f = Fixture::new(CLAUDE_WORKFLOW);
        f.successful_run(Some(value));
        assert!(
            !f.usage_path().exists(),
            "opt-out {value:?} created a usage file"
        );
    }
}

#[test]
fn command_backend_run_appends_unavailable_record() {
    let f = Fixture::new(
        "command: [sh, -c, 'cat']\nsteps:\n  answer:\n    prompt: PRIVATE_PROMPT_SENTINEL_9472\n",
    );
    f.successful_run(None);
    let record = f.record();
    assert_allowed_keys(
        &record,
        &[
            "v",
            "ts",
            "session_id",
            "step",
            "purpose",
            "backend",
            "model",
            "attempt",
            "outcome",
            "usage",
        ],
    );
    assert_eq!(record["session_id"], ID);
    assert_eq!(record["step"], "answer");
    assert_eq!(record["purpose"], "workflow_step");
    assert_eq!(record["backend"], "command");
    assert_eq!(record["attempt"], 1);
    assert_eq!(record["outcome"], "success");
    assert_eq!(
        record["usage"],
        serde_json::json!({"status": "unavailable", "reason": "command_backend"})
    );
}

#[test]
fn shell_command_step_does_not_create_usage_record() {
    let f = Fixture::new("steps:\n  answer:\n    command: 'echo hello'\n");
    f.successful_run(None);
    assert!(!f.usage_path().exists());
}
