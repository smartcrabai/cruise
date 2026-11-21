//! Executable Gherkin contracts using production Rust HTTP + Chromium, not DOM mocks.
use cucumber::{World as _, given, then, when};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn runtime_binary() -> PathBuf {
    // Cargo runs integration tests from the package root. A cached test
    // executable can still contain the absolute paths of another worktree.
    let built = Path::new(env!("CARGO_BIN_EXE_cruise-runtime"));
    match built.strip_prefix(env!("CARGO_MANIFEST_DIR")) {
        Ok(relative) => std::env::current_dir()
            .unwrap_or_else(|_| panic!("Cargo package root unavailable"))
            .join(relative),
        Err(_) => built.to_owned(), // External Cargo target directory.
    }
}

#[derive(Debug, Default, cucumber::World)]
struct ResponsiveWorld {
    width: u32,
    height: u32,
    output: Option<Output>,
}

#[given(expr = "a production WebUI at {int} by {int} pixels")]
fn viewport(world: &mut ResponsiveWorld, width: u32, height: u32) {
    world.width = width;
    world.height = height;
}

#[when(expr = "I exercise the {word} browser contract")]
fn exercise(world: &mut ResponsiveWorld, contract: String) {
    let script = if contract == "option" {
        "tests/webui/option-dialog.mjs"
    } else {
        "tests/webui/browser.mjs"
    };
    world.output = Some(browser_output(
        Command::new("node")
            .arg(script)
            .args([contract, world.width.to_string(), world.height.to_string()])
            .env("CRUISE_UI_LANGUAGE", "en"),
    ));
}

fn browser_output(command: &mut Command) -> Output {
    command
        .env("WEBUI_BDD_BIN", runtime_binary())
        // Cargo's dylib paths are for Rust executables, not Chromium helper processes.
        .env_remove("DYLD_LIBRARY_PATH")
        .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
        .env_remove("LD_LIBRARY_PATH")
        .output()
        .unwrap_or_else(|error| panic!("Node/browser prerequisites unavailable: {error}"))
}

#[then("all observable responsive assertions pass")]
fn passed(world: &mut ResponsiveWorld) {
    let output = world
        .output
        .as_ref()
        .unwrap_or_else(|| panic!("browser not run"));
    assert!(
        output.status.success(),
        "browser failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("WEBUI_BROWSER_CONTRACT_PASSED"),
        "bridge must positively acknowledge completion"
    );
}

#[when(
    expr = "I run a real command workflow in Chromium and exercise the {word} shortcut on its Planning question"
)]
fn planning_shortcut(world: &mut ResponsiveWorld, modifier: String) {
    planning_browser(world, &modifier);
}

#[when(
    "I run a real command workflow in Chromium and exercise Send answer recovery on its Planning question"
)]
fn planning_send_recovery(world: &mut ResponsiveWorld) {
    planning_browser(world, "send-recovery");
}

fn planning_browser(world: &mut ResponsiveWorld, mode: &str) {
    world.output = Some(browser_output(
        Command::new("node")
            .arg("tests/webui/ask-keyboard.mjs")
            .arg(mode),
    ));
}

#[then("the controls re-enable and retry the original answer to the daemon")]
fn planning_recovered(world: &mut ResponsiveWorld) {
    planning_evidence(world, "ASK_SEND_RECOVERY_PASSED");
}

fn planning_evidence(world: &ResponsiveWorld, marker: &str) {
    let output = world
        .output
        .as_ref()
        .unwrap_or_else(|| panic!("Planning browser not run"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.lines().any(|line| line == marker),
        "Planning browser failed or missing {marker}:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[then(
    "reload preserves the pending question, plain Enter inserts a newline, and empty answers are blocked"
)]
fn planning_native_behavior(world: &mut ResponsiveWorld) {
    planning_evidence(world, "ASK_KEYBOARD_NATIVE_PASSED");
}

#[then("the Tell editor remains independent of the Planning shortcut")]
fn planning_editor_isolation(world: &mut ResponsiveWorld) {
    planning_evidence(world, "ASK_KEYBOARD_EDITOR_PASSED");
}

#[then(
    "the intercepted answer POST fails without replay and only the explicit retry reaches the daemon with the original request ID and text and clears the question"
)]
fn planning_submission(world: &mut ResponsiveWorld) {
    planning_evidence(world, "ASK_KEYBOARD_SUBMISSION_PASSED");
}

#[tokio::main]
async fn main() {
    let override_feature = std::env::var("WEBUI_BDD_FEATURE").ok();
    let features = match override_feature.as_deref() {
        Some(feature) => vec![feature],
        None => vec![
            "tests/features/webui_responsive.feature",
            "tests/features/webui_ask_keyboard.feature",
        ],
    };
    for feature in features {
        ResponsiveWorld::cucumber()
            .max_concurrent_scenarios(1)
            .fail_on_skipped()
            .run_and_exit(feature)
            .await;
    }
}
