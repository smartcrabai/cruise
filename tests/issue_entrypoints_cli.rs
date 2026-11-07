//! CLI entry point contracts for GitHub issue input.
use std::process::{Command, Stdio};

#[cfg(unix)]
#[test]
fn gitlab_issue_rejection_precedes_glab_check() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("test operation failed: {error}"));
    for (args, expected) in [
        (
            vec![
                "plan",
                "--repo",
                "https://gitlab.com/group/project",
                "--issue",
                "12",
            ],
            "GitHub repositories only",
        ),
        (
            vec![
                "--plan",
                "#12",
                "--repo",
                "https://gitlab.com/group/project",
            ],
            "GitHub repositories only",
        ),
        (
            vec!["plan", "--repo", "https://gitlab.com/group/project", "task"],
            "glab CLI is not installed",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_cruise"))
            .args(args)
            .env("HOME", root.path())
            .env("XDG_DATA_HOME", root.path().join("data"))
            .env("PATH", root.path())
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|error| panic!("test operation failed: {error}"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success() && stderr.contains(expected),
            "{stderr}"
        );
    }
}

#[test]
fn issue_dry_run_identifies_issue_without_creating_session() {
    let root = tempfile::tempdir().unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let output = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .args(["plan", "--issue", "12", "--dry-run", "-c", "__builtin__"])
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("#12"));
    assert!(!root.path().join("data/cruise/sessions").exists());
}

#[test]
fn issue_flags_are_exposed_by_plan_and_draft_but_not_exec() {
    for command in ["plan", "draft"] {
        let output = Command::new(env!("CARGO_BIN_EXE_cruise"))
            .args([command, "--help"])
            .stdin(Stdio::null())
            .output()
            .unwrap_or_else(|error| panic!("run cruise help: {error}"));
        assert!(output.status.success());
        let help = String::from_utf8_lossy(&output.stdout);
        assert!(help.contains("--issue"), "{command} help: {help}");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .args(["exec", "--issue", "12"])
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("run cruise exec: {error}"));
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--issue"));
}

#[cfg(unix)]
#[test]
fn draft_issue_fetches_and_persists_issue_with_additional_instructions() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let root =
        tempfile::tempdir().unwrap_or_else(|error| panic!("create temporary directory: {error}"));
    let project = root.path().join("project");
    let bin = root.path().join("bin");
    fs::create_dir_all(&project).unwrap_or_else(|error| panic!("test operation failed: {error}"));
    fs::create_dir_all(&bin).unwrap_or_else(|error| panic!("test operation failed: {error}"));
    for args in [
        vec!["init", "-q"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/example/project.git",
        ],
    ] {
        assert!(
            Command::new("git")
                .current_dir(&project)
                .args(args)
                .status()
                .unwrap_or_else(|error| panic!("test operation failed: {error}"))
                .success()
        );
    }
    let gh = bin.join("gh");
    fs::write(&gh, r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_TEST_LOG"
case "$*" in
  *issues/12/comments*) printf '%s\n' '[{"user":{"login":"reviewer"},"body":"A useful comment"}]' ;;
  *issues/12*) printf '%s\n' '{"number":12,"title":"Fix widget","body":"Issue details","html_url":"https://github.com/example/project/issues/12","labels":[{"name":"bug"}]}' ;;
  *auth\ token*) exit 1 ;;
  *) exit 0 ;;
esac
"#).unwrap_or_else(|error| panic!("test operation failed: {error}"));
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755))
        .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let log = root.path().join("gh.log");
    let output = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .args([
            "draft",
            "--issue",
            "12",
            "Please add tests",
            "-c",
            "__builtin__",
        ])
        .current_dir(&project)
        .env("HOME", root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env(
            "PATH",
            std::env::join_paths(paths)
                .unwrap_or_else(|error| panic!("test operation failed: {error}")),
        )
        .env("GH_TEST_LOG", &log)
        .env("CRUISE_DISABLE_HERDR", "1")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("run draft: {error}"));
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_draft_issue_persisted(root.path(), &log);
}

#[cfg(unix)]
fn assert_draft_issue_persisted(root: &std::path::Path, log: &std::path::Path) {
    use std::fs;

    let sessions = root.join("data/cruise/sessions");
    let entries: Vec<_> = fs::read_dir(&sessions)
        .unwrap_or_else(|error| panic!("read sessions directory: {error}"))
        .map(|e| e.unwrap_or_else(|error| panic!("test operation failed: {error}")))
        .collect();
    assert_eq!(entries.len(), 1);
    let state: serde_json::Value = serde_json::from_slice(
        &fs::read(entries[0].path().join("state.json"))
            .unwrap_or_else(|error| panic!("test operation failed: {error}")),
    )
    .unwrap_or_else(|error| panic!("test operation failed: {error}"));
    assert_eq!(state["issue_number"], 12);
    let input = state["input"]
        .as_str()
        .unwrap_or_else(|| panic!("missing session input"));
    for expected in [
        "# Fix widget",
        "Issue #12:",
        "Issue details",
        "bug",
        "reviewer",
        "A useful comment",
        "Please add tests",
    ] {
        assert!(
            input.contains(expected),
            "missing {expected:?} in {input:?}"
        );
    }
    let calls = fs::read_to_string(log).unwrap_or_else(|error| panic!("read gh calls: {error}"));
    assert!(calls.contains("issues/12"), "{calls}");
    assert!(calls.contains("issues/12/comments"), "{calls}");
}
