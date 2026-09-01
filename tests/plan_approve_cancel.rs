#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

#[derive(Clone, Copy)]
enum ActionChoice {
    Approve,
    ExecuteNow,
}

#[cfg(target_os = "linux")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn run_plan_in_pty(binary: &Path, config: &Path, home: &Path, action: ActionChoice) -> Output {
    let mut command = Command::new("script");

    #[cfg(target_os = "macos")]
    {
        command.args(["-q", "/dev/null"]);
        command.arg(binary);
        command.args(["plan", "--skip-planning", "--config"]);
        command.arg(config);
        command.arg("test plan");
    }

    #[cfg(target_os = "linux")]
    {
        let args = [
            binary.to_string_lossy().into_owned(),
            "plan".to_string(),
            "--skip-planning".to_string(),
            "--config".to_string(),
            config.to_string_lossy().into_owned(),
            "test plan".to_string(),
        ];
        let command_line = args
            .iter()
            .map(|arg| shell_quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        command.args(["-q", "-e", "-c", &command_line, "/dev/null"]);
    }

    let mut child = command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        // Ignore GIT_CONFIG_* pairs inherited from an outer cruise commit guard.
        .env("GIT_CONFIG_COUNT", "0")
        .env_remove("CRUISE_CONFIG")
        .env_remove("CRUISE_MODEL")
        .env_remove("CRUISE_PLAN_MODEL")
        .env_remove("CRUISE_SDK")
        .env_remove("CRUISE_LANGUAGE_PR")
        .env_remove("CRUISE_LANGUAGE_PLAN")
        .env_remove("CRUISE_CLEANUP_AFTER_PR")
        .env_remove("CRUISE_INTERACTIVE_PLANNING")
        .env_remove("CRUISE_FORCE_EXEC")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("failed to start script: {e}"));

    let mut input = child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("script stdin should be piped"));
    let sequences: &[&[u8]] = match action {
        ActionChoice::Approve => &[b"\r", b"\x1b", b"\x1b"],
        ActionChoice::ExecuteNow => &[b"\x1b[B", b"\x1b[B", b"\x1b[B", b"\r", b"\x1b", b"\x1b"],
    };
    for sequence in sequences {
        if input.write_all(sequence).is_err() {
            break;
        }
        thread::sleep(Duration::from_secs(2));
    }
    drop(input);

    // Bound the test if a prompt stops consuming input; otherwise a failed
    // interactive regression could leave the test process waiting forever.
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

fn assert_cancel_returns_to_action_menu(action: ActionChoice) {
    // Given: a workflow with a skippable step and an interactive plan entry point
    let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
    let home = tmp.path();
    let config = tmp.path().join("cruise.yaml");
    std::fs::write(
        &config,
        "command: [echo]\nsteps:\n  check:\n    command: echo check\n",
    )
    .unwrap_or_else(|e| panic!("{e:?}"));

    // When: the action is selected, then the Steps to skip prompt is cancelled,
    // and the action menu is cancelled as well
    let output = run_plan_in_pty(
        Path::new(env!("CARGO_BIN_EXE_cruise")),
        &config,
        home,
        action,
    );
    let transcript = String::from_utf8_lossy(&output.stdout);
    let transcript_with_stderr = format!("{transcript}{}", String::from_utf8_lossy(&output.stderr));

    // Then: cancelling the skip prompt redraws the action menu instead of approving
    assert!(
        output.status.success(),
        "interactive plan failed: {transcript_with_stderr}"
    );
    assert!(
        transcript_with_stderr.matches("Publish as Issue").count() >= 2,
        "action menu should be shown again after skip cancellation: {transcript_with_stderr}"
    );
    assert!(
        transcript_with_stderr.contains("discarded."),
        "the final action-menu cancellation should discard the session: {transcript_with_stderr}"
    );
    assert!(
        !transcript_with_stderr.contains("created."),
        "skip cancellation must not approve the session: {transcript_with_stderr}"
    );
}

#[test]
fn skip_prompt_cancellation_returns_to_action_menu_for_both_actions() {
    for action in [ActionChoice::Approve, ActionChoice::ExecuteNow] {
        assert_cancel_returns_to_action_menu(action);
    }
}
