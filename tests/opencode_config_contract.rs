//! Observable configuration contracts from plan 7.1.1 and the `OpenCode` safety boundary.
//! These use public APIs only and intentionally fail until `OpenCode` is supported.
use std::{fs, process::Command};

use cruise::config::{WorkflowConfig, validate_computer_use, validate_config, validate_sdk};
use cruise::session_config::serialize_resolved_config;
use cruise::workflow::compile;
use cruise::workflow_call::resolve_workflow_calls_without_app_model_defaults;
use cruise::workflow_defaults::{WorkflowConfigResolution, WorkflowDefaults};
use serde_json::{Value, json};
use tempfile::TempDir;

const BASIC: &str = "sdk: opencode\nsteps:\n  implement:\n    prompt: implement\n";

fn parse(yaml: &str) -> WorkflowConfig {
    WorkflowConfig::from_yaml(yaml)
        .unwrap_or_else(|error| panic!("parse workflow fixture: {error}"))
}

fn defaults(yaml: &str) -> WorkflowDefaults {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let path = dir.path().join(".cruise/default.yaml");
    fs::create_dir_all(
        path.parent()
            .unwrap_or_else(|| panic!("defaults path must have parent")),
    )
    .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
    fs::write(&path, yaml).unwrap_or_else(|error| panic!("write defaults: {error}"));
    WorkflowDefaults::load(Some(dir.path()))
        .unwrap_or_else(|error| panic!("load defaults: {error}"))
}

fn isolated_test(name: &str) -> bool {
    const MARKER: &str = "OPENCODE_CONFIG_ISOLATED_CHILD";
    if std::env::var(MARKER).as_deref() == Ok(name) {
        return true;
    }
    let dir = TempDir::new().unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
    let output = Command::new(
        std::env::current_exe().unwrap_or_else(|error| panic!("fixture operation failed: {error}")),
    )
    .args(["--exact", name, "--nocapture"])
    .env_clear()
    .env(MARKER, name)
    .env("HOME", dir.path())
    .env("XDG_CONFIG_HOME", dir.path().join("config"))
    .env("XDG_DATA_HOME", dir.path().join("data"))
    .env("XDG_STATE_HOME", dir.path().join("state"))
    .output()
    .unwrap_or_else(|error| panic!("run isolated contract: {error}"));
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

#[test]
fn opencode_parse_validate_roundtrip_and_snapshot() {
    let config = parse(
        "sdk: opencode\nmodel: [provider/model#variant, other/model]\nplan_model: provider/plan\nenv: {CONTRACT: kept}\nsteps:\n  implement:\n    prompt: implement\n",
    );
    let original = serde_json::to_value(&config)
        .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
    let roundtrip = parse(
        &serde_yaml::to_string(&config)
            .unwrap_or_else(|error| panic!("fixture operation failed: {error}")),
    );
    assert_eq!(
        serde_json::to_value(roundtrip)
            .unwrap_or_else(|error| panic!("fixture operation failed: {error}")),
        original
    );
    let snapshot = serialize_resolved_config(&config)
        .unwrap_or_else(|error| panic!("serialize resolved snapshot: {error}"));
    let restored = parse(&snapshot);
    assert_eq!(
        serde_json::to_value(restored)
            .unwrap_or_else(|error| panic!("fixture operation failed: {error}")),
        original
    );
    validate_config(&config)
        .unwrap_or_else(|error| panic!("OpenCode must be accepted by full validation: {error}"));
}

#[test]
fn published_schema_sdk_enum_advertises_exact_supported_backends() {
    // Parse the published schema artifact, not Rust source or textual formatting.
    let schema: Value = serde_json::from_str(include_str!("../cruise-schema.json"))
        .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
    let choices = schema["properties"]["sdk"]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("SDK enum must be an array"));
    for sdk in ["jcode", "claude", "opencode"] {
        assert!(choices.contains(&json!(sdk)), "schema must advertise {sdk}");
    }
    for sdk in ["pi", "seher", "unknown-sdk"] {
        assert!(
            !choices.contains(&json!(sdk)),
            "schema must not advertise {sdk}"
        );
    }
}

#[test]
fn opencode_and_command_are_mutually_exclusive() {
    let config = parse(&format!("command: [echo]\n{BASIC}"));
    let error = validate_sdk(&config)
        .err()
        .unwrap_or_else(|| panic!("configuration must be rejected"))
        .to_string();
    assert!(
        error.contains("sdk") && error.contains("command") && error.contains("exclusive"),
        "{error}"
    );
}

#[test]
fn unknown_and_removed_sdk_diagnostics_remain_distinct() {
    for sdk in ["pi", "seher", "unknown-sdk"] {
        let config = parse(&BASIC.replace("opencode", sdk));
        let error = validate_sdk(&config)
            .err()
            .unwrap_or_else(|| panic!("configuration must be rejected"))
            .to_string();
        assert!(error.contains(sdk), "{error}");
        if sdk == "unknown-sdk" {
            assert!(
                error.contains("unknown") && error.contains("opencode"),
                "{error}"
            );
        } else {
            assert!(
                error.contains("no longer supported") && error.contains("model"),
                "{error}"
            );
        }
    }
}

