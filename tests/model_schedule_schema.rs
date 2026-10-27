//! Editor schema contracts for candidate schedules. Runtime parsing remains authoritative.
use serde_json::{Value, json};

fn validator() -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(include_str!("../cruise-schema.json"))
        .unwrap_or_else(|error| panic!("parse editor schema: {error}"));
    jsonschema::validator_for(&schema)
        .unwrap_or_else(|error| panic!("compile editor schema: {error}"))
}

#[test]
fn candidate_schema_accepts_schedules_without_duplicating_runtime_grammar() {
    let validator = validator();
    for field in ["model", "plan_model"] {
        for candidate in [
            json!({"model": "provider/main", "disabled_during": ["00:00-24:00"]}),
            json!({"sdk": "claude", "model": "main", "disabled_during": ["Mon-Fri 01:00-04:00 +09:00"]}),
            json!({"model": "main", "disabled_during": ["invalid runtime grammar"]}),
        ] {
            let mut workflow = json!({"steps": {"main": {"prompt": "hi"}}});
            workflow[field] = json!([candidate]);
            assert!(validator.is_valid(&workflow), "rejected {workflow}");
        }
    }
}

#[test]
fn candidate_schema_rejects_empty_untyped_and_null_schedules() {
    let validator = validator();
    for field in ["model", "plan_model"] {
        for candidate in [
            json!({"model": "main"}),
            json!({"model": "main", "disabled_during": []}),
            json!({"sdk": "claude", "model": "main", "disabled_during": []}),
            json!({"model": "main", "disabled_during": [""]}),
            json!({"model": "main", "disabled_during": ["   "]}),
            json!({"model": "main", "disabled_during": [null]}),
            json!({"model": "main", "disabled_during": "00:00-24:00"}),
            json!({"sdk": null, "model": "main", "disabled_during": ["00:00-24:00"]}),
        ] {
            let mut workflow = json!({"steps": {"main": {"prompt": "hi"}}});
            workflow[field] = json!([candidate]);
            assert!(!validator.is_valid(&workflow), "accepted {workflow}");
        }
    }
}

#[test]
fn subagent_schema_still_rejects_schedules_and_missing_sdk_at_both_levels() {
    let validator = validator();
    for step_level in [false, true] {
        for entry in [
            json!({"model": "small", "disabled_during": ["00:00-24:00"]}),
            json!({"sdk": "claude", "model": "small", "disabled_during": ["00:00-24:00"]}),
            json!({"model": "small"}),
        ] {
            let mut workflow = json!({"steps": {"main": {"prompt": "hi"}}});
            if step_level {
                workflow["steps"]["main"]["subagent_model"] = entry;
            } else {
                workflow["subagent_model"] = entry;
            }
            assert!(!validator.is_valid(&workflow), "accepted {workflow}");
        }
    }
}
