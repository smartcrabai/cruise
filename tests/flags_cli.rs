#![cfg(unix)]
//! Entry-point contract for `cruise exec --flag NAME=on|off`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const WORKFLOW: &str = "command: [echo]
flags:
  e2e:
    description: Run E2E
steps:
  build:
    command: touch build.txt
  e2e-test:
    flag: e2e
    command: touch e2e.txt
";

struct Fixture {
    root: TempDir,
    repo: PathBuf,
    config: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git command: {error}"));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new()
            .unwrap_or_else(|error| panic!("failed to create temporary fixture directory: {error}"));
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo)
            .unwrap_or_else(|error| panic!("failed to create fixture repository directory: {error}"));
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=T",
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
                "x",
            ],
        );
        let config = root.path().join("workflow.yaml");
        std::fs::write(&config, WORKFLOW)
            .unwrap_or_else(|error| panic!("failed to write fixture workflow config: {error}"));
        Self { root, repo, config }
    }

    fn exec(&self, extra: &[&str]) -> Output {
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
            .args(extra)
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
            .output()
            .unwrap_or_else(|error| panic!("failed to run cruise exec command: {error}"))
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
fn exec_flag_off_skips_the_flagged_step() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e=off"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("build.txt").exists());
    assert!(!f.repo.join("e2e.txt").exists());
}

#[test]
fn exec_flag_on_runs_the_flagged_step() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e=on"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("e2e.txt").exists());
}

#[test]
fn exec_without_a_value_for_a_declared_flag_fails_before_running_anything() {
    let f = Fixture::new();
    let out = f.exec(&[]);
    let message = text(&out);
    assert!(!out.status.success(), "{message}");
    assert!(message.contains("e2e"), "{message}");
    assert!(!f.repo.join("build.txt").exists(), "nothing may run");
}

#[test]
fn exec_rejects_an_undeclared_flag_name() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e=on", "--flag", "ghost=on"]);
    let message = text(&out);
    assert!(!out.status.success(), "{message}");
    assert!(message.contains("ghost"), "{message}");
    assert!(!f.repo.join("build.txt").exists());
}
