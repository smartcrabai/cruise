//! Load-time input contracts from plan sections 1 and 2.
//!
//! Contract map:
//! - Required string inputs and strict call arguments -> declaration/argument error tests.
//! - Lexical binding before recursion -> `repeated_calls_keep_nested_scopes_isolated`.
//! - Value fields, source-relative prompt files and parallel children -> value binding tests.
//! - Runtime tokens, escapes and single-pass insertion -> runtime template tests.
//! - Execution-time metadata removal -> `materialized_config_has_no_call_metadata`.
//! - Workflow-only metadata -> `defaults_reject_input_declarations_as_workflow_only_metadata`.
//!
//! Existing `workflow_call` tests retain legacy local/GitHub cycle, prefix, group and flag rules.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cruise::config::{WorkflowConfig, validate_config};
use cruise::error::CruiseError;
use cruise::variable::VariableStore;
use cruise::workflow_call::resolve_workflow_calls_from_path_with_defaults;
use cruise::workflow_defaults::WorkflowDefaults;
use serde_json::{Value, json};
use tempfile::TempDir;

fn write(dir: &Path, relative: &str, contents: &str) -> PathBuf {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().unwrap_or(dir))
        .unwrap_or_else(|error| panic!("create fixture directory: {error}"));
    std::fs::write(&path, contents).unwrap_or_else(|error| panic!("write {relative}: {error}"));
    path
}

fn load(path: &Path) -> cruise::error::Result<WorkflowConfig> {
    let config =
        resolve_workflow_calls_from_path_with_defaults(path, &WorkflowDefaults::default())?;
    validate_config(&config)?;
    Ok(config)
}

fn resolved(path: &Path) -> WorkflowConfig {
    load(path).unwrap_or_else(|error| panic!("resolve {}: {error}", path.display()))
}

fn call_fixture(callee: &str, arguments: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    write(dir.path(), "callee.yaml", callee);
    let root = write(
        dir.path(),
        "caller.yaml",
        &format!(
            "model: fixture-model\nplan_model: fixture-model\ncommand: [unused-agent]\ncreate_pr: false\nsteps:\n  review:\n    workflow_call: ./callee.yaml\n{arguments}"
        ),
    );
    (dir, root)
}

fn invalid_step(path: &Path, fragments: &[&str]) {
    match load(path) {
        Err(CruiseError::InvalidStepConfig(message)) => {
            for fragment in fragments {
                assert!(
                    message.contains(fragment),
                    "missing {fragment:?}: {message}"
                );
            }
        }
        other => panic!("expected InvalidStepConfig, got {other:?}"),
    }
}

#[test]
fn repeated_calls_keep_nested_scopes_isolated() {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    write(
        dir.path(),
        "nested/outer.yaml",
        r"inputs: [label, forwarded]
steps:
  own:
    prompt: '{inputs.label}'
  child:
    workflow_call: ./inner/leaf.yaml
    with:
      label: child-value
      forwarded: '{inputs.forwarded}'
  own-again:
    prompt: '{inputs.label}'
",
    );
    write(
        dir.path(),
        "nested/inner/leaf.yaml",
        "inputs: [label, forwarded]\nsteps:\n  leaf:\n    prompt: '{inputs.label}|{inputs.forwarded}'\n",
    );
    let root = write(
        dir.path(),
        "caller.yaml",
        r"model: fixture-model
plan_model: fixture-model
command: [unused-agent]
steps:
  first:
    workflow_call: ./nested/outer.yaml
    with: {label: first-parent, forwarded: first-forwarded}
  second:
    workflow_call: ./nested/outer.yaml
    with: {label: second-parent, forwarded: second-forwarded}
",
    );
    let config = resolved(&root);
    for (id, expected) in [
        ("first/own", "first-parent"),
        ("first/child/leaf", "child-value|first-forwarded"),
        ("first/own-again", "first-parent"),
        ("second/own", "second-parent"),
        ("second/child/leaf", "child-value|second-forwarded"),
        ("second/own-again", "second-parent"),
    ] {
        assert_eq!(config.steps[id].prompt.as_deref(), Some(expected), "{id}");
    }
}

#[test]
fn nested_call_does_not_implicitly_inherit_parent_inputs() {
    let (dir, root) = call_fixture(
        "inputs: [label]\nsteps:\n  child:\n    workflow_call: ./child.yaml\n",
        "    with: {label: parent}\n",
    );
    write(
        dir.path(),
        "child.yaml",
        "inputs: [label]\nsteps:\n  leaf:\n    prompt: '{inputs.label}'\n",
    );
    invalid_step(&root, &["child", "label"]);
}

