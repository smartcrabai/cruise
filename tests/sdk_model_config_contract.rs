//! Public configuration contracts for SDK-qualified model candidates.
//! No SDK processes, environment overrides, or external services are used.
use cruise::app_config::AppConfig;
use cruise::config::{WorkflowConfig, validate_config};
use serde_json::{Value, json};
use std::fs;
use tempfile::TempDir;

fn workflow(value: &Value) -> WorkflowConfig {
    let yaml =
        serde_yaml::to_string(value).unwrap_or_else(|e| panic!("serialize workflow fixture: {e}"));
    WorkflowConfig::from_yaml(&yaml).unwrap_or_else(|e| panic!("parse workflow fixture: {e}"))
}

fn validation_error(config: &WorkflowConfig) -> String {
    match validate_config(config) {
        Ok(()) => panic!("expected conflicting configuration to be rejected"),
        Err(error) => error.to_string(),
    }
}

fn base() -> Value {
    json!({"steps": {"implement": {"prompt": "implement"}}})
}

fn candidate_forms() -> [Value; 3] {
    [
        json!(["openai/gpt-6.1-sol:medium", {"sdk": "claude", "model": "claude-sonnet-5-5:medium"}, "openrouter/z-ai/glm-5.3-flash:high"]),
        json!([{"sdk": "claude", "model": "claude-first:high"}, "openai/next"]),
        json!([{"sdk": "jcode", "model": "provider/main"}, {"sdk": "claude", "model": "same-name"}]),
    ]
}

#[test]
fn absent_step_override_matches_direct_candidate_resolution() {
    use cruise::model_selection::{Sdk, resolve_candidates, select_step_model};

    for sdk in [Sdk::Jcode, Sdk::Claude, Sdk::OpenCode] {
        for model in [
            None,
            Some(json!("provider/main:high")),
            Some(json!([])),
            Some(json!([
                "provider/main:high",
                {"sdk": "jcode", "model": "provider/jcode"},
                {"sdk": "claude", "model": "claude-next"},
                {"sdk": "opencode", "model": "provider/opencode"},
                "provider/tail"
            ])),
        ] {
            let mut value = base();
            if let Some(model) = model {
                value["model"] = model;
            }
            let config = workflow(&value);
            assert_eq!(
                select_step_model(config.model.as_ref(), None, sdk),
                resolve_candidates(config.model.as_ref(), sdk),
                "sdk={sdk}, workflow={value}"
            );
        }
    }
}

#[test]
fn unknown_workflow_keys_remain_permissive_but_sdk_objects_are_strict() {
    let mut value = base();
    value["unrelated_future_workflow_key"] = json!({"accepted": true});
    value["model"] = json!([{"sdk": "jcode", "model": "provider/main"}]);
    validate_config(&workflow(&value)).unwrap();
    value["model"][0]["unrelated_future_object_key"] = json!(true);
    assert!(WorkflowConfig::from_yaml(&serde_yaml::to_string(&value).unwrap()).is_err());
}

#[test]
fn workflow_yaml_roundtrips_candidate_order_sdk_and_original_element_forms() {
    for candidates in candidate_forms() {
        for field in ["model", "plan_model"] {
            let mut value = base();
            value[field] = candidates.clone();
            let config = workflow(&value);
            validate_config(&config).unwrap_or_else(|e| panic!("valid SDK candidate list: {e}"));
            assert_eq!(
                serde_json::to_value(&config)
                    .unwrap_or_else(|error| panic!("fixture operation failed: {error}"))[field],
                candidates
            );
            let saved = serde_yaml::to_string(&config)
                .unwrap_or_else(|e| panic!("save workflow YAML: {e}"));
            let reloaded = WorkflowConfig::from_yaml(&saved)
                .unwrap_or_else(|e| panic!("reload workflow YAML: {e}"));
            assert_eq!(
                serde_json::to_value(reloaded)
                    .unwrap_or_else(|error| panic!("fixture operation failed: {error}"))[field],
                candidates
            );
        }
    }
}

