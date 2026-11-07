#![cfg(unix)]

use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

fn dry_run(root: &TempDir, args: &[&str], overrides: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CRUISE_") {
            command.env_remove(name);
        }
    }
    command
        .args(["exec", "--instant", "--dry-run"])
        .args(args)
        .current_dir(root.path())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("NO_COLOR", "1")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env_remove("HERDR_ENV")
        .envs(overrides.iter().copied())
        .stdin(Stdio::null())
        .output()
        .expect("start cruise")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn defaults_prepend_command_step_to_instant() {
    let root = TempDir::new().unwrap();
    let dir = root.path().join("config/cruise");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("default.yaml"),
        "pre_steps:\n  setup:\n    command: echo setup\n",
    )
    .unwrap();
    let result = dry_run(&root, &["task"], &[]);
    let output = text(&result);
    assert!(result.status.success(), "{output}");
    let steps = output.split("steps:").last().unwrap();
    assert!(
        steps.find("setup").unwrap() < steps.find("implement").unwrap(),
        "{output}"
    );
}

#[test]
fn review_and_verify_steps_follow_declared_order() {
    let root = TempDir::new().unwrap();
    let result = dry_run(
        &root,
        &[
            "--review",
            "--verify",
            "echo first",
            "--verify",
            "echo second",
            "task",
        ],
        &[],
    );
    let output = text(&result);
    assert!(result.status.success(), "{output}");
    let steps = output.split("steps:").last().expect("steps section");
    let positions: Vec<_> = ["implement", "review-pass", "verify-1", "verify-2"]
        .iter()
        .map(|name| {
            steps
                .find(name)
                .unwrap_or_else(|| panic!("missing {name}: {output}"))
        })
        .collect();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "{output}"
    );
    assert!(
        output.contains("review-loop") && output.contains("review-pass/review"),
        "{output}"
    );
    assert!(output.contains("max_retries: 2"), "{output}");
}

#[test]
fn review_rounds_above_default_budget_are_valid() {
    let root = TempDir::new().unwrap();
    let result = dry_run(&root, &["--review", "--review-rounds", "5", "task"], &[]);
    let output = text(&result);
    assert!(result.status.success(), "{output}");
    assert!(output.contains("max_retries: 5"), "{output}");
}

#[test]
fn environment_model_overrides_instant_model() {
    let root = TempDir::new().unwrap();
    let result = dry_run(
        &root,
        &["--model", "requested/model", "task"],
        &[("CRUISE_MODEL", "environment/model")],
    );
    let output = text(&result);
    assert!(result.status.success(), "{output}");
    assert!(output.contains("environment/model"), "{output}");
    assert!(!output.contains("requested/model"), "{output}");
}

#[test]
fn task_with_variable_like_text_is_not_interpreted_as_workflow_yaml() {
    let root = TempDir::new().unwrap();
    let result = dry_run(&root, &["fix {weird} a: b"], &[]);
    let output = text(&result);
    assert!(result.status.success(), "{output}");
    assert!(output.contains("implement"), "{output}");
}