#[test]
fn input_binding_preserves_runtime_tokens_and_brace_escapes() {
    let (_dir, root) = call_fixture(
        r"inputs: [fragment]
steps:
  review:
    prompt: '{inputs.fragment}|{{inputs.fragment}}|{{literal}}|{prev.output}'
",
        "    with:\n      fragment: '{input}|{plan}|{pr.url}'\n",
    );
    let config = resolved(&root);
    let prompt = config.steps["review/review"]
        .prompt
        .as_deref()
        .unwrap_or_else(|| panic!("resolved prompt missing"));
    assert_eq!(
        prompt,
        "{input}|{plan}|{pr.url}|{{inputs.fragment}}|{{literal}}|{prev.output}"
    );
    let mut variables = VariableStore::new("runtime-input".into());
    variables.set_named_value("plan", "runtime-plan".into());
    variables.set_named_value("pr.url", "runtime-pr-url".into());
    variables.set_prev_output(Some("runtime-previous".into()));
    assert_eq!(
        variables
            .resolve_prompt(prompt)
            .unwrap_or_else(|error| panic!("runtime: {error}")),
        "runtime-input|runtime-plan|runtime-pr-url|{inputs.fragment}|{literal}|runtime-previous"
    );
}

#[test]
fn injected_template_fragments_keep_their_escapes_until_runtime() {
    let (_dir, root) = call_fixture(
        "inputs: [fragment]\nsteps:\n  review:\n    prompt: '{inputs.fragment}'\n",
        "    with: {fragment: '{{inputs.not_declared}}|{{literal}}|{input}'}\n",
    );
    let config = resolved(&root);
    let prompt = config.steps["review/review"]
        .prompt
        .as_deref()
        .unwrap_or_else(|| panic!("prompt missing"));
    assert_eq!(prompt, "{{inputs.not_declared}}|{{literal}}|{input}");
    let variables = VariableStore::new("runtime".into());
    assert_eq!(
        variables
            .resolve_prompt(prompt)
            .unwrap_or_else(|error| panic!("runtime: {error}")),
        "{inputs.not_declared}|{literal}|runtime"
    );
}

#[test]
fn binding_applies_to_instruction_and_environment_values_without_shell_quoting() {
    let (_dir, root) = call_fixture(
        r"inputs: [fragment]
steps:
  review:
    prompt: inspect
    instruction: 'Review {inputs.fragment}'
    env:
      LABEL: '{inputs.fragment}'
",
        "    with: {fragment: 'a b; $HOME'}\n",
    );
    let config = resolved(&root);
    let step = &config.steps["review/review"];
    assert_eq!(step.instruction.as_deref(), Some("Review a b; $HOME"));
    assert_eq!(
        step.env.get("LABEL").map(String::as_str),
        Some("a b; $HOME")
    );
}

#[test]
fn binding_applies_to_both_command_representations() {
    let (_dir, root) = call_fixture(
        r"inputs: [fragment]
steps:
  single:
    command: 'echo {inputs.fragment}'
  multiple:
    command: ['first {inputs.fragment}', 'second {inputs.fragment}']
",
        "    with: {fragment: 'a b; $HOME'}\n",
    );
    let config = resolved(&root);
    for (id, expected) in [
        ("review/single", vec!["echo a b; $HOME"]),
        (
            "review/multiple",
            vec!["first a b; $HOME", "second a b; $HOME"],
        ),
    ] {
        let commands = config.steps[id]
            .command
            .as_ref()
            .unwrap_or_else(|| panic!("{id}"));
        assert_eq!(commands.as_slice(), expected.as_slice());
    }
}

