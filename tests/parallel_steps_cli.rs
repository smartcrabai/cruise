#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    repo: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new(yaml: &str) -> Self {
        let root = TempDir::new().unwrap_or_else(|e| panic!("{e}"));
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap_or_else(|e| panic!("{e}"));
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
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
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap_or_else(|e| panic!("{e}"));
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let config = root.path().join("workflow.yaml");
        std::fs::write(&config, yaml).unwrap_or_else(|e| panic!("{e}"));
        Self { root, repo, config }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        command
            .arg("exec")
            .arg("--config")
            .arg(&self.config)
            .arg("review")
            .current_dir(&self.repo)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

#[test]
fn parallel_cli_prints_each_child_stream_once_with_its_name() {
    let fixture = Fixture::new(
        r"
command: [sh, -c, 'cat; echo prompt-stderr >&2']
steps:
  before:
    command: 'echo sequential-stdout; echo sequential-stderr >&2'
  checks:
    parallel:
      command_child:
        command: 'echo command-stdout; echo command-stderr >&2'
      prompt_child:
        prompt: prompt-stdout
",
    );
    let output = fixture.command().output().unwrap_or_else(|e| panic!("{e}"));
    let terminal = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{terminal}");
    for marker in [
        "sequential-stdout",
        "sequential-stderr",
        "command-stdout",
        "command-stderr",
        "prompt-stdout",
        "prompt-stderr",
    ] {
        assert_eq!(
            terminal.matches(marker).count(),
            1,
            "missing or duplicate {marker}: {terminal}"
        );
    }
    for line in [
        "[checks/command_child] command-stdout",
        "[checks/command_child] command-stderr",
        "[checks/prompt_child] prompt-stdout",
        "[checks/prompt_child] prompt-stderr",
    ] {
        assert!(
            terminal.contains(line),
            "missing child prefix for {line}: {terminal}"
        );
    }
}

#[test]
fn parallel_cli_ctrl_c_drains_children_before_suspending() {
    let fixture = Fixture::new(
        r"
steps:
  checks:
    parallel:
      first: { command: 'touch first.started; sleep 2; touch first.leaked' }
      second: { command: 'touch second.started; sleep 2; touch second.leaked' }
  unexpected:
    command: touch unexpected
",
    );
    let mut child = fixture.command().spawn().unwrap_or_else(|e| panic!("{e}"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fixture.repo.join("first.started").exists()
        || !fixture.repo.join("second.started").exists()
    {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap_or_else(|e| panic!("{e}"));
            panic!(
                "children failed to start: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = i32::try_from(child.id()).unwrap_or_else(|e| panic!("{e}"));
    // Signal only the Cruise process, as the user's terminal does. Its child
    // commands own separate process groups and must be cancelled explicitly.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
    let output = child.wait_with_output().unwrap_or_else(|e| panic!("{e}"));
    assert!(!output.status.success());
    std::thread::sleep(Duration::from_millis(2300));
    for file in ["first.leaked", "second.leaked", "unexpected"] {
        assert!(
            !fixture.repo.join(file).exists(),
            "{file} written after interruption"
        );
    }
    let sessions = fixture.root.path().join("data/cruise/sessions");
    let states: Vec<_> = std::fs::read_dir(sessions)
        .unwrap_or_else(|e| panic!("{e}"))
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("{e}"))
                .path()
                .join("state.json")
        })
        .filter(|path| path.exists())
        .collect();
    assert_eq!(
        states.len(),
        1,
        "interrupted exec session must remain resumable"
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&states[0]).unwrap_or_else(|e| panic!("{e}")))
            .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(state["phase"], "Suspended");
    assert_eq!(state["current_step_is_node_id"], true);
    assert!(
        state["current_step"]
            .as_str()
            .is_some_and(|step| step.starts_with('n')),
        "DAG checkpoint should persist a node id: {}",
        state["current_step"]
    );
}
