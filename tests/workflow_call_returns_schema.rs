//! Published editor schema contracts for workflow-call return labels.
use serde_json::{Value, json};

fn validator() -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(include_str!("../cruise-schema.json"))
        .unwrap_or_else(|error| panic!("parse editor schema: {error}"));
    jsonschema::validator_for(&schema)
        .unwrap_or_else(|error| panic!("compile editor schema: {error}"))
}

#[test]
fn schema_accepts_return_label_declarations_and_routes() {
    let validator = validator();
    for workflow in [
        json!({"returns": ["approved", "needs_fix", "_retry-2"], "steps": {
            "finish": {"command": "true", "return": "approved"},
            "decide": {"option": [
                {"selector": "Fix", "return": "needs_fix"},
                {"selector": "Retry", "return": "_retry-2"}
            ]}
        }}),
        json!({"returns": [], "steps": {"gate": {
            "workflow_call": "./callee.yaml", "on_return": {"approved": "publish", "needs_fix": "fix"}
        }}}),
        json!({"steps": {"gate": {"workflow_call": "./callee.yaml", "on_return": null}}}),
        json!({"steps": {"finish": {"command": "true", "return": "ok", "skip": true}}}),
    ] {
        assert!(validator.is_valid(&workflow), "rejected {workflow}");
    }
}

#[test]
fn schema_rejects_invalid_return_declarations_and_values() {
    let validator = validator();
    for returns in [
        json!(null),
        json!("ok"),
        json!([1]),
        json!(["ok", "ok"]),
        json!([""]),
        json!(["bad label"]),
        json!(["1bad"]),
    ] {
        let workflow = json!({"returns": returns, "steps": {"main": {"command": "true"}}});
        assert!(!validator.is_valid(&workflow), "accepted {workflow}");
    }
    for value in [json!(42), json!(true), json!([]), json!({})] {
        for step in [
            json!({"command": "true", "return": value}),
            json!({"option": [{"selector": "OK", "return": value}]}),
            json!({"workflow_call": "./callee.yaml", "on_return": {"ok": value}}),
        ] {
            let workflow = json!({"steps": {"main": step}});
            assert!(!validator.is_valid(&workflow), "accepted {workflow}");
        }
    }
    for value in ["", "bad label", "1bad", "bad/label"] {
        for step in [
            json!({"command": "true", "return": value}),
            json!({"option": [{"selector": "OK", "return": value}]}),
        ] {
            let workflow = json!({"steps": {"main": step}});
            assert!(!validator.is_valid(&workflow), "accepted {workflow}");
        }
    }
    for routes in [json!("ok"), json!([]), json!(42)] {
        let workflow =
            json!({"steps": {"gate": {"workflow_call": "./callee.yaml", "on_return": routes}}});
        assert!(!validator.is_valid(&workflow), "accepted {workflow}");
    }
}

#[test]
fn schema_rejects_conflicting_return_routes() {
    let validator = validator();
    for step in [
        json!({"command": "true", "on_return": {"ok": "done"}}),
        json!({"command": "true", "return": "ok", "next": "done"}),
        json!({"option": [{"selector": "OK", "return": "ok", "next": "done"}]}),
        json!({"command": "true", "return": "ok", "if": {"file-changed": "done"}}),
        json!({"workflow_call": "./callee.yaml", "return": "ok"}),
        json!({"group": "build", "return": "ok"}),
        json!({"parallel": {"child": {"command": "true"}}, "return": "ok"}),
    ] {
        let workflow = json!({"steps": {"main": step}});
        assert!(!validator.is_valid(&workflow), "accepted {workflow}");
    }
}
