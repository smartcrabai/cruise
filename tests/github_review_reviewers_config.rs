//! Public contracts for github-review reviewer scopes and the editor schema.
use cruise::config::{WorkflowConfig, validate_config};
use cruise::error::CruiseError;
use serde_json::{Value, json};

fn workflow(review: &str) -> String {
    format!(
        "steps:\n  main:\n    prompt: implement\nafter-pr:\n  review:\n    github-review: {review}\n    prompt: address feedback\n    timeout: 30m\n"
    )
}

fn parse(yaml: &str) -> WorkflowConfig {
    WorkflowConfig::from_yaml(yaml).unwrap_or_else(|error| panic!("parse {yaml}: {error}"))
}

#[test]
fn reviewer_scopes_accept_all_and_preserve_legacy_bots() {
    for review in [
        "{reviewers: all}",
        "{reviewers: all, bots: []}",
        "{reviewers: bots, bots: [reviewbot]}",
        "{bots: [reviewbot]}",
    ] {
        let yaml = workflow(review);
        let config = parse(&yaml);
        validate_config(&config).unwrap_or_else(|error| panic!("{review}: {error}"));
        let serialized = serde_yaml::to_string(&config)
            .unwrap_or_else(|error| panic!("serialize config: {error}"));
        let restored = parse(&serialized);
        validate_config(&restored).unwrap_or_else(|error| panic!("roundtrip {review}: {error}"));
        assert_eq!(
            serde_json::to_value(config)
                .unwrap_or_else(|error| panic!("serialize config as JSON: {error}")),
            serde_json::to_value(restored)
                .unwrap_or_else(|error| panic!("serialize restored config as JSON: {error}"))
        );
    }
}

#[test]
fn reviewer_scope_validation_rejects_incompatible_bots() {
    for (review, expected) in [
        ("{reviewers: all, bots: [x]}", "reviewers: all"),
        ("{bots: []}", "github-review bots must not be empty"),
        (
            "{reviewers: bots, bots: []}",
            "github-review bots must not be empty",
        ),
        ("{}", "github-review bots must not be empty"),
    ] {
        let Err(error) = validate_config(&parse(&workflow(review))) else {
            panic!("{review}: expected invalid reviewer configuration");
        };
        match error {
            CruiseError::InvalidStepConfig(message) => {
                assert!(message.contains(expected), "{review}: {message}");
            }
            other => panic!("{review}: expected InvalidStepConfig, got {other:?}"),
        }
    }
}

#[test]
fn unknown_reviewer_scope_is_a_deserialization_error() {
    assert!(WorkflowConfig::from_yaml(&workflow("{reviewers: any}")).is_err());
}

#[test]
fn schema_supports_both_reviewer_scopes_and_legacy_bots() {
    let schema: Value = serde_json::from_str(include_str!("../cruise-schema.json"))
        .unwrap_or_else(|error| panic!("parse workflow schema: {error}"));
    let validator = jsonschema::validator_for(&schema)
        .unwrap_or_else(|error| panic!("compile schema: {error}"));
    let fixture = |review: Value| {
        json!({
            "steps": {"main": {"prompt": "implement"}},
            "after-pr": {"review": {"github-review": review, "prompt": "address feedback", "timeout": "30m"}}
        })
    };
    for review in [
        json!({"reviewers": "all"}),
        json!({"reviewers": "all", "bots": []}),
        json!({"reviewers": "bots", "bots": ["reviewbot"]}),
        json!({"bots": ["reviewbot"]}),
    ] {
        let document = fixture(review);
        assert!(
            validator.is_valid(&document),
            "schema rejected {document}: {:?}",
            validator.iter_errors(&document).collect::<Vec<_>>()
        );
    }
    for review in [
        json!({"reviewers": "any"}),
        json!({"reviewers": 7}),
        json!({"reviewers": null}),
        json!({"reviewers": "all", "unknown": true}),
    ] {
        let document = fixture(review);
        assert!(!validator.is_valid(&document), "schema accepted {document}");
    }
}