#[test]
fn app_json_and_yaml_roundtrips_lists_and_plan_model_alias_without_normalizing_entries() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    for extension in ["json", "yaml"] {
        for alias in ["plan_model", "planModel"] {
            for candidates in candidate_forms() {
                let mut value = json!({"model": candidates, "runAllParallelism": 2});
                value[alias] = value["model"].clone();
                let path = dir.path().join(format!("config.{extension}"));
                let content = if extension == "yaml" {
                    serde_yaml::to_string(&value)
                        .unwrap_or_else(|error| panic!("fixture operation failed: {error}"))
                } else {
                    serde_json::to_string(&value)
                        .unwrap_or_else(|error| panic!("fixture operation failed: {error}"))
                };
                fs::write(&path, content).unwrap_or_else(|e| panic!("write app fixture: {e}"));
                let mut config = AppConfig::load_from(&path)
                    .unwrap_or_else(|e| panic!("load mixed app candidates: {e}"));
                // Exercise the existing UI/CLI-style unrelated update and atomic save path.
                config.run_all_parallelism = 3;
                config
                    .save_to(&path)
                    .unwrap_or_else(|e| panic!("save mixed app candidates: {e}"));
                let loaded = AppConfig::load_from(&path)
                    .unwrap_or_else(|e| panic!("reload app candidates: {e}"));
                let saved = serde_json::to_value(&loaded)
                    .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
                assert_eq!(saved["model"], value["model"]);
                assert_eq!(saved["plan_model"], value[alias]);
                assert!(saved.get("planModel").is_none());
                assert_eq!(saved["runAllParallelism"], 3);
                assert_eq!(loaded, config);
            }
        }
    }
}

fn invalid_entries() -> [Value; 18] {
    [
        json!({}),
        json!({"sdk": "claude"}),
        json!({"model": "provider/model"}),
        json!({"sdk": "", "model": "provider/model"}),
        json!({"sdk": "claude", "model": ""}),
        json!({"sdk": "claude", "model": "provider/model", "typo": true}),
        json!({"sdk": 1, "model": "provider/model"}),
        json!({"sdk": "claude", "model": 1}),
        json!({"sdk": null, "model": "provider/model"}),
        json!({"sdk": "claude", "model": null}),
        json!({"sdk": "unknown-sdk", "model": "provider/model"}),
        json!({"sdk": "openai", "model": "provider/model"}),
        json!({"sdk": "openrouter", "model": "provider/model"}),
        json!({"sdk": "seher", "model": "provider/model"}),
        json!({"sdk": "pi", "model": "provider/model"}),
        json!(42),
        json!(null),
        json!(["nested"]),
    ]
}

#[test]
fn workflow_rejects_invalid_candidate_fields_types_and_sdks_in_both_lists() {
    for field in ["model", "plan_model"] {
        for invalid in invalid_entries() {
            for candidates in [json!([invalid]), json!(["provider/valid", invalid])] {
                let mut value = base();
                value[field] = candidates;
                let yaml = serde_yaml::to_string(&value)
                    .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
                let rejected = match WorkflowConfig::from_yaml(&yaml) {
                    Ok(config) => validate_config(&config).is_err(),
                    Err(_) => true,
                };
                assert!(rejected, "accepted invalid {field}: {}", value[field]);
            }
        }
    }
}

#[test]
fn app_load_rejects_invalid_candidates_in_json_yaml_and_plan_alias() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    for extension in ["json", "yaml"] {
        for field in ["model", "plan_model", "planModel"] {
            for invalid in invalid_entries() {
                let mut value = json!({});
                value[field] = json!(["provider/valid", invalid]);
                let path = dir.path().join(format!("invalid.{extension}"));
                let content = if extension == "yaml" {
                    serde_yaml::to_string(&value)
                        .unwrap_or_else(|error| panic!("fixture operation failed: {error}"))
                } else {
                    serde_json::to_string(&value)
                        .unwrap_or_else(|error| panic!("fixture operation failed: {error}"))
                };
                fs::write(&path, content)
                    .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
                assert!(
                    AppConfig::load_from(&path).is_err(),
                    "accepted {extension} {field}: {}",
                    value[field]
                );
            }
        }
    }
}