#[test]
fn prompt_files_and_parallel_children_bind_in_the_declaring_source_scope() {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let template = "{inputs.label}|{{inputs.label}}|{{literal}}|{input}\n";
    write(dir.path(), "nested/prompt.md", template);
    // A same-named file at the caller must not win over the callee's file.
    write(dir.path(), "prompt.md", "wrong source");
    write(
        dir.path(),
        "nested/callee.yaml",
        r"inputs: [label]
steps:
  from-file:
    prompt_file: prompt.md
  concurrent:
    parallel:
      review:
        prompt_file: prompt.md
        instruction: '{inputs.label}'
      check:
        command: ['echo {inputs.label}', 'echo {input}']
        env:
          LABEL: '{inputs.label}'
",
    );
    let root = write(
        dir.path(),
        "caller.yaml",
        "model: fixture-model\nplan_model: fixture-model\ncommand: [unused-agent]\nsteps:\n  review:\n    workflow_call: ./nested/callee.yaml\n    with: {label: supplied}\n",
    );
    let config = resolved(&root);
    let children = config.steps["review/concurrent"]
        .parallel
        .as_ref()
        .unwrap_or_else(|| panic!("parallel children missing"));
    let variables = VariableStore::new("runtime".into());
    for step in [&config.steps["review/from-file"], &children["review"]] {
        assert!(step.prompt_file.is_none());
        let prompt = step
            .prompt
            .as_deref()
            .unwrap_or_else(|| panic!("prompt missing"));
        assert_eq!(prompt, "supplied|{{inputs.label}}|{{literal}}|{input}\n");
        assert_eq!(
            variables
                .resolve_prompt(prompt)
                .unwrap_or_else(|error| panic!("runtime: {error}")),
            "supplied|{inputs.label}|{literal}|runtime\n"
        );
    }
    assert_eq!(children["review"].instruction.as_deref(), Some("supplied"));
    assert_eq!(
        children["check"].env.get("LABEL").map(String::as_str),
        Some("supplied")
    );
    assert_eq!(
        children["check"]
            .command
            .as_ref()
            .unwrap_or_else(|| panic!("command missing"))
            .as_slice(),
        ["echo supplied", "echo {input}"]
    );
}

#[test]
fn empty_string_arguments_and_identifier_boundaries_are_supported() {
    let (_dir, root) = call_fixture(
        "inputs: [_name2, Z, _]\nsteps:\n  review:\n    prompt: 'before{inputs._name2}{inputs.Z}{inputs._}after'\n",
        "    with: {_name2: '', Z: '', _: ''}\n",
    );
    assert_eq!(
        resolved(&root).steps["review/review"].prompt.as_deref(),
        Some("beforeafter")
    );
}

#[test]
fn binding_does_not_parameterize_step_ids_models_or_prompt_file_paths() {
    let (dir, root) = call_fixture(
        "inputs: [label]\nsteps:\n  '{inputs.label}':\n    prompt_file: '{inputs.label}.md'\n    model: '{inputs.label}'\n  finish:\n    command: 'true'\n    next: '{inputs.label}'\n",
        "    with: {label: supplied}\n",
    );
    write(dir.path(), "{inputs.label}.md", "Review {inputs.label}");
    let config = resolved(&root);
    let step = &config.steps["review/{inputs.label}"];
    assert_eq!(step.prompt.as_deref(), Some("Review supplied"));
    assert_eq!(step.model.as_deref(), Some("{inputs.label}"));
    assert_eq!(
        config.steps["review/finish"].next.as_deref(),
        Some("review/{inputs.label}")
    );
}

#[test]
fn missing_required_argument_identifies_call_site_and_input() {
    let (_dir, root) = call_fixture(
        "inputs: [label]\nsteps:\n  review:\n    prompt: inspect\n",
        "",
    );
    invalid_step(&root, &["review", "label"]);
}

#[test]
fn extra_argument_is_rejected_even_when_the_callee_declares_no_inputs() {
    let (_dir, root) = call_fixture(
        "steps:\n  review:\n    prompt: inspect\n",
        "    with: {unexpected: supplied}\n",
    );
    invalid_step(&root, &["review", "unexpected"]);
}

#[test]
fn undeclared_input_reference_is_rejected_before_execution() {
    let (_dir, root) = call_fixture(
        "inputs: [label]\nsteps:\n  review:\n    prompt: '{inputs.typo}'\n",
        "    with: {label: supplied}\n",
    );
    invalid_step(&root, &["review", "typo"]);
}

#[test]
fn duplicate_input_declarations_are_rejected() {
    let (_dir, root) = call_fixture(
        "inputs: [label, label]\nsteps:\n  review:\n    prompt: inspect\n",
        "    with: {label: supplied}\n",
    );
    invalid_step(&root, &["label"]);
}

#[test]
fn invalid_input_names_are_rejected() {
    for name in [
        "",
        "1label",
        "has-dash",
        "has.dot",
        "has space",
        "\u{65e5}\u{672c}\u{8a9e}",
    ] {
        let quoted = serde_json::to_string(name).unwrap_or_else(|error| panic!("quote: {error}"));
        let (_dir, root) = call_fixture(
            &format!("inputs: [{quoted}]\nsteps:\n  review:\n    prompt: inspect\n"),
            &format!("    with: {{{quoted}: supplied}}\n"),
        );
        invalid_step(&root, &["input"]);
    }
}

