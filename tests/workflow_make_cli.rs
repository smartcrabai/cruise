#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

fn cruise(home: &Path, project: &Path, args: &[&str]) -> std::io::Result<Output> {
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
        .env("JCODE_HOME", home.join("jcode"))
        .current_dir(project)
        .stdin(Stdio::null())
        .args(args)
        .output()
}

fn output_text(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn workflow_help_lists_make_and_make_help_describes_arguments() -> std::io::Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;
    let output = cruise(home.path(), project.path(), &["workflow", "--help"])?;
    assert!(output.status.success(), "{}", output_text(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("make"),
        "{}",
        output_text(&output)
    );

    let output = cruise(home.path(), project.path(), &["workflow", "make", "--help"])?;
    assert!(output.status.success(), "{}", output_text(&output));
    let help = String::from_utf8_lossy(&output.stdout);
    for expected in ["--name", "--user", "--config", "--from", "DESCRIPTION"] {
        assert!(
            help.contains(expected),
            "missing {expected}: {}",
            output_text(&output)
        );
    }
    Ok(())
}

#[test]
fn make_requires_a_valid_name_before_terminal_check() -> std::io::Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;
    for (args, expected) in [
        (vec!["workflow", "make"], "--name"),
        (
            vec!["workflow", "make", "--name", "../invalid"],
            "invalid workflow name",
        ),
    ] {
        let output = cruise(home.path(), project.path(), &args)?;
        assert!(!output.status.success(), "{}", output_text(&output));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{}",
            output_text(&output)
        );
        assert!(!project.path().join(".cruise").exists());
        assert!(!home.path().join("data/cruise/workflow-make").exists());
    }
    Ok(())
}

#[test]
fn make_rejects_non_terminal_without_side_effects_or_backend_call() -> std::io::Result<()> {
    let home = TempDir::new()?;
    let project = TempDir::new()?;
    let marker = home.path().join("backend-called");
    let backend = home.path().join("backend");
    std::fs::write(
        &backend,
        format!(
            "#!/bin/sh\ntouch '{}'\necho BACKEND_OUTPUT\n",
            marker.display()
        ),
    )?;
    std::fs::set_permissions(&backend, std::fs::Permissions::from_mode(0o755))?;
    let config = home.path().join("backend.yaml");
    std::fs::write(
        &config,
        format!(
            "command: ['{}']\nsteps:\n  s:\n    command: echo\n",
            backend.display()
        ),
    )?;

    for use_config in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
        for (name, _) in std::env::vars_os() {
            if name.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(name);
            }
        }
        command
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("XDG_STATE_HOME", home.path().join("state"))
            .current_dir(project.path())
            .stdin(Stdio::piped())
            .args(["workflow", "make", "--name", "ci"]);
        if use_config {
            command.env("CRUISE_CONFIG", &config);
        }
        let output = command.output()?;
        assert!(!output.status.success(), "{}", output_text(&output));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("interactive terminal"),
            "{}",
            output_text(&output)
        );
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("BACKEND_OUTPUT"),
            "{}",
            output_text(&output)
        );
        assert!(!marker.exists(), "backend was invoked");
        assert!(!project.path().join(".cruise").exists());
        assert!(!home.path().join("data/cruise/workflow-make").exists());
    }
    Ok(())
}
