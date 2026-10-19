use std::process::{Command, Output};

use tempfile::TempDir;

fn cruise(home: &std::path::Path, cwd: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CRUISE_") {
            command.env_remove(name);
        }
    }
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("XDG_STATE_HOME", home.join("state"))
        .env("GIT_CONFIG_COUNT", "0")
        .current_dir(cwd);
    command
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn project_with_workflow(defaults: &str) -> (TempDir, TempDir) {
    let home = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let project = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    std::fs::create_dir_all(project.path().join(".cruise")).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(project.path().join(".cruise/default.yaml"), defaults)
        .unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(
        project.path().join("workflow.yaml"),
        "command: [echo]\nsteps:\n  main-step:\n    command: echo main\n",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    (home, project)
}

#[test]
fn exec_dry_run_lists_pre_steps_before_main_steps() {
    let (home, project) =
        project_with_workflow("pre_steps:\n  setup:\n    command: mise trust || true\n");
    let output = cruise(home.path(), project.path())
        .args(["exec", "--dry-run", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|e| panic!("run cruise: {e}"));
    let terminal = text(&output);
    assert!(output.status.success(), "{terminal}");
    let pre = terminal
        .find("pre/setup")
        .unwrap_or_else(|| panic!("pre/setup missing:\n{terminal}"));
    let main = terminal
        .find("main-step")
        .unwrap_or_else(|| panic!("main-step missing:\n{terminal}"));
    assert!(pre < main, "pre step must come first:\n{terminal}");
}

#[test]
fn defaults_file_with_workflow_only_key_fails_with_path_and_key() {
    let (home, project) = project_with_workflow("steps:\n  x:\n    command: echo\n");
    let output = cruise(home.path(), project.path())
        .args(["exec", "--dry-run", "--config", "workflow.yaml"])
        .output()
        .unwrap_or_else(|e| panic!("run cruise: {e}"));
    let terminal = text(&output);
    assert!(!output.status.success(), "{terminal}");
    assert!(terminal.contains("default.yaml"), "{terminal}");
    assert!(terminal.contains("steps"), "{terminal}");
}

#[test]
fn workflow_eject_default_to_project_is_rejected() {
    let home = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let project = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    let output = cruise(home.path(), project.path())
        .args(["workflow", "eject", "default", "--to", "project"])
        .output()
        .unwrap_or_else(|e| panic!("run cruise: {e}"));
    assert!(!output.status.success(), "{}", text(&output));
    assert!(!project.path().join(".cruise/default.yaml").exists());
}