#[test]
fn with_values_must_be_strings_without_yaml_scalar_coercion() {
    for value in ["42", "true", "null", "[a]", "{a: b}"] {
        let yaml = format!(
            "steps:\n  review:\n    workflow_call: ./callee.yaml\n    with: {{label: {value}}}\n"
        );
        assert!(
            WorkflowConfig::from_yaml(&yaml).is_err(),
            "accepted non-string with value: {value}"
        );
    }
}

#[test]
fn with_mapping_is_only_allowed_on_workflow_calls_even_when_empty() {
    for executable in [
        "prompt: inspect",
        "command: 'true'",
        "option: [{selector: done}]",
        "parallel: {child: {command: 'true'}}",
    ] {
        for arguments in ["{}", "{label: value}"] {
            let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
            let root = write(
                dir.path(),
                "caller.yaml",
                &format!(
                    "model: fixture-model\nplan_model: fixture-model\ncommand: [unused-agent]\nsteps:\n  invalid:\n    {executable}\n    with: {arguments}\n"
                ),
            );
            invalid_step(&root, &["invalid", "with"]);
        }
    }
}

#[test]
fn with_is_rejected_inside_groups_parallel_children_and_native_actions() {
    for yaml in [
        "model: fixture-model\nplan_model: fixture-model\ncommand: [unused-agent]\ngroups:\n  batch:\n    steps:\n      invalid:\n        prompt: inspect\n        with: {}\nsteps:\n  run:\n    group: batch\n",
        "model: fixture-model\nplan_model: fixture-model\ncommand: [unused-agent]\nsteps:\n  batch:\n    parallel:\n      invalid:\n        prompt: inspect\n        with: {}\n",
        "model: fixture-model\nplan_model: fixture-model\ncommand: [unused-agent]\ngroups:\n  batch:\n    steps:\n      leaf:\n        command: 'true'\nsteps:\n  invalid:\n    group: batch\n    with: {}\n",
        "steps: {}\nflags:\n  merge: {}\nafter-pr:\n  invalid:\n    squash-merge: true\n    flag: merge\n    with: {}\n",
    ] {
        let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
        let root = write(dir.path(), "caller.yaml", yaml);
        invalid_step(&root, &["invalid", "with"]);
    }
}

#[test]
fn null_with_is_equivalent_to_absence_on_legacy_steps_and_calls() {
    for arguments in ["", "    with: null\n", "    with: {}\n"] {
        let (_dir, root) = call_fixture("steps:\n  leaf:\n    command: 'true'\n", arguments);
        assert!(resolved(&root).steps.contains_key("review/leaf"));
    }
    let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let root = write(
        dir.path(),
        "caller.yaml",
        "steps:\n  leaf:\n    command: 'true'\n    with: null\n",
    );
    assert!(resolved(&root).steps.contains_key("leaf"));
}

#[test]
fn root_with_required_inputs_has_no_argument_supply() {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let root = write(
        dir.path(),
        "root.yaml",
        "inputs: [label]\nsteps:\n  leaf:\n    command: 'true'\n",
    );
    invalid_step(&root, &["label"]);
}

#[test]
fn root_cannot_reference_undeclared_inputs() {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    let root = write(
        dir.path(),
        "root.yaml",
        "model: fixture-model\nplan_model: fixture-model\ncommand: [unused-agent]\nsteps:\n  leaf:\n    prompt: '{inputs.typo}'\n",
    );
    invalid_step(&root, &["typo"]);
}

#[test]
fn defaults_reject_input_declarations_as_workflow_only_metadata() {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    write(dir.path(), ".cruise/default.yaml", "inputs: []\n");
    let error = WorkflowDefaults::load(Some(dir.path()))
        .err()
        .unwrap_or_else(|| panic!("inputs must not be accepted in defaults"));
    let message = error.to_string();
    for fragment in ["default.yaml", "inputs", "workflow-only"] {
        assert!(
            message.contains(fragment),
            "missing {fragment:?}: {message}"
        );
    }
}

fn assert_materialized(value: &Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                assert!(
                    !["inputs", "with", "workflow_call", "prompt_file"].contains(&key.as_str()),
                    "unresolved materialization metadata: {key}"
                );
                assert_materialized(child);
            }
        }
        Value::Array(items) => items.iter().for_each(assert_materialized),
        _ => {}
    }
}

