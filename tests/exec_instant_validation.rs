use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

fn run(args: &[&str]) -> Output {
    let home = TempDir::new().expect("temporary home");
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
        .env("JCODE_HOME", home.path().join("jcode"))
        .env_remove("JCODE_MODEL")
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env("NO_COLOR", "1")
        .current_dir(home.path())
        .stdin(Stdio::null())
        .args(args)
        .output()
        .expect("run cruise")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn instant_options_require_their_dependencies() {
    for option in [
        vec!["--sdk", "jcode"],
        vec!["--model", "model"],
        vec!["--review"],
        vec!["--verify", "true"],
    ] {
        let mut args = vec!["exec"];
        args.extend(option.iter().copied());
        args.push("task");
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {}", text(&output));
        assert!(
            text(&output).contains("--instant"),
            "{args:?}: {}",
            text(&output)
        );
    }
    let output = run(&["exec", "--instant", "--review-rounds", "3", "task"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(text(&output).contains("--review"), "{}", text(&output));
}

#[test]
fn instant_conflicts_with_config_and_flags() {
    for option in [
        vec!["-c", "workflow.yaml"],
        vec!["-f", "after_pr"],
        vec!["--config", "workflow.yaml"],
        vec!["--flag", "after_pr"],
    ] {
        let mut args = vec!["exec", "--instant"];
        args.extend(option.iter().copied());
        args.push("task");
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {}", text(&output));
        assert!(
            text(&output).contains("--instant") && text(&output).contains("cannot be used with"),
            "{args:?}: {}",
            text(&output)
        );
    }
}

#[test]
fn rejects_invalid_instant_inputs() {
    for args in [
        vec!["exec", "--instant", "--dry-run"],
        vec!["exec", "--instant", "--dry-run", "   "],
        vec![
            "exec",
            "--instant",
            "--dry-run",
            "--review",
            "--review-rounds",
            "0",
            "task",
        ],
        vec!["exec", "--instant", "--dry-run", "--verify", "  ", "task"],
        vec![
            "exec",
            "--instant",
            "--dry-run",
            "--verify",
            "echo {unknown}",
            "task",
        ],
    ] {
        let output = run(&args);
        assert!(!output.status.success(), "{args:?}: {}", text(&output));
        assert_ne!(
            output.status.code(),
            Some(2),
            "input should reach runtime validation: {args:?}: {}",
            text(&output)
        );
    }
}

#[test]
fn sdk_validation_preserves_unknown_and_removed_diagnostics() {
    for (sdk, expected) in [
        ("claude-cli", "unknown"),
        ("seher", "no longer supported"),
        ("pi", "no longer supported"),
    ] {
        let output = run(&["exec", "--instant", "--dry-run", "--sdk", sdk, "task"]);
        let diagnostic = text(&output);
        assert!(!output.status.success(), "{sdk}: {diagnostic}");
        assert!(
            diagnostic.contains(sdk) && diagnostic.contains(expected),
            "{diagnostic}"
        );
        if sdk != "claude-cli" {
            assert!(
                diagnostic.contains("claude"),
                "migration guidance missing: {diagnostic}"
            );
        }
    }
}

#[test]
fn review_retry_budget_respects_boundaries_and_cli_override() {
    for rounds in ["1", "3", "4", "5"] {
        let output = run(&[
            "exec",
            "--instant",
            "--dry-run",
            "--review",
            "--review-rounds",
            rounds,
            "task",
        ]);
        assert!(
            output.status.success(),
            "rounds {rounds}: {}",
            text(&output)
        );
        assert!(text(&output).contains("review"), "{}", text(&output));
    }
    let output = run(&[
        "exec",
        "--instant",
        "--dry-run",
        "--review",
        "--review-rounds",
        "4",
        "--max-retries",
        "3",
        "task",
    ]);
    let diagnostic = text(&output);
    assert!(!output.status.success(), "{diagnostic}");
    assert!(
        diagnostic.contains("review-loop") && diagnostic.contains("max_retries"),
        "{diagnostic}"
    );
}
