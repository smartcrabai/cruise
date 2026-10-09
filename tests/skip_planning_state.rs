#![cfg(unix)]

//! Entry-point contract: sessions created with `--skip-planning` persist an
//! empty `input` and keep the task text in `plan.md`.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use cruise::session::{SessionManager, SessionPhase};
use tempfile::TempDir;

const CONFIG: &str = "command: [cat]\nsteps:\n  s1:\n    prompt: plan\n";

struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let repo = root.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("mkdir: {e}"));
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &[
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
        );
        std::fs::write(repo.join("cruise.yaml"), CONFIG).unwrap_or_else(|e| panic!("{e}"));
        Self { root }
    }

    fn repo(&self) -> std::path::PathBuf {
        self.root.path().join("repo")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        command
            .current_dir(self.repo())
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("GIT_CONFIG_COUNT", "0")
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env("NO_COLOR", "1")
            .env_remove("HERDR_ENV")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn manager(&self) -> SessionManager {
        SessionManager::new(self.root.path().join("data/cruise"))
    }

    fn assert_skip_planning_state(&self, task: &str) {
        let manager = self.manager();
        let sessions = manager.list().unwrap_or_else(|e| panic!("list: {e}"));
        assert_eq!(sessions.len(), 1, "expected exactly one session");
        let session = &sessions[0];
        let raw =
            std::fs::read_to_string(manager.sessions_dir().join(&session.id).join("state.json"))
                .unwrap_or_else(|e| panic!("state.json: {e}"));
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(json["input"], serde_json::json!(""), "{raw}");
        assert_eq!(json["input_as_plan"], serde_json::json!(true), "{raw}");
        assert!(
            session
                .title
                .as_deref()
                .is_some_and(|t| !t.trim().is_empty()),
            "title should be set: {raw}"
        );
        assert_eq!(session.phase, SessionPhase::Planned);
        assert_eq!(
            std::fs::read_to_string(session.plan_path(&manager.sessions_dir()))
                .unwrap_or_else(|e| panic!("plan.md: {e}"))
                .trim(),
            task
        );
    }
}

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_COUNT", "0")
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(output.status.success(), "git {args:?} failed");
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn plan_skip_planning_persists_empty_input() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .args(["plan", "--skip-planning", "--config"])
        .arg(fixture.repo().join("cruise.yaml"))
        .arg("skip planning foreground task")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(output.status.success(), "{}", text(&output));

    fixture.assert_skip_planning_state("skip planning foreground task");
}

#[test]
fn background_plan_skip_planning_persists_empty_input() {
    let fixture = Fixture::new();
    let mut child = fixture
        .command()
        .args(["--plan", "stdin", "--skip-planning"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("{e}"));
    child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("stdin"))
        .write_all(b"skip planning background task")
        .unwrap_or_else(|e| panic!("{e}"));
    let output = child.wait_with_output().unwrap_or_else(|e| panic!("{e}"));
    assert!(output.status.success(), "{}", text(&output));

    fixture.assert_skip_planning_state("skip planning background task");
}
