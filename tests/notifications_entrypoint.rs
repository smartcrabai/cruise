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

    let output = Command::new(env!("CARGO_BIN_EXE_cruise"))
        .args([
            "plan",
            "--dry-run",
            "--config",
            config
                .to_str()
                .unwrap_or_else(|| panic!("fixture path is not UTF-8")),
            "test notification opt-out",
        ])
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .output()
        .unwrap_or_else(|error| panic!("failed to run cruise plan: {error}"));

    assert!(
        output.status.success(),
        "plan entry point failed with notification opt-out: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Would plan"),
        "dry-run should still report the planned action"
    );
}
