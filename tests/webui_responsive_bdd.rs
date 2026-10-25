//! Executable Gherkin contracts using production Rust HTTP + Chromium, not DOM mocks.
use cucumber::{World as _, given, then, when};
use std::process::{Command, Output};

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
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/webui/option-dialog.mjs")
    } else {
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/webui/browser.mjs")
    };
    let output = Command::new("node")
        .arg(script)
        .args([contract, world.width.to_string(), world.height.to_string()])
        .env("WEBUI_BDD_BIN", env!("CARGO_BIN_EXE_cruise-runtime"))
        // Cargo's dylib paths are for Rust executables, not Chromium helper processes.
        .env_remove("DYLD_LIBRARY_PATH")
        .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
        .env_remove("LD_LIBRARY_PATH")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap_or_else(|error| panic!("Node/browser prerequisites unavailable: {error}"));
    world.output = Some(output);
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

#[tokio::main]
async fn main() {
    let feature = std::env::var("WEBUI_BDD_FEATURE").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/webui_responsive.feature"
        )
        .to_owned()
    });
    ResponsiveWorld::cucumber()
        .max_concurrent_scenarios(1)
        .fail_on_skipped()
        .run_and_exit(feature)
        .await;
}
