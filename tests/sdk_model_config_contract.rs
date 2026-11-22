//! Configuration contracts for the sole supported SDK.
use cruise::config::{WorkflowConfig, validate_config};

fn config_error(yaml: &str) -> String {
    match WorkflowConfig::from_yaml(yaml) {
        Ok(config) => match validate_config(&config) {
            Ok(()) => panic!("expected invalid configuration"),
            Err(error) => error.to_string(),
        },
        Err(error) => error.to_string(),
    }
}

#[test]
fn removed_sdk_values_report_the_supported_sdk() {
    for sdk in ["claude", concat!("open", "code")] {
        let yaml = format!(
            "sdk: {sdk}\nmodel: claude/claude-opus-4-5\nsteps:\n  main:\n    prompt: inspect\n"
        );
        let error = config_error(&yaml);
        assert!(
            error.contains(&format!(
                "unsupported SDK \"{sdk}\"; only \"jcode\" is supported"
            )),
            "{error}"
        );
    }
}

#[test]
fn sdk_qualified_model_and_subagent_objects_are_rejected() {
    for yaml in [
        "sdk: jcode\nmodel: [claude/claude-opus-4-5, {sdk: claude, model: openai/fallback}]\nsteps:\n  main:\n    prompt: inspect\n",
        "sdk: jcode\nmodel: claude/claude-opus-4-5\nsubagent_model: {sdk: claude, model: haiku}\nsteps:\n  main:\n    prompt: inspect\n",
    ] {
        let error = config_error(yaml);
        assert!(
            error.contains("SDK-qualified model objects are unsupported; use a plain model string"),
            "{error}"
        );
    }
}

#[test]
fn sdk_qualified_retry_fallback_objects_are_rejected_as_non_string() {
    let error = config_error(
        "sdk: jcode\nmodel: claude/claude-opus-4-5\nretry:\n  fallback_chains:\n    claude/claude-opus-4-5:\n      - {sdk: claude, model: openai/fallback}\nsteps:\n  main:\n    prompt: inspect\n",
    );
    assert!(
        error.contains("retry.fallback_chains") && error.contains("expected a string"),
        "{error}"
    );
}
#[test]
fn sdk_qualified_mcp_entries_are_rejected() {
    let error = config_error(
        "sdk: jcode\nmodel: claude/claude-opus-4-5\nmcp_servers:\n  local:\n    sdk: claude\n    command: local-mcp\nsteps:\n  main:\n    prompt: inspect\n",
    );
    assert!(
        error.contains("mcp_servers") && error.contains("sdk"),
        "{error}"
    );
}

#[test]
fn jcode_keeps_plain_string_fallback_and_subagent_models() {
    let yaml = "sdk: jcode\nmodel: [claude/claude-opus-4-5, openai/fallback]\nsubagent_model: openai/worker\nretry:\n  fallback_chains:\n    claude/claude-opus-4-5:\n      - openai/fallback\nsteps:\n  main:\n    prompt: inspect\n";
    let config =
        WorkflowConfig::from_yaml(yaml).unwrap_or_else(|error| panic!("parse config: {error}"));
    validate_config(&config).unwrap_or_else(|error| panic!("validate config: {error}"));
}

#[test]
fn jcode_rejects_remote_mcp_transports_but_keeps_stdio() {
    for transport in ["http", "sse"] {
        let yaml = format!(
            "sdk: jcode\nmodel: claude/claude-opus-4-5\nmcp_servers:\n  remote:\n    type: {transport}\n    url: https://example.invalid/mcp\nsteps:\n  main:\n    prompt: inspect\n"
        );
        let error = config_error(&yaml);
        assert!(
            error.contains("mcp_servers") && error.to_ascii_lowercase().contains("stdio"),
            "{error}"
        );
    }

    let config = WorkflowConfig::from_yaml(
        "sdk: jcode\nmodel: claude/claude-opus-4-5\nmcp_servers:\n  local:\n    command: local-mcp\nsteps:\n  main:\n    prompt: inspect\n",
    )
    .unwrap_or_else(|error| panic!("parse stdio config: {error}"));
    validate_config(&config).unwrap_or_else(|error| panic!("validate stdio config: {error}"));
}
