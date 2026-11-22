use std::process::Command;

use tempfile::TempDir;

#[test]
fn exec_dry_run_accepts_computer_use_with_jcode() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let config = temp.path().join("workflow.yaml");
    std::fs::write(
        &config,
        "sdk: jcode\nmodel: claude/claude-opus-4-5\ncomputer_use: true\nsteps:\n  inspect:\n    prompt: Inspect the repository.\n",
    )
    .unwrap_or_else(|error| panic!("write workflow config: {error}"));

    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CRUISE_") {
            command.env_remove(name);
        }
    }
    let output = command
        .env("CRUISE_UI_LANGUAGE", "en")
        .args(["exec", "--dry-run", "--config"])
        .arg(&config)
        .output()
        .unwrap_or_else(|error| panic!("run cruise exec dry-run: {error}"));

    assert!(
        output.status.success(),
        "jcode should accept computer_use:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
