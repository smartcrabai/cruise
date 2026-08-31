#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn script_is_available() -> bool {
    match Command::new("script").arg("-V").output() {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => panic!("failed to probe script: {error}"),
    }
}

fn run_tui_in_pty(binary: &Path, home: &Path) -> Output {
    let mut command = Command::new("script");

    let command_line = format!(
        "stty cols 120 rows 30; exec {}",
        shell_quote(&binary.to_string_lossy())
    );

    #[cfg(target_os = "macos")]
    command.args(["-q", "/dev/null", "/bin/sh", "-c", &command_line]);

    #[cfg(target_os = "linux")]
    command.args(["-q", "-e", "-c", &command_line, "/dev/null"]);

    let mut child = command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("NO_COLOR", "1")
        .env_remove("CRUISE_CONFIG")
        .env_remove("CRUISE_MODEL")
        .env_remove("CRUISE_PLAN_MODEL")
        .env_remove("CRUISE_SDK")
        .env_remove("CRUISE_LANGUAGE_PR")
        .env_remove("CRUISE_LANGUAGE_PLAN")
        .env_remove("CRUISE_CLEANUP_AFTER_PR")
        .env_remove("CRUISE_INTERACTIVE_PLANNING")
        .env_remove("CRUISE_FORCE_EXEC")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("failed to start script: {error}"));

    let mut input = child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("script stdin should be piped"));
    input
        .write_all(b"23??q\r")
        .unwrap_or_else(|error| panic!("failed to send TUI input: {error}"));
    drop(input);

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match child
            .try_wait()
            .unwrap_or_else(|error| panic!("failed to poll script: {error}"))
        {
            Some(_) => break,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                panic!("cruise did not exit after q");
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    }

    child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("failed to collect script output: {error}"))
}

#[test]
fn tui_navigation_and_terminal_lifecycle_work_in_pty() {
    if !script_is_available() {
        eprintln!("skipping TUI PTY E2E: script is unavailable");
        return;
    }

    let tmp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
    let home = tmp.path().join("home");
    for directory in [
        home.clone(),
        home.join("config"),
        home.join("data"),
        home.join("state"),
    ] {
        std::fs::create_dir_all(directory).unwrap_or_else(|error| panic!("{error}"));
    }

    let output = run_tui_in_pty(Path::new(env!("CARGO_BIN_EXE_cruise")), &home);
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.status.success(),
        "cruise failed in PTY: {transcript}"
    );

    for expected in [
        "Detail",
        "SOURCE",
        "PARALLELISM",
        "Keyboard-only",
        "\u{1b}[?1049l",
        "\u{1b}[?25h",
    ] {
        assert!(
            transcript.contains(expected),
            "PTY transcript missing {expected:?}: {transcript}"
        );
    }
}
