#![cfg(unix)]

//! CLI validation keeps ordered model fallback candidates as plain strings.
use std::process::Command;
use tempfile::TempDir;

#[test]
fn jcode_fallback_strings_pass_real_exec_validation() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let git = Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(temp.path())
        .output()
        .unwrap_or_else(|error| panic!("initialize test repository: {error}"));
    assert!(git.status.success(), "initialize test repository");
    let workflow = temp.path().join("workflow.yaml");
    std::fs::write(
        &workflow,
        "sdk: jcode\nmodel: [claude/claude-opus-4-5:high, openai/gpt-5:medium]\nplan_model: [openai/gpt-5, claude/claude-sonnet-5-5]\nsteps:\n  main:\n    prompt: inspect\n",
    )
    .unwrap_or_else(|error| panic!("write workflow: {error}"));

    let output = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", temp.path().join("home"))
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("GIT_CONFIG_COUNT", "0")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .current_dir(temp.path())
        .args(["exec", "--dry-run", "--config"])
        .arg(&workflow)
        .output()
        .unwrap_or_else(|error| panic!("run exec dry-run: {error}"));
    assert!(
        output.status.success(),
        "jcode plain-model fallback config was rejected: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