#[test]
fn qualified_single_objects_remain_invalid_outside_arrays() {
    let dir = TempDir::new().unwrap_or_else(|e| panic!("tempdir: {e}"));
    for field in ["model", "plan_model"] {
        let mut value = base();
        value[field] = json!({"sdk": "claude", "model": "example"});
        assert!(
            WorkflowConfig::from_yaml(
                &serde_yaml::to_string(&value)
                    .unwrap_or_else(|error| panic!("fixture operation failed: {error}"))
            )
            .is_err()
        );
        for extension in ["json", "yaml"] {
            let path = dir.path().join(format!("single.{extension}"));
            let mut app = json!({});
            app[field] = value[field].clone();
            fs::write(
                &path,
                if extension == "yaml" {
                    serde_yaml::to_string(&app)
                        .unwrap_or_else(|error| panic!("fixture operation failed: {error}"))
                } else {
                    serde_json::to_string(&app)
                        .unwrap_or_else(|error| panic!("fixture operation failed: {error}"))
                },
            )
            .unwrap_or_else(|error| panic!("fixture operation failed: {error}"));
            assert!(AppConfig::load_from(&path).is_err());
        }
    }
}

#[test]
fn command_rejects_sdk_objects_at_primary_and_fallback_positions_in_either_list() {
    for field in ["model", "plan_model"] {
        for candidates in candidate_forms() {
            let mut value = base();
            value[field] = candidates;
            let valid = workflow(&value);
            validate_config(&valid)
                .unwrap_or_else(|e| panic!("list must be valid without command: {e}"));
            value["command"] = json!(["custom-agent"]);
            let error = validation_error(&workflow(&value));
            assert!(
                error.contains("command"),
                "missing command diagnostic: {error}"
            );
        }
    }
}

#[test]
fn explicit_chains_replace_incompatible_implicit_tails_for_capabilities() {
    for chain in [json!([]), json!(["provider/safe"]), json!(["provider/main:low"])] {
        let desktop = workflow(&json!({
            "sdk": "jcode", "computer_use": true,
            "model": ["provider/main:high", {"sdk": "claude", "model": "unreachable"}],
            "plan_model": [], "retry": {"fallback_chains": {"provider/main": chain}},
            "steps": {"desktop": {"prompt": "inspect"}}
        }));
        cruise::config::validate_computer_use(&desktop).unwrap_or_else(|error| panic!("overridden tail must not reject computer use: {error}"));
        if !desktop.retry.as_ref().unwrap().fallback_chains["provider/main"].is_empty() {
            validate_config(&desktop).unwrap();
        }
    }
    for transport in ["http", "sse"] {
        for chain in [json!(["claude-safe"]), json!(["claude-main:low"])] {
            let remote = workflow(&json!({
                "sdk": "claude", "model": ["claude-main:high", {"sdk": "jcode", "model": "unreachable"}],
                "plan_model": [], "retry": {"fallback_chains": {"claude-main": chain}},
                "mcp_servers": {"remote": {"type": transport, "url": "https://example.invalid/mcp"}},
                "steps": {"answer": {"prompt": "answer"}}
            }));
            validate_config(&remote).unwrap_or_else(|error| panic!("overridden tail must not reject remote MCP: {error}"));
        }
    }
    // A wildcard with no primary model expands to nothing, but is still authoritative.
    let remote = workflow(&json!({"sdk": "claude", "model": ["", {"sdk": "jcode", "model": "unreachable"}],
        "retry": {"fallback_chains": {"default": ["provider/*"]}},
        "mcp_servers": {"remote": {"type": "http", "url": "https://example.invalid/mcp"}},
        "steps": {"answer": {"prompt": "answer"}}}));
    validate_config(&remote).unwrap();
}

