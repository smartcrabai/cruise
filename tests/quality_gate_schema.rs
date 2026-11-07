//! Published editor schema contracts for prompt-step quality gates.
//! Runtime validation remains authoritative for command contents and timeout grammar.
use serde_json::{Value, json};
use std::sync::OnceLock;

fn validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let schema: Value = serde_json::from_str(include_str!("../cruise-schema.json"))
            .unwrap_or_else(|error| panic!("parse published schema: {error}"));
        jsonschema::validator_for(&schema)
            .unwrap_or_else(|error| panic!("compile published schema: {error}"))
    })
}

fn check(document: &Value, expected: bool) {
    let errors: Vec<_> = validator()
        .iter_errors(document)
        .map(|e| e.to_string())
        .collect();
    assert_eq!(
        errors.is_empty(),
        expected,
        "document: {document}\nerrors: {errors:?}"
    );
}

#[test]
fn schema_accepts_prompt_forms_and_group_internal_gates() {
    for field in ["prompt", "prompt_file"] {
        let mut step = json!({"quality_gates": [
            {"command": "cargo test"},
            {"command": "cargo clippy", "timeout": "10m"}
        ]});
        step[field] = json!("example");
        check(&json!({"steps": {"main": step}}), true);
    }
    check(
        &json!({"groups": {"review": {"steps": {"inspect": {
        "prompt": "inspect", "quality_gates": [{"command": "true"}]
    }}}}, "steps": {"run": {"group": "review"}}}),
        true,
    );
    check(&json!({"steps": {"main": {"prompt": "no gates"}}}), true);
}

#[test]
fn schema_rejects_invalid_gate_arrays_and_items() {
    for gates in [
        json!([]),
        json!(null),
        json!({"command": "true"}),
        json!([null]),
        json!(["true"]),
        json!([{}]),
        json!([{"timeout": "10m"}]),
        json!([{"command": 42}]),
        json!([{"command": "true", "timeout": 10}]),
        json!([{"command": "true", "unexpected": true}]),
    ] {
        check(
            &json!({"steps": {"main": {"prompt": "work", "quality_gates": gates}}}),
            false,
        );
    }
}

#[test]
fn schema_requires_prompt_and_excludes_other_step_kinds() {
    for step in [
        json!({}),
        json!({"command": "true"}),
        json!({"option": []}),
        json!({"parallel": {"child": {"prompt": "child"}}}),
        json!({"group": "review"}),
        json!({"workflow_call": "callee.yaml"}),
        json!({"prompt": "work", "command": "true"}),
        json!({"prompt": "work", "option": []}),
        json!({"prompt": "work", "parallel": {"child": {"prompt": "child"}}}),
        json!({"prompt": "work", "group": "review"}),
        json!({"prompt": "work", "workflow_call": "callee.yaml"}),
    ] {
        let mut step = step;
        step["quality_gates"] = json!([{"command": "true"}]);
        check(&json!({"steps": {"main": step}}), false);
    }
}
