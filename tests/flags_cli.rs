#![cfg(unix)]
//! End-to-end contract for `cruise exec --flag`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const WORKFLOW: &str = "command: [echo]
flags:
  e2e:
    description: Run E2E
  docs: {}
steps:
  build:
    command: touch build.txt
  e2e-test:
    flag: e2e
    command: touch e2e.txt
  docs-test:
    flag: docs
    command: touch docs.txt
  any-test:
    flag: [e2e, docs]
    command: touch any.txt
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
        let root = TempDir::new().unwrap_or_else(|error| {
            panic!("failed to create temporary fixture directory: {error}")
        });
        let repo = root.path().join("repo");
        std::fs::create_dir(&repo).unwrap_or_else(|error| {
            panic!("failed to create fixture repository directory: {error}")
        });
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
fn exec_without_flags_runs_unflagged_steps_and_skips_flagged_side_effects() {
    let f = Fixture::new();
    let out = f.exec(&[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("build.txt").exists());
    assert!(!f.repo.join("e2e.txt").exists());
    assert!(!f.repo.join("docs.txt").exists());
    assert!(!f.repo.join("any.txt").exists());
}

#[test]
fn exec_accepts_a_bare_flag_name() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("e2e.txt").exists());
    assert!(f.repo.join("any.txt").exists());
    assert!(!f.repo.join("docs.txt").exists());
}

#[test]
fn exec_enables_any_flag_list_member_without_enabling_others() {
    let f = Fixture::new();
    let out = f.exec(&["-f", "docs"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("docs.txt").exists());
    assert!(f.repo.join("any.txt").exists());
    assert!(!f.repo.join("e2e.txt").exists());
}

#[test]
fn exec_accepts_a_comma_separated_flag_list() {
    let f = Fixture::new();
    let out = f.exec(&["-f", "e2e,docs"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("e2e.txt").exists());
    assert!(f.repo.join("docs.txt").exists());
    assert!(f.repo.join("any.txt").exists());
}

#[test]
fn exec_accepts_repeated_flag_arguments() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e", "-f", "docs"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("e2e.txt").exists());
    assert!(f.repo.join("docs.txt").exists());
    assert!(f.repo.join("any.txt").exists());
}

#[test]
fn exec_accepts_explicit_off_in_a_comma_separated_list() {
    let f = Fixture::new();
    let out = f.exec(&["-f", "e2e,docs=off"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("e2e.txt").exists());
    assert!(f.repo.join("any.txt").exists());
    assert!(!f.repo.join("docs.txt").exists());
}

#[test]
fn exec_explicit_off_skips_the_flagged_step() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e=off"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(f.repo.join("build.txt").exists());
    assert!(!f.repo.join("e2e.txt").exists());
    assert!(!f.repo.join("any.txt").exists());
}

#[test]
fn exec_rejects_malformed_and_empty_flag_values() {
    for invalid in ["e2e=maybe", "e2e=", "e2e,,docs", ",e2e", "e2e,", ""] {
        let f = Fixture::new();
        let out = f.exec(&["--flag", invalid]);
        let message = text(&out);
        assert!(!out.status.success(), "{invalid:?}: {message}");
        assert!(!f.repo.join("build.txt").exists(), "{invalid:?}: {message}");
    }
}

#[test]
fn exec_rejects_duplicate_flag_names() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e,e2e"]);
    let message = text(&out);
    assert!(!out.status.success(), "{message}");
    assert!(message.contains("more than once"), "{message}");
    assert!(!f.repo.join("build.txt").exists());
}

#[test]
fn exec_rejects_an_undeclared_flag_name() {
    let f = Fixture::new();
    let out = f.exec(&["--flag", "e2e,ghost"]);
    let message = text(&out);
    assert!(!out.status.success(), "{message}");
    assert!(message.contains("ghost"), "{message}");
    assert!(!f.repo.join("build.txt").exists());
}