#[test]
fn explicit_candidate_ownership_controls_computer_use_not_workflow_sdk() {
    for sdk in ["claude", "opencode"] {
        let mut value = json!({"sdk": sdk, "model": [{"sdk": "jcode", "model": "provider/desktop"}],
            "plan_model": [], "computer_use": true, "steps": {"desktop": {"prompt": "inspect"}}});
        validate_config(&workflow(&value)).unwrap();
        value["model"][0]["sdk"] = json!("claude");
        let error = validation_error(&workflow(&value));
        for meaning in ["every effective applicable candidate", "jcode backend", "no `command:`", "does not override SDK-qualified objects"] {
            assert!(error.contains(meaning), "missing {meaning}: {error}");
        }
    }
}

#[test]
fn remote_mcp_transports_require_all_explicit_candidates_to_support_them() {
    for workflow_sdk in ["jcode", "claude", "opencode"] {
        for (owners, transport, valid) in [
            (["claude", "opencode"], "http", true),
            (["claude", "claude"], "sse", true),
            (["claude", "opencode"], "sse", false),
            (["claude", "jcode"], "http", false),
        ] {
            let config = workflow(&json!({"sdk": workflow_sdk,
                "model": [{"sdk": owners[0], "model": "primary"}, {"sdk": owners[1], "model": "fallback"}],
                "plan_model": [], "mcp_servers": {
                    "remote_a": {"type": transport, "url": "https://example.invalid/a"},
                    "remote_b": {"type": "http", "url": "https://example.invalid/b"},
                    "local": {"command": "local-mcp"}
                }, "steps": {"answer": {"prompt": "answer"}}}));
            if valid { validate_config(&config).unwrap(); } else {
                let error = validation_error(&config);
                assert!(error.contains("mcp_servers."), "{error}");
                assert!(error.contains("every effective applicable candidate"), "{error}");
                assert!(error.contains("does not override SDK-qualified objects"), "{error}");
            }
        }
    }
}

#[test]
fn computer_use_rejects_claude_primary_and_fallback_candidates() {
    for field in ["model", "plan_model"] {
        for candidates in [
            json!([{"sdk": "claude", "model": "example"}, "provider/next"]),
            json!(["provider/main", {"sdk": "claude", "model": "example"}]),
        ] {
            let mut value = base();
            value[field] = candidates;
            validate_config(&workflow(&value))
                .unwrap_or_else(|e| panic!("valid without computer use: {e}"));
            value["computer_use"] = json!(true);
            let error = validation_error(&workflow(&value));
            assert!(
                error.contains("computer_use"),
                "missing capability diagnostic: {error}"
            );
        }
    }
}

#[test]
fn remote_mcp_rejects_jcode_primary_and_fallback_candidates_under_claude() {
    for transport in ["http", "sse"] {
        for field in ["model", "plan_model"] {
            for candidates in [
                json!([{"sdk": "jcode", "model": "provider/main"}, "claude-next"]),
                json!(["claude-main", {"sdk": "jcode", "model": "provider/next"}]),
            ] {
                let mut value = base();
                value["sdk"] = json!("claude");
                value[field] = candidates;
                validate_config(&workflow(&value))
                    .unwrap_or_else(|e| panic!("valid without remote MCP: {e}"));
                value["mcp_servers"] =
                    json!({"remote": {"type": transport, "url": "https://example.invalid/mcp"}});
                let error = validation_error(&workflow(&value));
                assert!(
                    error.contains("MCP") || error.contains("mcp"),
                    "missing MCP diagnostic: {error}"
                );
            }
        }
    }
}

#[test]
fn computer_use_step_override_does_not_inherit_unrelated_claude_candidates() {
    let config = workflow(&json!({
        "model": ["provider/main", {"sdk": "claude", "model": "claude-fallback"}],
        "plan_model": [{"sdk": "claude", "model": "claude-planner"}],
        "steps": {
            "desktop": {
                "prompt": "inspect desktop",
                "model": "provider/desktop",
                "computer_use": true
            },
            "implement": {"prompt": "implement without computer use"}
        }
    }));
    validate_config(&config).unwrap_or_else(|error| {
        panic!("step-specific jcode model must not inherit unrelated Claude candidates: {error}")
    });
}

