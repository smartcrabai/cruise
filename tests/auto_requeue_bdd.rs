#![expect(clippy::unwrap_used, reason = "isolated acceptance fixtures")]

use cucumber::{World as _, given, then, when};
use std::process::{Command, Output, Stdio};

#[derive(Debug, Default, cucumber::World)]
struct World {
    root: Option<tempfile::TempDir>,
    output: Option<Output>,
    state_path: Option<std::path::PathBuf>,
}

impl World {
    fn root(&self) -> &std::path::Path {
        self.root.as_ref().unwrap().path()
    }
    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if (name.starts_with("CRUISE_") && name != "CRUISE_GUARD_COMMON_DIR")
                || name.starts_with("JCODE_")
                || name.starts_with("OTEL_")
            {
                cmd.env_remove(name.as_ref());
            }
        }
        cmd.current_dir(self.root())
            .env("HOME", self.root().join("home"))
            .env("USERPROFILE", self.root().join("home"))
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("XDG_DATA_HOME", self.root().join("data"))
            .env("XDG_STATE_HOME", self.root().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
            .env_remove("HERDR_ENV")
            .stdin(Stdio::null())
            .args(args)
            .output()
            .unwrap()
    }
    fn text(&self) -> String {
        let out = self.output.as_ref().unwrap();
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }
}

#[given("an isolated Cruise installation")]
fn isolated(w: &mut World) {
    w.root = Some(tempfile::tempdir().unwrap());
}

#[when("I inspect the retry budget")]
fn inspect(w: &mut World) {
    w.output = Some(w.run(&["config"]));
}

#[when(expr = "I set the retry budget to {int}")]
fn set(w: &mut World, n: u32) {
    let out = w.run(&["config", "--set-auto-requeue-max-attempts", &n.to_string()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[then(expr = "the retry budget is {int}")]
fn budget(w: &mut World, n: u32) {
    assert!(w.output.as_ref().unwrap().status.success(), "{}", w.text());
    assert!(
        w.text()
            .contains(&format!("run_all_auto_requeue_max_attempts = {n}")),
        "{}",
        w.text()
    );
}

#[when("I run one session with a retry override")]
fn single(w: &mut World) {
    w.output = Some(w.run(&["run", "example", "--auto-requeue-max-attempts", "2"]));
}

#[then("the command is rejected because it requires all")]
fn rejected(w: &mut World) {
    assert!(!w.output.as_ref().unwrap().status.success(), "{}", w.text());
    assert!(w.text().contains("--all"), "{}", w.text());
}

#[given("a negative persisted retry budget")]
fn invalid(w: &mut World) {
    let dir = w.root().join("config/cruise");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.json"),
        r#"{"runAllAutoRequeueMaxAttempts":-1}"#,
    )
    .unwrap();
}

#[when("I start a batch without an override")]
fn batch(w: &mut World) {
    w.output = Some(w.run(&["run", "--all"]));
}

#[then("the command fails")]
fn fails(w: &mut World) {
    assert!(!w.output.as_ref().unwrap().status.success(), "{}", w.text());
}

#[given("a previously failed session with one recorded retry")]
fn failed_session(w: &mut World) {
    let config = w.root().join("workflow.yaml");
    std::fs::write(
        &config,
        "command: [echo]\nsteps:\n  work:\n    command: echo unexpected\n",
    )
    .unwrap();
    let out = w.run(&[
        "plan",
        "--skip-planning",
        "--config",
        config.to_str().unwrap(),
        "batch startup test",
    ]);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let out = w.run(&["list", "--json"]);
    assert!(out.status.success());
    let sessions: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(sessions.len(), 1);
    let id = sessions[0]["id"].as_str().unwrap();
    let path = w
        .root()
        .join("data/cruise/sessions")
        .join(id)
        .join("state.json");
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    state["phase"] = serde_json::json!({"Failed": "old failure"});
    state["auto_requeue_count"] = 1.into();
    std::fs::write(&path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();
    w.state_path = Some(path);
}

#[when("I start a batch with a retry budget of 2")]
fn batch_override(w: &mut World) {
    w.output = Some(w.run(&["run", "--all", "--auto-requeue-max-attempts", "2"]));
}

#[then("the session remains failed with one recorded retry")]
fn unchanged(w: &mut World) {
    assert!(w.output.as_ref().unwrap().status.success(), "{}", w.text());
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(w.state_path.as_ref().unwrap()).unwrap()).unwrap();
    assert_eq!(
        state["phase"],
        serde_json::json!({"Failed": "old failure"}),
        "{}",
        w.text()
    );
    assert_eq!(state["auto_requeue_count"], 1, "{}", w.text());
}

#[given("an independent repository with a planned no-changes failure")]
fn planned_failure(w: &mut World) {
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    ] {
        let out = Command::new("git")
            .current_dir(w.root())
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let config = w.root().join("workflow.yaml");
    std::fs::write(&config, "command: [cat]\ncreate_pr: false\nsteps:\n  work:\n    command: 'true'\n    if:\n      no-file-changes: failed\n").unwrap();
    let out = w.run(&[
        "plan",
        "--skip-planning",
        "--config",
        config.to_str().unwrap(),
        "retry failing step",
    ]);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let out = w.run(&["list", "--json"]);
    assert!(out.status.success());
    let sessions: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(sessions.len(), 1);
    w.state_path = Some(
        w.root()
            .join("data/cruise/sessions")
            .join(sessions[0]["id"].as_str().unwrap())
            .join("state.json"),
    );
}

#[when(expr = "I run the batch with a retry budget of {int}")]
fn run_batch_budget(w: &mut World, limit: u32) {
    w.output = Some(w.run(&[
        "run",
        "--all",
        "--auto-requeue-max-attempts",
        &limit.to_string(),
    ]));
}

#[given("an independent repository with a planned authentication failure")]
fn planned_auth_failure(w: &mut World) {
    planned_failure(w);
    std::fs::write(w.root().join("workflow.yaml"), "command: [sh, -c, 'echo HTTP 401 authentication failed >&2; exit 1']\ncreate_pr: false\nsteps:\n  work:\n    prompt: 'perform work'\n").unwrap();
}

#[then("the failure reports authentication")]
fn auth_reported(w: &mut World) {
    let state = std::fs::read_to_string(w.state_path.as_ref().unwrap()).unwrap();
    assert!(state.to_lowercase().contains("authentication"), "{state}; {}", w.text());
}

#[then(expr = "the batch session is failed with {int} automatic retries")]
fn verify_budget(w: &mut World, limit: u32) {
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(w.state_path.as_ref().unwrap()).unwrap()).unwrap();
    assert!(
        state["phase"].get("Failed").is_some(),
        "state: {state}; {}",
        w.text()
    );
    assert_eq!(
        state["auto_requeue_count"].as_u64().unwrap_or(0),
        u64::from(limit),
        "state: {state}; {}",
        w.text()
    );
    assert_eq!(
        w.text().matches("Auto-requeued ").count(),
        limit as usize,
        "{}",
        w.text()
    );
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    World::cucumber()
        .max_concurrent_scenarios(1)
        .fail_on_skipped()
        .run_and_exit(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/auto_requeue.feature"
        ))
        .await;
}
