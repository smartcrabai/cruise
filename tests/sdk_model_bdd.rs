//! Production configuration/compilation contracts for Jcode model fallback strings.
#![expect(clippy::unwrap_used, reason = "isolated executable contract fixtures")]

use cruise::config::{WorkflowConfig, validate_config};
use cruise::workflow;
use cucumber::{World as _, given, then, when};
use serde_json::{Value, json};

#[derive(Default, cucumber::World)]
struct World {
    yaml: Option<String>,
    first: Option<String>,
    fallback: Option<String>,
    execution: Option<Value>,
    planning: Option<Value>,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World").finish_non_exhaustive()
    }
}

#[given(expr = "a Jcode workflow with model candidates {string} then {string}")]
fn workflow(w: &mut World, first: String, fallback: String) {
    w.yaml = Some(format!(
        "sdk: jcode\nmodel: ['{first}', '{fallback}']\nplan_model: ['{first}', '{fallback}']\nsteps:\n  main:\n    prompt: inspect\n"
    ));
    w.first = Some(first);
    w.fallback = Some(fallback);
}

#[when("production validation and compilation run")]
fn compile(w: &mut World) {
    let yaml = w
        .yaml
        .as_deref()
        .unwrap_or_else(|| panic!("workflow fixture missing"));
    let config =
        WorkflowConfig::from_yaml(yaml).unwrap_or_else(|error| panic!("parse workflow: {error}"));
    validate_config(&config).unwrap_or_else(|error| panic!("validate workflow: {error}"));
    let compiled =
        workflow::compile(config).unwrap_or_else(|error| panic!("compile workflow: {error}"));
    w.execution = Some(
        serde_json::to_value(compiled.model)
            .unwrap_or_else(|error| panic!("serialize execution candidates: {error}")),
    );
    w.planning = Some(
        serde_json::to_value(compiled.plan_model)
            .unwrap_or_else(|error| panic!("serialize planning candidates: {error}")),
    );
}

#[then("the compiled candidates preserve that order")]
fn ordered(w: &mut World) {
    let expected = json!([
        w.first
            .as_deref()
            .unwrap_or_else(|| panic!("first model missing")),
        w.fallback
            .as_deref()
            .unwrap_or_else(|| panic!("fallback model missing"))
    ]);
    assert_eq!(
        w.execution.as_ref(),
        Some(&expected),
        "execution fallback order changed"
    );
    assert_eq!(
        w.planning.as_ref(),
        Some(&expected),
        "planning fallback order changed"
    );
}

#[then("a deliberately wrong Jcode candidate order is rejected")]
fn deliberately_wrong(w: &mut World) {
    let expected = json!([
        w.fallback
            .as_deref()
            .unwrap_or_else(|| panic!("fallback model missing")),
        w.first
            .as_deref()
            .unwrap_or_else(|| panic!("first model missing"))
    ]);
    assert_eq!(
        w.execution.as_ref(),
        Some(&expected),
        "deliberately wrong Jcode model candidate order"
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
            "/tests/features/sdk_model.feature"
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
        "no SDK model scenarios executed"
    );
}
