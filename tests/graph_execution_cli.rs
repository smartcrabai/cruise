#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    repo: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new(yaml: &str) -> Self {
        let root = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap_or_else(|error| panic!("create repo: {error}"));
        run_git(&repo, &["init", "-q", "-b", "main"]);
        run_git(
            &repo,
            &[
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
        );
        let config = root.path().join("workflow.yaml");
        std::fs::write(&config, yaml).unwrap_or_else(|error| panic!("write workflow: {error}"));
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
            .current_dir(&self.repo)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("HERDR_ENV")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

fn run_git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?} failed to start: {error}"));
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
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

const EXIT_CAPABLE_CYCLE: &str = r"
command: [echo]
steps:
  test:
    command: echo test
  review:
    command: echo review
    if:
      file-changed: test
  finish:
    command: echo finish
";

#[test]
fn exec_dry_run_allows_a_cycle_with_a_proven_normal_exit() {
    // Given: a conditional back edge from review to test and a sequential exit
    // from review to finish.
    let fixture = Fixture::new(EXIT_CAPABLE_CYCLE);

    // When: the public exec dry-run entry point validates the workflow.
    let output = fixture
        .command()
        .arg("--dry-run")
        .output()
        .unwrap_or_else(|error| panic!("exec --dry-run failed to start: {error}"));
    let message = terminal(&output);

    // Then: the cycle is accepted because a normal finish path is structurally
    // available, and dry-run still describes all steps.
    assert!(output.status.success(), "{message}");
    for step in ["test", "review", "finish"] {
        assert!(message.contains(step), "dry-run omitted {step}: {message}");
    }
}

#[test]
fn exec_dry_run_rejects_a_closed_cycle_before_running_steps() {
    // Given: an unconditional self-loop with no normal exit.
    let fixture = Fixture::new(
        r"
command: [echo]
steps:
  loop:
    command: touch started.txt
    next: loop
",
    );

    // When: the public exec dry-run entry point validates the workflow.
    let output = fixture
        .command()
        .arg("--dry-run")
        .output()
        .unwrap_or_else(|error| panic!("exec --dry-run failed to start: {error}"));
    let message = terminal(&output);

    // Then: the invalid closed cycle is rejected, names the affected step, and
    // does not execute its command as part of validation.
    assert!(
        !output.status.success(),
        "closed cycle unexpectedly accepted: {message}"
    );
    assert!(
        message.contains("loop"),
        "diagnostic must name the cycle: {message}"
    );
    assert!(
        !fixture.repo.join("started.txt").exists(),
        "preflight validation must run before workflow commands"
    );
}

#[test]
fn exec_runs_a_cycle_until_its_conditional_exit_is_taken() {
    // Given: review changes the workspace only on its first visit. The first
    // change takes the back edge, while the second visit follows finish.
    let fixture = Fixture::new(
        r"
command: [echo]
steps:
  test:
    command: printf test >> visits.txt
  review:
    command: if [ ! -e .reviewed ]; then touch .reviewed; fi
    if:
      file-changed: test
  finish:
    command: touch finished.txt
",
    );

    // When: the public exec entry point runs the workflow normally.
    let output = fixture
        .command()
        .output()
        .unwrap_or_else(|error| panic!("exec failed to start: {error}"));
    let message = terminal(&output);

    // Then: execution terminates through the available normal exit instead of
    // being rejected merely because the graph contains a cycle.
    assert!(output.status.success(), "{message}");
    assert!(fixture.repo.join(".reviewed").exists(), "{message}");
    assert!(fixture.repo.join("finished.txt").exists(), "{message}");
}

#[test]
fn after_pr_closed_cycle_is_rejected_before_main_including_dry_run() {
    let fixture = Fixture::new(
        "command: [echo]\nsteps:\n  main:\n    command: touch started.txt\nafter-pr:\n  loop:\n    command: touch after.txt\n    next: loop\n",
    );
    for dry_run in [false, true] {
        let mut command = fixture.command();
        if dry_run {
            command.arg("--dry-run");
        }
        let output = command.output().unwrap_or_else(|error| panic!("{error}"));
        assert!(!output.status.success(), "{}", terminal(&output));
        assert!(!fixture.repo.join("started.txt").exists());
        assert!(!fixture.repo.join("after.txt").exists());
    }
}
