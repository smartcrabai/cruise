#![cfg(unix)]

//! Entry-point contracts for model and fallback notices.
//!
//! These tests keep process homes, config lookup, and the session store isolated
//! so terminal output and the persisted `run.log` are asserted independently.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use cruise::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};
use tempfile::TempDir;

static PROCESS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct Fixture {
    _process_lock: MutexGuard<'static, ()>,
    root: TempDir,
    repo: PathBuf,
    manager: SessionManager,
}

impl Fixture {
    fn new() -> Self {
        let process_lock = PROCESS_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap_or_else(|error| panic!("repo mkdir: {error}"));
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(
            &repo,
            &[
                "-c",
                "user.name=Cruise test",
                "-c",
                "user.email=cruise-test@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        );
        let manager = SessionManager::new(root.path().join("data/cruise"));
        Self {
            _process_lock: process_lock,
            root,
            repo,
            manager,
        }
    }

    fn isolated_command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        command
            .current_dir(&self.repo)
            .env("HOME", self.root.path().join("home"))
            .env("USERPROFILE", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
            .env_remove("CRUISE_CONFIG")
            .env_remove("CRUISE_MODEL")
            .env_remove("CRUISE_PLAN_MODEL")
            .env_remove("CRUISE_SDK")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn write_repo_config(&self, yaml: &str) -> PathBuf {
        let path = self.repo.join("cruise.yaml");
        std::fs::write(&path, yaml).unwrap_or_else(|error| panic!("write config: {error}"));
        path
    }

    fn seed_prompt_session(&self, id: &str, yaml: &str, plan: &str) {
        let config_path = self.root.path().join(format!("{id}.yaml"));
        std::fs::write(&config_path, yaml)
            .unwrap_or_else(|error| panic!("write session config: {error}"));
        let mut state = SessionState::new(
            id.to_string(),
            self.repo.clone(),
            cruise::session_config::SessionConfigRef::File {
                path: config_path.clone(),
            },
            "seeded model logging session".to_string(),
        );
        state.phase = SessionPhase::Planned;
        state.workspace_mode = WorkspaceMode::CurrentBranch;
        state.target_branch = Some("main".to_string());
        state.has_dag = true;
        self.manager
            .create(&state)
            .unwrap_or_else(|error| panic!("create session: {error}"));
        std::fs::write(
            state.plan_path(&self.manager.sessions_dir()),
            format!("# Seeded plan\n\n{plan}\n"),
        )
        .unwrap_or_else(|error| panic!("write plan: {error}"));
        let workflow = cruise::config::WorkflowConfig::from_yaml(yaml)
            .unwrap_or_else(|error| panic!("parse workflow: {error}"));
        let compiled = cruise::workflow::compile(workflow)
            .unwrap_or_else(|error| panic!("compile workflow: {error}"));
        let dag = cruise::dag::build_dag(&compiled, 0)
            .unwrap_or_else(|error| panic!("build execution graph: {error}"));
        cruise::dag::save_dag(&dag, &self.manager.dag_path(id))
            .unwrap_or_else(|error| panic!("save execution graph: {error}"));
    }

    fn run_command(&self, args: &[&str]) -> Output {
        self.isolated_command()
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("cruise {args:?}: {error}"))
    }
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_COUNT", "0")
        .output()
        .unwrap_or_else(|error| panic!("git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn terminal(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn wait_for_log(path: &Path, fragment: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(log) = std::fs::read_to_string(path)
            && log.contains(fragment)
        {
            return log;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {fragment:?} in {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn exec_reports_the_selected_model_on_stderr_and_keeps_stdout_machine_clean() {
    let fixture = Fixture::new();
    let config = fixture.write_repo_config(
        "command: [sh, -c, 'cat']\nmodel: 'provider/model:free:xhigh'\nsteps:\n  answer:\n    prompt: exec response\n",
    );

    let output = fixture.run_command(&[
        "exec",
        "--config",
        config
            .to_str()
            .unwrap_or_else(|| panic!("config is not UTF-8")),
        "exec model logging",
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "exec failed: {}",
        terminal(&output)
    );
    assert!(
        stderr.contains("Model: provider/model:free:xhigh"),
        "selected model should be shown on stderr: {stderr}"
    );
    assert!(
        !stdout.contains("Model:"),
        "stdout must not receive human-readable model notices: {stdout}"
    );
    assert!(
        fixture
            .manager
            .list()
            .unwrap_or_else(|error| panic!("list sessions: {error}"))
            .is_empty(),
        "completed exec sessions retain their existing deletion behavior"
    );
}

#[test]
fn run_entrypoint_persists_the_model_notice_in_the_selected_session_log() {
    let fixture = Fixture::new();
    let id = "20260921070000_00000000000000000000000000000000";
    fixture.seed_prompt_session(
        id,
        "command: [sh, -c, 'cat']\nmodel: 'provider/run-model:free:xhigh'\nsteps:\n  answer:\n    prompt: run response\n",
        "run model logging",
    );

    let output = fixture.run_command(&["run", id]);
    assert!(output.status.success(), "run failed: {}", terminal(&output));
    let log = std::fs::read_to_string(fixture.manager.run_log_path(id))
        .unwrap_or_else(|error| panic!("read run.log: {error}"));
    assert!(
        log.contains("[info] Model: provider/run-model:free:xhigh"),
        "run.log should retain the info notice: {log}"
    );
    assert_eq!(
        log.matches("Model: provider/run-model:free:xhigh").count(),
        1,
        "one prompt execution should announce its model once: {log}"
    );
}

#[test]
fn foreground_plan_entrypoint_persists_the_initial_model_notice() {
    let fixture = Fixture::new();
    let config = fixture.write_repo_config(
        "command: [sh, -c, 'cat']\nmodel: 'provider/plan-model:free:xhigh'\nsteps:\n  answer:\n    prompt: plan response\n",
    );

    let output = fixture.run_command(&[
        "plan",
        "--config",
        config
            .to_str()
            .unwrap_or_else(|| panic!("config is not UTF-8")),
        "foreground model logging",
    ]);
    assert!(
        output.status.success(),
        "plan failed: {}",
        terminal(&output)
    );
    let session = fixture
        .manager
        .list()
        .unwrap_or_else(|error| panic!("list sessions: {error}"))
        .into_iter()
        .find(|session| session.input == "foreground model logging")
        .unwrap_or_else(|| panic!("foreground plan session was not created"));
    let log = wait_for_log(
        &fixture.manager.run_log_path(&session.id),
        "Model: provider/plan-model:free:xhigh",
    );
    assert!(
        log.contains("[info] Model: provider/plan-model:free:xhigh"),
        "initial CLI planning should persist its model notice: {log}"
    );
}
