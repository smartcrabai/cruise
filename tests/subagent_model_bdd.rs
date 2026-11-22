//! Jcode-only subagent model validation and compilation contracts.
#![expect(clippy::unwrap_used, reason = "isolated configuration fixtures")]

use cruise::config::{WorkflowConfig, validate_config};
use cruise::workflow;
use cucumber::{World as _, given, then, when};
use serde_json::Value;

#[derive(Default, cucumber::World)]
struct World {
    yaml: Option<String>,
    config: Option<Value>,
    error: Option<String>,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World").finish_non_exhaustive()
    }
}

#[given(expr = "a Jcode subagent workflow with global model {string} and step model {string}")]
fn precedence_fixture(w: &mut World, global: String, local: String) {
    w.yaml = Some(format!(
        "sdk: jcode\nmodel: claude/claude-opus-4-5\nsubagent_model: '{global}'\nsteps:\n  answer:\n    prompt: inspect\n    subagent_model: '{local}'\n"
    ));
}

#[given(expr = "a Jcode workflow with subagent value {string}")]
fn invalid_fixture(w: &mut World, value: String) {
    let value = if value == "blank" {
        "   ".to_owned()
    } else {
        value
    };
    w.yaml = Some(format!(
        "sdk: jcode\nmodel: claude/claude-opus-4-5\nsubagent_model: '{value}'\nsteps:\n  answer:\n    prompt: inspect\n"
    ));
}

#[given(expr = "a command workflow with subagent value {string}")]
fn command_fixture(w: &mut World, value: String) {
    w.yaml = Some(format!(
        "command: [echo]\nsubagent_model: '{value}'\nsteps:\n  answer:\n    prompt: inspect\n"
    ));
}

#[when("production validation and compilation run")]
fn validate_and_compile(w: &mut World) {
    let yaml = w
        .yaml
        .as_deref()
        .unwrap_or_else(|| panic!("workflow fixture missing"));
    let result = WorkflowConfig::from_yaml(yaml)
        .map_err(|error| error.to_string())
        .and_then(|config| {
            validate_config(&config).map_err(|error| error.to_string())?;
            let _compiled = workflow::compile(config.clone()).map_err(|error| error.to_string())?;
            serde_json::to_value(config).map_err(|error| error.to_string())
        });
    match result {
        Ok(config) => w.config = Some(config),
        Err(error) => w.error = Some(error),
    }
}

#[then("the step subagent model takes precedence")]
fn step_precedence(w: &mut World) {
    let config = w
        .config
        .as_ref()
        .unwrap_or_else(|| panic!("compiled config missing"));
    assert_eq!(config["subagent_model"], "openai/global");
    assert_eq!(config["steps"]["answer"]["subagent_model"], "openai/worker");
}

#[then("subagent validation rejects it before prompt dispatch")]
fn rejected(w: &mut World) {
    let error = w
        .error
        .as_deref()
        .unwrap_or_else(|| panic!("invalid workflow was accepted"));
    assert!(
        error.contains("subagent_model") || error.contains("command"),
        "{error}"
    );
}

#[then(expr = "the step subagent model is {string}")]
fn deliberately_assert_model(w: &mut World, expected: String) {
    let config = w
        .config
        .as_ref()
        .unwrap_or_else(|| panic!("compiled config missing"));
    assert_eq!(
        config["steps"]["answer"]["subagent_model"], expected,
        "subagent step model assertion mismatch"
    );
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    use cucumber::writer::Stats as _;
    let writer = World::cucumber()
        .max_concurrent_scenarios(1)
        .fail_on_skipped()
        .run(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/subagent_model.feature"
        ))
        .await;
    assert!(
        !writer.execution_has_failed(),
        "{} step failed, {} parsing errors, {} hook errors",
        writer.failed_steps(),
        writer.parsing_errors(),
        writer.hook_errors()
    );
    assert!(
        writer.passed_steps() + writer.failed_steps() > 0,
        "no subagent model scenarios executed"
    );
}