#[test]
fn materialized_config_has_no_call_metadata_and_retains_runtime_tokens() {
    let (_dir, root) = call_fixture(
        "inputs: [label]\nsteps:\n  review:\n    prompt: '{inputs.label}'\n",
        "    with: {label: '{input}'}\n",
    );
    let config = resolved(&root);
    assert_materialized(
        &serde_json::to_value(&config).unwrap_or_else(|error| panic!("serialize: {error}")),
    );
    assert_eq!(
        config.steps["review/review"].prompt.as_deref(),
        Some("{input}")
    );
}

fn schema_validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let schema: Value = serde_json::from_str(include_str!("../cruise-schema.json"))
            .unwrap_or_else(|error| panic!("published schema: {error}"));
        jsonschema::validator_for(&schema)
            .unwrap_or_else(|error| panic!("schema validator: {error}"))
    })
}

#[test]
fn published_schema_accepts_input_declarations_and_string_call_arguments() {
    assert!(schema_validator().is_valid(&json!({
        "inputs": ["_label2", "Z"],
        "steps": {"nested": {"workflow_call": "./shared.yaml", "with": {"label": ""}}}
    })));
}

#[test]
fn published_schema_accepts_null_with_as_no_arguments() {
    for step in [
        json!({"workflow_call": "./shared.yaml", "with": null}),
        json!({"command": "true", "with": null}),
    ] {
        assert!(schema_validator().is_valid(&json!({"steps": {"run": step}})));
    }
}

#[test]
fn published_schema_rejects_invalid_input_declarations() {
    for names in [
        json!(["label", "label"]),
        json!([""]),
        json!(["1label"]),
        json!(["bad-name"]),
        json!([42]),
        json!("label"),
    ] {
        assert!(
            !schema_validator().is_valid(&json!({"inputs": names, "steps": {}})),
            "invalid inputs: {names}"
        );
    }
}

#[test]
fn published_schema_rejects_non_string_call_argument_values() {
    for value in [
        json!(42),
        json!(true),
        Value::Null,
        json!(["a"]),
        json!({"a": "b"}),
    ] {
        assert!(
            !schema_validator().is_valid(&json!({
                "steps": {"nested": {"workflow_call": "./shared.yaml", "with": {"label": value}}}
            })),
            "invalid with value: {value}"
        );
    }
}

#[test]
fn published_schema_rejects_with_on_non_call_steps() {
    assert!(!schema_validator().is_valid(&json!({
        "steps": {"invalid": {"command": "true", "with": {}}}
    })));
}

/// The real CLI must reject invalid calls before running even an earlier command.
/// This is an execution preflight contract, not a permanent echo wiring smoke test.
#[cfg(unix)]
#[test]
fn cli_rejects_missing_inputs_before_any_command_side_effect() {
    use std::process::{Command, Stdio};

    let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    // Only this independent temporary repository receives a fixture commit.
    // The working repository and its linked worktrees are never modified.
    for arguments in [
        &["init", "-q", "-b", "main"][..],
        &[
            "-c",
            "user.name=Contract Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fixture",
        ][..],
    ] {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(dir.path())
            .env("HOME", dir.path().join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap_or_else(|error| panic!("initialize temporary repository: {error}"));
        assert!(
            output.status.success(),
            "git fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    write(
        dir.path(),
        "callee.yaml",
        "inputs: [label]\nsteps:\n  leaf:\n    command: 'true'\n",
    );
    let root = write(
        dir.path(),
        "caller.yaml",
        "create_pr: false\nsteps:\n  earlier:\n    command: 'printf executed > sentinel'\n  missing-label:\n    workflow_call: ./callee.yaml\n",
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_cruise"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CRUISE_") {
            command.env_remove(name);
        }
    }
    let output = command
        .arg("exec")
        .arg("--config")
        .arg(root)
        .arg("preflight-inputs")
        .current_dir(dir.path())
        .env("HOME", dir.path().join("home"))
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("CRUISE_DISABLE_NOTIFICATIONS", "1")
        .env_remove("HERDR_ENV")
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|error| panic!("run CLI: {error}"));
    assert!(
        !dir.path().join("sentinel").exists(),
        "preflight executed a command"
    );
    assert!(!output.status.success(), "missing input was accepted");
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for fragment in ["missing-label", "label"] {
        assert!(
            diagnostic.contains(fragment),
            "missing {fragment:?}: {diagnostic}"
        );
    }
}
