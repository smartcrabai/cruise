#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::process::Command;

use tempfile::TempDir;

#[test]
fn cli_plan_entrypoint_continues_with_notification_opt_out_enabled() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("failed to create fixture: {error}"));
    let config = temp.path().join("cruise.yaml");
    std::fs::write(
        &config,
        "command: [echo]\nsteps:\n  check:\n    command: echo check\n",
    )
    .unwrap_or_else(|error| panic!("failed to write fixture config: {error}"));

    let home = temp.path().join("home");
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            let name = name.to_string_lossy();
            if (name.starts_with("CRUISE_") && name != "CRUISE_GUARD_COMMON_DIR")
                || name.starts_with("JCODE_")
                || name.starts_with("OTEL_")
                || name.starts_with("CLAUDE_")
                || name.starts_with("ANTHROPIC_")
            {
                command.env_remove(name.as_ref());
            }
        }
        command
            .current_dir(temp.path())
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("XDG_DATA_HOME", temp.path().join("data"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env("JCODE_HOME", temp.path().join("jcode"))
            .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
            .env_remove("HERDR_ENV");
        command
    };

    let output = command()
        .args(["plan", "--skip-planning", "--config"])
        .arg(&config)
        .arg("test notification opt-out")
        .output()
        .unwrap_or_else(|error| panic!("failed to run cruise plan: {error}"));
    assert!(
        output.status.success(),
        "plan entry point failed with notification opt-out: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let output = command()
        .args(["list", "--json"])
        .output()
        .unwrap_or_else(|error| panic!("failed to list cruise sessions: {error}"));
    assert!(
        output.status.success(),
        "list entry point failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let sessions: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("list --json output was not valid JSON: {error}"));
    assert_eq!(
        sessions.len(),
        1,
        "expected one planned session: {sessions:?}"
    );
    assert_eq!(sessions[0]["phase"], "Planned", "{sessions:?}");
}