#[test]
fn explicit_retry_strings_inherit_workflow_sdk_for_capability_checks() {
    for field in ["model", "plan_model"] {
        let mut desktop = base();
        desktop["sdk"] = json!("claude");
        desktop[field] = json!([{"sdk": "jcode", "model": "provider/desktop"}]);
        desktop[if field == "model" {
            "plan_model"
        } else {
            "model"
        }] = desktop[field].clone();
        desktop["computer_use"] = json!(true);
        validate_config(&workflow(&desktop))
            .unwrap_or_else(|error| panic!("jcode-only desktop candidates must be valid: {error}"));
        desktop["retry"] = json!({"fallback_chains": {"default": ["claude-fallback"]}});
        assert!(validation_error(&workflow(&desktop)).contains("computer_use"));

        let mut remote = base();
        remote[field] = json!([{"sdk": "claude", "model": "claude-main"}]);
        // Keep both prompt purposes on Claude until an explicit chain adds jcode.
        remote[if field == "model" {
            "plan_model"
        } else {
            "model"
        }] = remote[field].clone();
        remote["mcp_servers"] =
            json!({"remote": {"type": "http", "url": "https://example.invalid/mcp"}});
        validate_config(&workflow(&remote)).unwrap_or_else(|error| {
            panic!("Claude-only remote MCP candidates must be valid: {error}")
        });
        remote["retry"] = json!({"fallback_chains": {"default": ["provider/fallback"]}});
        let error = validation_error(&workflow(&remote));
        assert!(error.contains("MCP") || error.contains("mcp"));
    }
}

fn assert_retired_sdk_diagnostic(error: &str, sdk: &str) {
    for fragment in [sdk, "claude", "model"] {
        assert!(
            error.contains(fragment),
            "missing migration fragment {fragment:?}: {error}"
        );
    }
    if sdk == "seher" {
        for fragment in ["build", "plan"] {
            assert!(
                error.contains(fragment),
                "missing retired mode guidance {fragment:?}: {error}"
            );
        }
    }
}

#[test]
fn workflow_retired_candidate_sdks_preserve_migration_guidance() {
    for sdk in ["seher", "pi"] {
        for field in ["model", "plan_model"] {
            for candidates in [
                json!([{"sdk": sdk, "model": "build"}]),
                json!(["provider/main", {"sdk": sdk, "model": "plan"}]),
            ] {
                let mut value = base();
                value[field] = candidates;
                let yaml = serde_yaml::to_string(&value)
                    .unwrap_or_else(|error| panic!("serialize retired SDK fixture: {error}"));
                let error = match WorkflowConfig::from_yaml(&yaml) {
                    Ok(config) => validation_error(&config),
                    Err(error) => error.to_string(),
                };
                assert_retired_sdk_diagnostic(&error, sdk);
            }
        }
    }
}

#[test]
fn app_retired_candidate_sdks_preserve_migration_guidance_in_json_and_yaml() {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    for extension in ["json", "yaml"] {
        for field in ["model", "plan_model", "planModel"] {
            for sdk in ["seher", "pi"] {
                let mut value = json!({});
                value[field] = json!(["provider/main", {"sdk": sdk, "model": "build"}]);
                let path = dir.path().join(format!("retired.{extension}"));
                let content = if extension == "yaml" {
                    serde_yaml::to_string(&value)
                        .unwrap_or_else(|error| panic!("serialize YAML: {error}"))
                } else {
                    serde_json::to_string(&value)
                        .unwrap_or_else(|error| panic!("serialize JSON: {error}"))
                };
                fs::write(&path, content)
                    .unwrap_or_else(|error| panic!("write retired SDK fixture: {error}"));
                let error = match AppConfig::load_from(&path) {
                    Ok(_) => panic!("retired candidate SDK {sdk} must be rejected"),
                    Err(error) => error.to_string(),
                };
                assert_retired_sdk_diagnostic(&error, sdk);
            }
        }
    }
}
