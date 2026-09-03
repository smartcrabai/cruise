#![cfg(any(target_os = "macos", target_os = "linux"))]

//! `cruise list` → **Generate Plan** must work for `AwaitingInput` sessions.
//!
//! The list menu offers Generate Plan for both `Draft` and `AwaitingInput`, but
//! the handler used to dispatch to a Draft-only generator and always failed with
//! `expected Draft phase`. This drives the real interactive menu through a PTY
//! and asserts the session ends up in `AwaitingApproval` with the generated
//! title.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// Command backend that writes a fixed plan to `{plan}` instead of calling an
/// LLM (same pattern as `plan_cmd`'s `regenerate_success_config_yaml`).
const CONFIG_YAML: &str = r##"command: ["sh", "-c", "cat >/dev/null; printf '%s' \"$MOCK_PLAN_CONTENT\" > \"$PLAN_FILE\""]
env:
  PLAN_FILE: "{plan}"
  MOCK_PLAN_CONTENT: "# Regenerated Plan Title\n\nStep 1: do the thing.\n"
steps:
  s:
    prompt: hi
"##;

#[cfg(target_os = "linux")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn cruise_env<'c>(command: &'c mut Command, home: &Path) -> &'c mut Command {
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        // Ignore GIT_CONFIG_* pairs inherited from an outer cruise commit guard.
        .env("GIT_CONFIG_COUNT", "0")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env_remove("CRUISE_CONFIG")
        .env_remove("CRUISE_MODEL")
        .env_remove("CRUISE_PLAN_MODEL")
        .env_remove("CRUISE_SDK")
        .env_remove("CRUISE_LANGUAGE_PR")
        .env_remove("CRUISE_LANGUAGE_PLAN")
        .env_remove("CRUISE_CLEANUP_AFTER_PR")
        .env_remove("CRUISE_INTERACTIVE_PLANNING")
        .env_remove("CRUISE_FORCE_EXEC")
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_COUNT", "0")
        .status()
        .unwrap_or_else(|e| panic!("failed to run git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed");
}

fn run_cruise(binary: &Path, home: &Path, cwd: &Path, args: &[&str]) -> Output {
    let output = cruise_env(&mut Command::new(binary), home)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("failed to run cruise {args:?}: {e}"));
    assert!(
        output.status.success(),
        "cruise {args:?} failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn list_json(binary: &Path, home: &Path, cwd: &Path) -> Vec<serde_json::Value> {
    let output = run_cruise(binary, home, cwd, &["list", "--json"]);
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("list --json is not valid JSON: {e}"))
}

/// Drive `cruise list` in a PTY: pick the only session, choose the first action
/// (**Generate Plan**), then back out of the action menu and the picker.
fn run_list_in_pty(binary: &Path, home: &Path, cwd: &Path) -> Output {
    let mut command = Command::new("script");

    #[cfg(target_os = "macos")]
    {
        command.args(["-q", "/dev/null"]);
        command.arg(binary);
        command.arg("list");
    }

    #[cfg(target_os = "linux")]
    {
        let command_line = [binary.to_string_lossy().into_owned(), "list".to_string()]
            .iter()
            .map(|arg| shell_quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        command.args(["-q", "-e", "-c", &command_line, "/dev/null"]);
    }

    let mut child = cruise_env(&mut command, home)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to start script: {e}"));

    let mut input = child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("script stdin should be piped"));
    // Enter: select the session; Enter: "Generate Plan" (first action);
    // Esc: back to the picker; Esc: exit.
    let sequences: &[&[u8]] = &[b"\r", b"\r", b"\x1b", b"\x1b"];
    for sequence in sequences {
        if input.write_all(sequence).is_err() {
            break;
        }
        thread::sleep(Duration::from_secs(2));
    }
    drop(input);

    // Bound the test if a prompt stops consuming input.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match child
            .try_wait()
            .unwrap_or_else(|e| panic!("failed to poll script: {e}"))
        {
            Some(_) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                break;
            }
            None => thread::sleep(Duration::from_millis(50)),
        }
    }

    child
        .wait_with_output()
        .unwrap_or_else(|e| panic!("failed to collect script output: {e}"))
}

#[test]
fn list_generate_plan_regenerates_awaiting_input_session() {
    // Given: a git repo with a plan-writing command backend and a session
    // persisted in AwaitingInput (as SDK planning leaves it after an
    // unanswered `ask_user`).
    let binary = Path::new(env!("CARGO_BIN_EXE_cruise"));
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
    let home = tmp.path();
    let repo = home.join("repo");
    std::fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=cruise-test",
            "-c",
            "user.email=cruise-test@example.com",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let config = repo.join("cruise.yaml");
    std::fs::write(&config, CONFIG_YAML).unwrap_or_else(|e| panic!("{e:?}"));

    let config_arg = config.to_string_lossy().into_owned();
    run_cruise(
        binary,
        home,
        &repo,
        &["draft", "--config", &config_arg, "test task"],
    );
    let sessions = list_json(binary, home, &repo);
    assert_eq!(
        sessions.len(),
        1,
        "draft should create one session: {sessions:?}"
    );
    let id = sessions[0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("session id missing: {sessions:?}"))
        .to_owned();
    assert_eq!(sessions[0]["phase"], "Draft");

    let state_path = home
        .join("data")
        .join("cruise")
        .join("sessions")
        .join(&id)
        .join("state.json");
    let mut state: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&state_path).unwrap_or_else(|e| panic!("{e:?}")),
    )
    .unwrap_or_else(|e| panic!("state.json is not valid JSON: {e}"));
    state["phase"] = serde_json::Value::String("AwaitingInput".to_owned());
    std::fs::write(&state_path, state.to_string()).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(list_json(binary, home, &repo)[0]["phase"], "AwaitingInput");

    // When: Generate Plan is chosen from the interactive list menu
    let output = run_list_in_pty(binary, home, &repo);
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    // Then: the plan is regenerated and the session moves to AwaitingApproval
    assert!(output.status.success(), "cruise list failed: {transcript}");
    assert!(
        !transcript.contains("Plan generation failed"),
        "Generate Plan must not fail for AwaitingInput: {transcript}"
    );
    let sessions = list_json(binary, home, &repo);
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert_eq!(
        sessions[0]["phase"], "AwaitingApproval",
        "transcript: {transcript}"
    );
    assert_eq!(sessions[0]["title"], "Regenerated Plan Title");
    assert_eq!(sessions[0]["plan_available"], true);
}
