use std::process::Command;

use tempfile::TempDir;

#[test]
fn exec_dry_run_rejects_computer_use_on_the_claude_backend() {
    let temp = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let config = temp.path().join("workflow.yaml");
    std::fs::write(
        &config,
        "sdk: claude\ncomputer_use: true\nsteps:\n  inspect:\n    prompt: Inspect the repository.\n",
    )
    .unwrap_or_else(|error| panic!("write workflow config: {error}"));

    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CRUISE_") {
            command.env_remove(name);
        }
    }
    let output = command
        .args(["exec", "--dry-run", "--config"])
        .arg(&config)
        .output()
        .unwrap_or_else(|error| panic!("run cruise exec dry-run: {error}"));
    let terminal = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        !output.status.success(),
        "computer_use=true on sdk: claude must fail during config validation, but exec --dry-run succeeded:\n{terminal}"
    );
    let diagnostic = terminal.to_lowercase();
    assert!(
        diagnostic.contains("computer_use") && diagnostic.contains("claude"),
        "the config error must identify computer_use and the unsupported claude backend, got:\n{terminal}"
    );
}
