use std::{
    fs,
    process::{Command, Output},
};

use tempfile::TempDir;

struct Fixture {
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap_or_else(|error| panic!("create temp dir: {error}"));
        for name in ["cwd", "home", "config", "data", "state"] {
            fs::create_dir(root.path().join(name))
                .unwrap_or_else(|error| panic!("create fixture directory: {error}"));
        }
        Self { root }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cruise-runtime"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CRUISE_") {
                command.env_remove(key);
            }
        }
        command
            .current_dir(self.root.path().join("cwd"))
            .env("HOME", self.root.path().join("home"))
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env_remove("HERDR_ENV")
            .env_remove("HERDR_BIN_PATH")
            .env_remove("HERDR_PANE_ID")
            .args(args)
            .output()
            .unwrap_or_else(|error| panic!("run cruise docs: {error}"))
    }

    fn assert_no_user_data_changes(&self) {
        for name in ["cwd", "home", "config", "data", "state"] {
            let path = self.root.path().join(name);
            let mut entries = fs::read_dir(&path)
                .unwrap_or_else(|error| panic!("read fixture directory {name}: {error}"));
            assert!(
                entries.next().is_none(),
                "docs command changed {}",
                path.display()
            );
        }
    }
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap_or_else(|error| panic!("CLI output is not UTF-8: {error}"))
}

#[test]
fn common_policy_and_framework_ids_are_discoverable_without_user_data_changes() {
    let fixture = Fixture::new();
    let output = fixture.run(&["docs", "e2e"]);
    assert!(output.status.success(), "stderr: {}", text(&output.stderr));
    let stdout = text(&output.stdout);
    assert!(stdout.contains("# UI end-to-end testing policy"));
    assert!(stdout.contains("## Framework policies"));
    for id in [
        "browser",
        "browser-extension",
        "swiftui",
        "android",
        "react-native",
        "electron",
        "tauri",
        "slint",
    ] {
        assert!(stdout.contains(id), "missing ID {id}");
    }
    assert!(
        !stdout.lines().any(|line| line.starts_with("- `exact`")),
        "exact must not be listed as a framework ID"
    );

    let docs_help = fixture.run(&["docs", "--help"]);
    assert!(
        docs_help.status.success(),
        "stderr: {}",
        text(&docs_help.stderr)
    );
    assert!(text(&docs_help.stdout).contains("e2e"));

    let e2e_help = fixture.run(&["docs", "e2e", "--help"]);
    assert!(
        e2e_help.status.success(),
        "stderr: {}",
        text(&e2e_help.stderr)
    );
    assert!(text(&e2e_help.stdout).contains("FRAMEWORK"));

    fixture.assert_no_user_data_changes();
}

#[test]
fn each_framework_id_returns_its_meaningful_policy() {
    let fixture = Fixture::new();
    let policies = [
        ("browser", "Playwright CLI (`playwright-cli`)"),
        ("browser-extension", "--load-extension"),
        ("swiftui", "Xcode MCP"),
        ("android", "Compose UI Test"),
        ("react-native", "Detox + Jest"),
        ("electron", "Playwright Electron API"),
        ("tauri", "tauri-webdriver"),
        ("slint", "i-slint-backend-testing"),
    ];
    for &(id, marker) in &policies {
        let output = fixture.run(&["docs", "e2e", id]);
        assert!(output.status.success(), "{id}: {}", text(&output.stderr));
        let stdout = text(&output.stdout);
        assert!(
            stdout.contains(marker),
            "{id} policy lacks its relevant guidance"
        );
        assert!(!stdout.contains("# UI end-to-end testing policy"));
        assert!(output.stderr.is_empty(), "{id}: {}", text(&output.stderr));
        for &(other_id, other_marker) in &policies {
            if id != other_id {
                assert!(
                    !stdout.contains(other_marker),
                    "{id} output includes {other_id} policy guidance"
                );
            }
        }
    }
    fixture.assert_no_user_data_changes();
}

#[test]
fn unknown_topic_and_framework_are_rejected_with_discovery_guidance() {
    let fixture = Fixture::new();

    let topic = fixture.run(&["docs", "unknown"]);
    assert!(!topic.status.success());
    assert!(text(&topic.stderr).contains("error:"));

    let framework = fixture.run(&["docs", "e2e", "exact"]);
    assert_eq!(framework.status.code(), Some(2));
    assert!(text(&framework.stderr).contains("unknown E2E framework"));
    assert!(text(&framework.stderr).contains("cruise docs e2e"));
    fixture.assert_no_user_data_changes();
}
