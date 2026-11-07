#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

fn write_executable_script(path: &Path, script: &str) {
    std::fs::write(path, script).expect("write agent");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("make agent executable");
}

struct Fixture {
    root: TempDir,
    repo: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).expect("create repo");
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
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
        let agent = root.path().join("agent.sh");
        write_executable_script(
            &agent,
            r#"#!/bin/sh
cat > "$AGENT_LOG.prompt"
if [ -f "$AGENT_LOG" ]; then
  echo review >> "$AGENT_LOG"
  if [ "$CHANGE_REVIEW" = 1 ]; then echo changed >> review-change.txt; fi
  if [ "$CHANGE_TRACKED" = 1 ] && [ "$(grep -c review "$AGENT_LOG")" = 1 ]; then echo changed >> target/tracked.txt; fi
else
  echo implement >> "$AGENT_LOG"
fi
"#,
        );
        let config = root.path().join("config/cruise");
        std::fs::create_dir_all(&config).expect("create config dir");
        let yaml = format!(
            "command: [{}]\n",
            serde_json::to_string(agent.to_str().unwrap()).unwrap()
        );
        std::fs::write(config.join("default.yaml"), yaml).expect("write defaults");
        let log = root.path().join("agent.log");
        Self { root, repo, log }
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
            .current_dir(&self.repo)
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("AGENT_LOG", &self.log)
            .env("CHANGE_REVIEW", "0")
            .env_remove("HERDR_ENV")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn assert_no_sessions(&self) {
        let sessions = self.root.path().join("data/cruise/sessions");
        assert!(
            !sessions.exists() || std::fs::read_dir(sessions).unwrap().next().is_none(),
            "exec session was retained"
        );
    }
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
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

#[test]
fn instant_preserves_task_whitespace_and_rejects_blank() {
    let f = Fixture::new();
    let output = f.command().args(["--instant", "  task with spaces  "]).output().unwrap();
    assert!(output.status.success(), "{}", terminal(&output));
    let prompt = std::fs::read_to_string(f.log.with_extension("log.prompt")).unwrap();
    assert!(prompt.contains("\n  task with spaces  \n"), "{prompt:?}");
    let output = f.command().args(["--instant", "   "]).output().unwrap();
    assert!(!output.status.success(), "{}", terminal(&output));
    assert!(terminal(&output).contains("non-empty task"));
    assert_eq!(f.calls(), ["implement"]);
}

#[test]
fn instant_exec_runs_once_and_removes_session() {
    let f = Fixture::new();
    let output = f
        .command()
        .args(["--instant", "add greeting"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", terminal(&output));
    assert_eq!(f.calls(), ["implement"]);
    f.assert_no_sessions();
}

#[test]
fn dry_run_does_not_spawn_or_create_session() {
    let f = Fixture::new();
    let output = f
        .command()
        .args(["--instant", "--dry-run", "task"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", terminal(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("implement"),
        "{}",
        terminal(&output)
    );
    assert!(f.calls().is_empty());
    f.assert_no_sessions();
}

#[test]
fn review_stops_on_no_changes_or_at_round_limit() {
    for (change, rounds, expected) in [("1", None, 2), ("0", None, 1), ("1", Some("1"), 1)] {
        let f = Fixture::new();
        let mut cmd = f.command();
        cmd.args(["--instant", "--review"])
            .env("CHANGE_REVIEW", change);
        if let Some(rounds) = rounds {
            cmd.args(["--review-rounds", rounds]);
        }
        let output = cmd.arg("task").output().unwrap();
        assert!(output.status.success(), "{}", terminal(&output));
        let calls = f.calls();
        assert_eq!(calls.first().map(String::as_str), Some("implement"));
        assert_eq!(
            calls.iter().filter(|call| *call == "review").count(),
            expected,
            "{}",
            terminal(&output)
        );
    }
}

#[test]
fn review_detects_tracked_file_in_target() {
    let f = Fixture::new();
    std::fs::create_dir(f.repo.join("target")).unwrap();
    std::fs::write(f.repo.join("target/tracked.txt"), "original").unwrap();
    git(&f.repo, &["add", "-f", "target/tracked.txt"]);
    let output = f.command().env("CHANGE_TRACKED", "1").args(["--instant", "--review", "task"]).output().unwrap();
    assert!(output.status.success(), "{}", terminal(&output));
    assert_eq!(f.calls(), ["implement", "review", "review"]);
}

#[test]
fn failed_verification_fails_the_run() {
    let f = Fixture::new();
    let output = f
        .command()
        .args(["--instant", "--verify", "false", "task"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", terminal(&output));
    assert!(
        terminal(&output).contains("verify-1"),
        "{}",
        terminal(&output)
    );
    assert_eq!(f.calls(), ["implement"]);
}

#[test]
fn verify_commands_preserve_shell_text_and_run_in_order() {
    let f = Fixture::new();
    let output = f.command().args([
        "--instant",
        "--verify", "printf '%s\\n' \"a: b\" > verify-output.txt",
        "--verify", "test \"$(cat verify-output.txt)\" = 'a: b' && printf '%s\\n' second >> verify-output.txt",
        "task",
    ]).output().unwrap();
    assert!(output.status.success(), "{}", terminal(&output));
    assert_eq!(
        std::fs::read_to_string(f.repo.join("verify-output.txt")).unwrap(),
        "a: b\nsecond\n"
    );
}

#[test]
fn instant_ignores_config_search_but_normal_exec_does_not() {
    let f = Fixture::new();
    let missing = f.root.path().join("missing.yaml");
    let output = f
        .command()
        .env("CRUISE_CONFIG", &missing)
        .args(["--instant", "task"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", terminal(&output));
    let output = f
        .command()
        .env("CRUISE_CONFIG", &missing)
        .arg("task")
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", terminal(&output));
    assert_eq!(f.calls(), ["implement"]);
}

#[test]
fn non_git_directory_fails_before_agent() {
    let f = Fixture::new();
    let dir = f.root.path().join("not-a-repo");
    std::fs::create_dir(&dir).unwrap();
    let output = f
        .command()
        .current_dir(dir)
        .args(["--instant", "task"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", terminal(&output));
    assert!(f.calls().is_empty());
}

#[test]
fn instant_rejects_config_and_flags() {
    let f = Fixture::new();
    for args in [
        ["--instant", "-c", "x.yaml", "task"],
        ["--instant", "-f", "after_pr", "task"],
    ] {
        let output = f.command().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{}", terminal(&output));
    }
}

#[test]
fn review_budget_cannot_exceed_cli_max_retries() {
    let f = Fixture::new();
    let output = f
        .command()
        .args([
            "--instant",
            "--review",
            "--review-rounds",
            "4",
            "--max-retries",
            "3",
            "task",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{}", terminal(&output));
    assert!(f.calls().is_empty());
}
