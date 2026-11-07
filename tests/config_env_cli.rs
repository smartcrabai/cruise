use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    project: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("create fixture: {error}"));
        let project = root.path().join("project");
        std::fs::create_dir_all(&project).unwrap_or_else(|error| panic!("create project: {error}"));
        std::fs::write(
            project.join("workflow.yaml"),
            "command: [echo]\nsteps:\n  main:\n    command: echo main\n",
        )
        .unwrap_or_else(|error| panic!("write workflow: {error}"));
        Self { root, project }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        command
            .current_dir(&self.project)
            .env("HOME", self.root.path().join("home"))
            .env("USERPROFILE", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
            .env_remove("HERDR_ENV")
            .env_remove("JCODE_MODEL")
            .stdin(Stdio::null());
        command
    }

    fn init_git(&self) {
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.com",
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
        ] {
            let output = Command::new("git")
                .current_dir(&self.project)
                .env("GIT_CONFIG_COUNT", "0")
                .args(&args)
                .output()
                .unwrap_or_else(|error| panic!("run git: {error}"));
            assert!(output.status.success(), "{}", text(&output));
        }
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn config_displays_environment_parallelism_without_persisting_it() {
    let fixture = Fixture::new();
    let show = fixture
        .command()
        .env("CRUISE_RUN_ALL_PARALLELISM", "4")
        .arg("config")
        .output()
        .unwrap_or_else(|error| panic!("show config: {error}"));
    assert!(show.status.success(), "{}", text(&show));
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(stdout.contains("run_all_parallelism = 4"), "{stdout}");
    assert!(
        stdout.contains("from CRUISE_RUN_ALL_PARALLELISM"),
        "{stdout}"
    );

    let set = fixture
        .command()
        .env("CRUISE_RUN_ALL_PARALLELISM", "4")
        .args(["config", "--set-parallelism", "2"])
        .output()
        .unwrap_or_else(|error| panic!("set config: {error}"));
    assert!(set.status.success(), "{}", text(&set));
    let show_raw = fixture
        .command()
        .arg("config")
        .output()
        .unwrap_or_else(|error| panic!("show persisted config: {error}"));
    assert!(show_raw.status.success(), "{}", text(&show_raw));
    assert!(
        String::from_utf8_lossy(&show_raw.stdout).contains("run_all_parallelism = 2"),
        "{}",
        text(&show_raw)
    );
    let saved = std::fs::read_to_string(fixture.root.path().join("config/cruise/config.json"))
        .unwrap_or_else(|error| panic!("read saved config: {error}"));
    let saved: serde_json::Value =
        serde_json::from_str(&saved).unwrap_or_else(|error| panic!("parse saved config: {error}"));
    assert_eq!(saved["runAllParallelism"], 2);
}

#[test]
fn invalid_environment_parallelism_is_rejected() {
    let fixture = Fixture::new();
    for value in ["0", "abc", "-1", "+1", "1.5"] {
        let output = fixture
            .command()
            .env("CRUISE_RUN_ALL_PARALLELISM", value)
            .arg("config")
            .output()
            .unwrap_or_else(|error| panic!("show config: {error}"));
        assert!(!output.status.success(), "value {value}: {}", text(&output));
        assert!(
            text(&output).contains("CRUISE_RUN_ALL_PARALLELISM"),
            "value {value}: {}",
            text(&output)
        );
    }
}

#[test]
fn invalid_boolean_override_fails_in_real_plan_command() {
    let fixture = Fixture::new();
    fixture.init_git();
    let output = fixture
        .command()
        .env("CRUISE_CLEANUP_AFTER_PR", "yes")
        .args([
            "plan",
            "--skip-planning",
            "--no-force-exec",
            "-c",
            "workflow.yaml",
            "test plan",
        ])
        .output()
        .unwrap_or_else(|error| panic!("run plan: {error}"));
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("CRUISE_CLEANUP_AFTER_PR"),
        "{}",
        text(&output)
    );
}

#[test]
fn conflicting_sdk_and_command_env_fail_before_execution() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("CRUISE_SDK", "jcode")
        .env("CRUISE_COMMAND", "[\"echo\"]")
        .args(["exec", "--dry-run", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run exec: {error}"));
    let message = text(&output);
    assert!(!output.status.success(), "{message}");
    assert!(
        message.contains("CRUISE_SDK")
            && message.contains("CRUISE_COMMAND")
            && message.contains("mutually exclusive"),
        "{message}"
    );
}

#[test]
fn invalid_count_override_fails_with_variable_name() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("CRUISE_MAX_RETRIES", "-1")
        .args(["exec", "--dry-run", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|error| panic!("run exec: {error}"));
    let message = text(&output);
    assert!(!output.status.success(), "{message}");
    assert!(message.contains("CRUISE_MAX_RETRIES"), "{message}");
}

#[cfg(unix)]
#[test]
fn no_force_exec_flag_overrides_environment_for_plan() {
    let fixture = Fixture::new();
    fixture.init_git();
    let marker = fixture.project.join("executed-marker");
    std::fs::write(
        fixture.project.join("workflow.yaml"),
        "command: [echo]\nsteps:\n  main:\n    command: touch executed-marker\n",
    )
    .unwrap_or_else(|error| panic!("write workflow: {error}"));
    let output = fixture
        .command()
        .env("CRUISE_FORCE_EXEC", "true")
        .args([
            "plan",
            "--skip-planning",
            "--no-force-exec",
            "-c",
            "workflow.yaml",
            "test plan",
        ])
        .output()
        .unwrap_or_else(|error| panic!("run plan: {error}"));
    assert!(output.status.success(), "{}", text(&output));
    assert!(!marker.exists(), "the workflow must not execute directly");
    let manager = cruise::session::SessionManager::new(fixture.root.path().join("data/cruise"));
    assert_eq!(
        manager
            .list()
            .unwrap_or_else(|error| panic!("list sessions: {error}"))
            .len(),
        1,
        "{}",
        text(&output)
    );
}