#[test]
fn cruise_sdk_override_uses_public_environment_override_api() {
    // A child test process avoids mutating the multithreaded test runner's environment.
    const MARKER: &str = "OPENCODE_CONFIG_OVERRIDE_CHILD";
    if std::env::var_os(MARKER).is_some() {
        let mut config = parse("sdk: claude\nsteps:\n  implement:\n    prompt: implement\n");
        config
            .apply_env_overrides()
            .unwrap_or_else(|error| panic!("apply environment: {error}"));
        assert_eq!(config.sdk.as_deref(), Some("opencode"));
        validate_config(&config)
            .unwrap_or_else(|error| panic!("environment-selected OpenCode is valid: {error}"));
        return;
    }
    let output = Command::new(
        std::env::current_exe().unwrap_or_else(|error| panic!("fixture operation failed: {error}")),
    )
    .args([
        "--exact",
        "cruise_sdk_override_uses_public_environment_override_api",
        "--nocapture",
    ])
    .env_clear()
    .env(MARKER, "1")
    .env("CRUISE_SDK", "opencode")
    .output()
    .unwrap_or_else(|error| panic!("run isolated override test: {error}"));
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn opencode_defaults_fill_unset_values_and_workflow_overrides_win() {
    if !isolated_test("opencode_defaults_fill_unset_values_and_workflow_overrides_win") {
        return;
    }
    let defaults =
        defaults("sdk: opencode\nmodel: provider/default\nplan_model: provider/planner\n");
    let mut inherited =
        WorkflowConfigResolution::from_yaml("steps:\n  implement:\n    prompt: implement\n")
            .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
    defaults
        .apply_to(&mut inherited)
        .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
    assert_eq!(inherited.config.sdk.as_deref(), Some("opencode"));
    assert_eq!(
        inherited
            .config
            .model
            .as_ref()
            .and_then(|model| model.primary()),
        Some("provider/default")
    );
    let mut explicit =
        WorkflowConfigResolution::from_yaml(&format!("model: provider/explicit\n{BASIC}"))
            .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
    defaults
        .apply_to(&mut explicit)
        .unwrap_or_else(|error| panic!("same-backend defaults may mix: {error}"));
    assert_eq!(
        explicit
            .config
            .model
            .as_ref()
            .and_then(|model| model.primary()),
        Some("provider/explicit")
    );
    assert_eq!(
        explicit
            .config
            .plan_model
            .as_ref()
            .and_then(|model| model.primary()),
        Some("provider/planner")
    );
    validate_config(&inherited.config)
        .unwrap_or_else(|error| panic!("inherited OpenCode is valid: {error}"));
}

#[test]
fn opencode_defaults_reject_cross_backend_model_leakage_in_both_directions() {
    if !isolated_test("opencode_defaults_reject_cross_backend_model_leakage_in_both_directions") {
        return;
    }
    for other in ["jcode", "claude"] {
        for (default_sdk, workflow_sdk) in [("opencode", other), (other, "opencode")] {
            let defaults = defaults(&format!("sdk: {default_sdk}\nmodel: provider/default\n"));
            let mut resolution =
                WorkflowConfigResolution::from_yaml(&BASIC.replace("opencode", workflow_sdk))
                    .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
            let error = defaults
                .apply_to(&mut resolution)
                .err()
                .unwrap_or_else(|| panic!("configuration must be rejected"))
                .to_string();
            assert!(
                error.contains("model")
                    && error.contains(default_sdk)
                    && error.contains(workflow_sdk),
                "{error}"
            );
        }
    }
}

#[test]
fn compile_and_after_pr_preserve_opencode_execution_settings() {
    let config = parse(
        "sdk: opencode\nmodel: provider/main\nplan_model: provider/plan\nenv: {PARENT: kept}\nmcp_servers:\n  local:\n    command: local-mcp\nsteps:\n  implement:\n    prompt: implement\nafter-pr:\n  review:\n    prompt: review\n",
    );
    let compiled = compile(config).unwrap_or_else(|error| panic!("compile OpenCode: {error}"));
    let after = compiled.to_after_pr_compiled();
    assert_eq!(after.sdk.as_deref(), Some("opencode"));
    assert_eq!(after.model, compiled.model);
    assert_eq!(after.plan_model, compiled.plan_model);
    assert_eq!(after.env, compiled.env);
    assert_eq!(
        serde_json::to_value(after.mcp_servers)
            .unwrap_or_else(|error| panic!("fixture operation failed: {error}")),
        serde_json::to_value(compiled.mcp_servers)
            .unwrap_or_else(|error| panic!("fixture operation failed: {error}"))
    );
    assert!(after.steps.contains_key("review"));
    assert!(!after.steps.contains_key("implement"));
}

#[test]
fn called_workflow_keeps_opencode_parent_execution_settings() {
    if !isolated_test("called_workflow_keeps_opencode_parent_execution_settings") {
        return;
    }
    let dir = TempDir::new().unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
    fs::write(dir.path().join("callee.yaml"), "sdk: ignored-sdk\ncommand: [ignored]\nmodel: ignored-model\nplan_model: ignored-plan\nenv: {CALLEE: ignored}\nsteps:\n  review:\n    prompt: review\n").unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
    let parent = parse(
        "sdk: opencode\nmodel: provider/main\nplan_model: provider/plan\nenv: {PARENT: kept}\nsteps:\n  shared:\n    workflow_call: ./callee.yaml\n",
    );
    let expected = serde_json::to_value(&parent)
        .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
    let resolved = resolve_workflow_calls_without_app_model_defaults(
        parent,
        dir.path(),
        &WorkflowDefaults::default(),
    )
    .unwrap_or_else(|error| panic!("resolve called workflow with OpenCode parent: {error}"));
    let actual = serde_json::to_value(&resolved)
        .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
    for key in [
        "sdk",
        "command",
        "model",
        "plan_model",
        "env",
        "permission",
        "mcp_servers",
    ] {
        assert_eq!(
            actual[key], expected[key],
            "callee must not replace parent {key}"
        );
    }
    assert!(
        resolved
            .steps
            .values()
            .any(|step| step.prompt.as_deref() == Some("review"))
    );
    assert!(
        resolved
            .steps
            .values()
            .all(|step| step.workflow_call.is_none())
    );
}

fn computer_use_cases() -> [(&'static str, &'static str); 7] {
    [
        (
            "workflow",
            "computer_use: true\nsteps:\n  work:\n    prompt: work\n",
        ),
        (
            "work",
            "steps:\n  work:\n    prompt: work\n    computer_use: true\n",
        ),
        (
            "review",
            "steps:\n  work:\n    prompt: work\nafter-pr:\n  review:\n    prompt: review\n    computer_use: true\n",
        ),
        (
            "inner",
            "steps:\n  call:\n    group: shared\ngroups:\n  shared:\n    steps:\n      inner:\n        prompt: work\n        computer_use: true\n",
        ),
        (
            "work/child",
            "steps:\n  work:\n    parallel:\n      child:\n        prompt: work\n        computer_use: true\n",
        ),
        (
            "review/child",
            "steps:\n  work:\n    prompt: work\nafter-pr:\n  review:\n    parallel:\n      child:\n        prompt: review\n        computer_use: true\n",
        ),
        (
            "inner/child",
            "steps:\n  call:\n    group: shared\ngroups:\n  shared:\n    steps:\n      inner:\n        parallel:\n          child:\n            prompt: work\n            computer_use: true\n",
        ),
    ]
}

#[test]
fn opencode_rejects_computer_use_at_every_configuration_position() {
    let mut failures = Vec::new();
    for (position, yaml) in computer_use_cases() {
        let config = parse(&format!("sdk: opencode\n{yaml}"));
        // Direct validator prevents unsupported-sdk validation from masking the safety diagnostic.
        match validate_computer_use(&config) {
            Ok(()) => failures.push(format!("{position}: computer_use was accepted")),
            Err(error) => {
                let error = error.to_string();
                if !error.to_lowercase().contains("opencode")
                    || !error.contains("computer_use")
                    || !error.contains(position)
                    || error.contains("command CLI")
                {
                    failures.push(format!("{position}: {error}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn opencode_accepts_explicitly_disabled_computer_use_at_prompt_positions() {
    for (_, yaml) in computer_use_cases() {
        let config = parse(&format!(
            "sdk: opencode\n{}",
            yaml.replace("computer_use: true", "computer_use: false")
        ));
        validate_computer_use(&config)
            .unwrap_or_else(|error| panic!("disabled computer use is valid: {error}"));
    }
}

#[test]
fn opencode_rejects_sse_without_removing_claude_sse_support() {
    for sdk in ["claude", "opencode"] {
        let config = parse(&format!(
            "sdk: {sdk}\nmcp_servers:\n  legacy:\n    type: sse\n    url: https://example.test/sse\nsteps:\n  work:\n    prompt: work\n"
        ));
        let result = validate_config(&config);
        if sdk == "claude" {
            result.unwrap_or_else(|error| panic!("Claude SSE remains supported: {error}"));
        } else {
            let error = result
                .err()
                .unwrap_or_else(|| panic!("configuration must be rejected"))
                .to_string()
                .to_lowercase();
            assert!(
                error.contains("opencode") && error.contains("sse") && error.contains("legacy"),
                "{error}"
            );
            assert!(
                !error.contains("unknown `sdk`"),
                "must reject transport, not SDK: {error}"
            );
        }
    }
}

#[test]
fn opencode_accepts_stdio_and_streamable_http_mcp() {
    let config = parse(
        "sdk: opencode\nmcp_servers:\n  local:\n    command: local-mcp\n    args: [serve]\n    env: {CONTRACT: kept}\n  remote:\n    type: http\n    url: https://example.test/mcp\n    headers: {X-Contract: kept}\nsteps:\n  work:\n    prompt: work\n",
    );
    validate_config(&config)
        .unwrap_or_else(|error| panic!("OpenCode supports stdio and Streamable HTTP: {error}"));
}
