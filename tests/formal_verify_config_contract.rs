//! Planned formal verification configuration contracts, exercised through public YAML APIs.
use cruise::config::{WorkflowConfig, validate_config};
use cruise::workflow_defaults::{WorkflowConfigResolution, WorkflowDefaults};
use serde_json::{Value, json};
use tempfile::TempDir;

fn workflow(setting: &str) -> String {
    format!("command: [echo]\n{setting}steps:\n  run:\n    prompt: review\n")
}

fn parse(setting: &str) -> WorkflowConfig {
    WorkflowConfig::from_yaml(&workflow(setting))
        .unwrap_or_else(|e| panic!("parse {setting:?}: {e}"))
}

fn timeout(config: &WorkflowConfig) -> Result<Value, serde_json::Error> {
    Ok(serde_json::to_value(config)?["formal_verify"]["model_check_timeout_seconds"].clone())
}

#[test]
fn explicit_timeout_roundtrips_including_boundaries() -> Result<(), Box<dyn std::error::Error>> {
    for seconds in [1, 900, 86400] {
        let config = parse(&format!(
            "formal_verify:\n  model_check_timeout_seconds: {seconds}\n"
        ));
        validate_config(&config).unwrap_or_else(|e| panic!("{seconds}: {e}"));
        assert_eq!(timeout(&config)?, json!(seconds));
        let yaml = serde_yaml::to_string(&config)?;
        assert_eq!(timeout(&WorkflowConfig::from_yaml(&yaml)?)?, json!(seconds));
    }
    Ok(())
}

#[test]
fn out_of_range_timeouts_are_rejected_with_key_name() {
    for seconds in [0, 86401] {
        let config = parse(&format!(
            "formal_verify:\n  model_check_timeout_seconds: {seconds}\n"
        ));
        let Err(error) = validate_config(&config) else {
            panic!("out-of-range timeout must fail validation");
        };
        assert!(
            error.to_string().contains("model_check_timeout_seconds"),
            "{error}"
        );
    }
}

#[test]
fn malformed_timeout_and_unknown_nested_field_are_rejected() {
    for setting in [
        "formal_verify:\n  model_check_timeout_seconds: -1\n",
        "formal_verify:\n  model_check_timeout_seconds: 1.5\n",
        "formal_verify:\n  model_check_timeout_seconds: nope\n",
        "formal_verify:\n  misspelled: 10\n",
    ] {
        assert!(
            WorkflowConfig::from_yaml(&workflow(setting)).is_err(),
            "accepted {setting}"
        );
    }
}

#[test]
fn project_default_is_inherited_and_workflow_override_wins()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = TempDir::new()?;
    let defaults_dir = dir.path().join(".cruise");
    std::fs::create_dir_all(&defaults_dir)?;
    std::fs::write(
        defaults_dir.join("default.yaml"),
        "formal_verify:\n  model_check_timeout_seconds: 123\n",
    )?;
    let defaults = WorkflowDefaults::load(Some(dir.path()))?;
    for (setting, expected) in [
        ("", 123),
        ("formal_verify:\n  model_check_timeout_seconds: 456\n", 456),
    ] {
        let mut resolution = WorkflowConfigResolution::from_yaml(&workflow(setting))?;
        defaults.apply_to(&mut resolution)?;
        validate_config(&resolution.config)?;
        assert_eq!(timeout(&resolution.config)?, json!(expected));
    }
    Ok(())
}
