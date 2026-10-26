//! Public configuration and published-schema contracts for per-prompt tool restrictions.
//! Fixtures deliberately use YAML and observable JSON, not proposed Rust fields or types.
use cruise::config::{WorkflowConfig, validate_config};
use cruise::error::CruiseError;
use cruise::workflow_call::resolve_workflow_calls_without_app_model_defaults;
use cruise::workflow_defaults::WorkflowDefaults;
use serde_json::{Value, json};
use std::sync::OnceLock;
use tempfile::TempDir;

fn parse(yaml: &str) -> WorkflowConfig {
    WorkflowConfig::from_yaml(yaml).unwrap_or_else(|error| panic!("parse fixture: {error}\n{yaml}"))
}

fn data(config: &WorkflowConfig) -> Value {
    serde_json::to_value(config).unwrap_or_else(|error| panic!("serialize config: {error}"))
}

fn valid(yaml: &str) {
    validate_config(&parse(yaml))
        .unwrap_or_else(|error| panic!("valid fixture rejected: {error}\n{yaml}"));
}

fn invalid_result(result: cruise::error::Result<()>, fragments: &[&str]) {
    match result {
        Err(CruiseError::InvalidStepConfig(message)) => {
            for fragment in fragments {
                assert!(
                    message.contains(fragment),
                    "missing {fragment:?} in {message:?}"
                );
            }
        }
        other => panic!("expected InvalidStepConfig, got {other:?}"),
    }
}

fn invalid(yaml: &str, fragments: &[&str]) {
    invalid_result(validate_config(&parse(yaml)), fragments);
}

#[test]
fn restrictions_roundtrip_as_strings_at_workflow_and_prompt_levels() {
    let config = parse(
        "sdk: jcode\ndisallowed_tools: [web, subagent, mcp, 'mcp:github', 'shell:npm publish']\nmcp_servers:\n  github:\n    command: github-mcp\nsteps:\n  review:\n    prompt: review\n    disallowed_tools: [web, 'shell:gh pr merge']\n",
    );
    validate_config(&config).unwrap_or_else(|error| panic!("valid restrictions: {error}"));
    let original = data(&config);
    assert_eq!(
        original["disallowed_tools"],
        json!(["web", "subagent", "mcp", "mcp:github", "shell:npm publish"])
    );
    assert_eq!(
        original["steps"]["review"]["disallowed_tools"],
        json!(["web", "shell:gh pr merge"])
    );
    let yaml =
        serde_yaml::to_string(&config).unwrap_or_else(|error| panic!("serialize YAML: {error}"));
    assert_eq!(data(&parse(&yaml)), original);
}

#[test]
fn omitted_restrictions_are_not_serialized() {
    let value = data(&parse("steps:\n  review:\n    prompt: review\n"));
    assert!(value.get("disallowed_tools").is_none());
    assert!(value["steps"]["review"].get("disallowed_tools").is_none());
}

#[test]
fn explicit_empty_workflow_restrictions_remain_distinct_from_omission() {
    let value = data(&parse(
        "disallowed_tools: []\nsteps:\n  review:\n    prompt: review\n    disallowed_tools: []\n",
    ));
    assert_eq!(value["disallowed_tools"], json!([]));
    assert!(value["steps"]["review"].get("disallowed_tools").is_none());
}

macro_rules! invalid_case {
    ($name:ident, $yaml:expr, [$($fragment:expr),*]) => {
        #[test]
        fn $name() {
            invalid($yaml, &[$($fragment),*]);
        }
    };
}

invalid_case!(
    command_step_rejects_restrictions,
    "sdk: jcode\nsteps:\n  run:\n    command: echo ok\n    disallowed_tools: [web]\n",
    ["disallowed_tools", "run"]
);
invalid_case!(
    option_step_rejects_restrictions,
    "sdk: jcode\nsteps:\n  choose:\n    option:\n      - selector: done\n        next: end\n    disallowed_tools: [web]\n",
    ["disallowed_tools", "choose"]
);
invalid_case!(
    parallel_wrapper_rejects_restrictions,
    "sdk: jcode\nsteps:\n  checks:\n    disallowed_tools: [web]\n    parallel:\n      child:\n        prompt: check\n",
    ["disallowed_tools", "checks"]
);
invalid_case!(
    parallel_command_child_rejects_restrictions,
    "sdk: jcode\nsteps:\n  checks:\n    parallel:\n      child:\n        command: echo ok\n        disallowed_tools: [web]\n",
    ["disallowed_tools", "child"]
);
invalid_case!(
    group_caller_rejects_restrictions,
    "sdk: jcode\ngroups:\n  shared:\n    steps:\n      inner:\n        prompt: review\nsteps:\n  run:\n    group: shared\n    disallowed_tools: [web]\n",
    ["disallowed_tools", "run"]
);
invalid_case!(
    squash_merge_rejects_restrictions,
    "sdk: jcode\nflags:\n  merge: {description: merge changes}\nsteps:\n  run:\n    prompt: review\nafter-pr:\n  merge:\n    squash-merge: true\n    flag: merge\n    disallowed_tools: [web]\n",
    ["disallowed_tools", "merge"]
);

#[test]
fn workflow_call_site_rejects_restrictions() {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("tempdir: {error}"));
    // A real local callee prevents an unrelated missing-file error masking the contract.
    std::fs::write(
        dir.path().join("callee.yaml"),
        "steps:\n  inner:\n    prompt: review\n",
    )
    .unwrap_or_else(|error| panic!("write callee: {error}"));
    let config = parse(
        "sdk: jcode\nsteps:\n  call:\n    workflow_call: ./callee.yaml\n    disallowed_tools: [web]\n",
    );
    let result = resolve_workflow_calls_without_app_model_defaults(
        config,
        dir.path(),
        &WorkflowDefaults::default(),
    );
    invalid_result(result.map(|_| ()), &["call", "disallowed_tools"]);
}

// Separate expansions ensure a traversal failure cannot hide another position.
invalid_case!(
    unknown_workflow_mcp_is_rejected,
    "sdk: jcode\ndisallowed_tools: ['mcp:missing']\nsteps:\n  run:\n    prompt: review\n",
    ["missing", "mcp"]
);
invalid_case!(
    unknown_step_mcp_is_rejected,
    "sdk: jcode\nsteps:\n  run:\n    prompt: review\n    disallowed_tools: ['mcp:missing']\n",
    ["missing", "run"]
);
invalid_case!(
    unknown_after_pr_mcp_is_rejected,
    "sdk: jcode\nsteps:\n  run:\n    prompt: review\nafter-pr:\n  post:\n    prompt: review\n    disallowed_tools: ['mcp:missing']\n",
    ["missing", "post"]
);
invalid_case!(
    unknown_group_inner_mcp_is_rejected,
    "sdk: jcode\nsteps:\n  run:\n    group: shared\ngroups:\n  shared:\n    steps:\n      inner:\n        prompt: review\n        disallowed_tools: ['mcp:missing']\n",
    ["missing", "inner"]
);
invalid_case!(
    unknown_parallel_prompt_child_mcp_is_rejected,
    "sdk: jcode\nsteps:\n  checks:\n    parallel:\n      child:\n        prompt: review\n        disallowed_tools: ['mcp:missing']\n",
    ["missing", "child"]
);
invalid_case!(
    unknown_after_pr_parallel_prompt_child_mcp_is_rejected,
    "sdk: jcode\nsteps:\n  run:\n    prompt: review\nafter-pr:\n  post:\n    parallel:\n      child:\n        prompt: review\n        disallowed_tools: ['mcp:missing']\n",
    ["missing", "child"]
);
invalid_case!(
    unknown_group_parallel_prompt_child_mcp_is_rejected,
    "sdk: jcode\nsteps:\n  run:\n    group: shared\ngroups:\n  shared:\n    steps:\n      checks:\n        parallel:\n          child:\n            prompt: review\n            disallowed_tools: ['mcp:missing']\n",
    ["missing", "child"]
);

macro_rules! valid_case {
    ($name:ident, $yaml:expr) => {
        #[test]
        fn $name() {
            valid($yaml);
        }
    };
}

valid_case!(
    known_mcp_restrictions_are_valid,
    "sdk: jcode\ndisallowed_tools: ['mcp:github']\nmcp_servers:\n  github:\n    command: github-mcp\nsteps:\n  run:\n    prompt: review\n    disallowed_tools: ['mcp:github']\n"
);
valid_case!(
    after_pr_prompt_restrictions_are_valid,
    "sdk: jcode\nsteps:\n  run:\n    prompt: review\nafter-pr:\n  post:\n    prompt: review\n    disallowed_tools: [web]\n"
);
valid_case!(
    group_inner_prompt_restrictions_are_valid,
    "sdk: jcode\nsteps:\n  run:\n    group: shared\ngroups:\n  shared:\n    steps:\n      inner:\n        prompt: review\n        disallowed_tools: [web, subagent, mcp]\n"
);
valid_case!(
    parallel_prompt_child_restrictions_are_valid,
    "sdk: jcode\nsteps:\n  checks:\n    parallel:\n      child:\n        prompt: review\n        disallowed_tools: [web]\n"
);
valid_case!(
    after_pr_parallel_prompt_child_restrictions_are_valid,
    "sdk: jcode\nsteps:\n  run:\n    prompt: review\nafter-pr:\n  checks:\n    parallel:\n      child:\n        prompt: review\n        disallowed_tools: [subagent]\n"
);
valid_case!(
    group_parallel_prompt_child_restrictions_are_valid,
    "sdk: jcode\nsteps:\n  run:\n    group: shared\ngroups:\n  shared:\n    steps:\n      checks:\n        parallel:\n          child:\n            prompt: review\n            disallowed_tools: [web]\n"
);

#[test]
fn github_review_after_pr_restrictions_are_valid_and_roundtrip() {
    let config = parse(
        "sdk: jcode\nsteps:\n  run:\n    prompt: implement\nafter-pr:\n  review:\n    github-review:\n      bots: ['coderabbitai[bot]']\n    prompt: address review feedback\n    timeout: 30m\n    disallowed_tools: [web, subagent, 'shell:npm publish']\n",
    );
    validate_config(&config)
        .unwrap_or_else(|error| panic!("valid github-review restrictions rejected: {error}"));
    let original = data(&config);
    assert_eq!(
        original["after-pr"]["review"]["disallowed_tools"],
        json!(["web", "subagent", "shell:npm publish"])
    );
    let yaml = serde_yaml::to_string(&config)
        .unwrap_or_else(|error| panic!("serialize github-review YAML: {error}"));
    let restored = parse(&yaml);
    validate_config(&restored)
        .unwrap_or_else(|error| panic!("restored github-review restrictions rejected: {error}"));
    assert_eq!(data(&restored), original);
}

#[test]
fn restrictions_accept_prompt_file_steps() {
    // Config validation does not read prompt files, so no filesystem dependency is needed.
    valid("sdk: jcode\nsteps:\n  run:\n    prompt_file: review.md\n    disallowed_tools: [web]\n");
}

invalid_case!(
    command_backend_rejects_explicit_workflow_restrictions,
    "command: [echo]\ndisallowed_tools: [web]\nsteps:\n  run:\n    prompt: review\n",
    ["disallowed_tools", "command"]
);
invalid_case!(
    command_backend_rejects_explicit_step_restrictions_even_with_allow_commit,
    "command: [echo]\nsteps:\n  run:\n    prompt: review\n    allow_commit: true\n    disallowed_tools: [web]\n",
    ["disallowed_tools", "command"]
);
invalid_case!(
    command_backend_rejects_after_pr_explicit_restrictions,
    "command: [echo]\nsteps:\n  run:\n    prompt: review\nafter-pr:\n  post:\n    prompt: review\n    disallowed_tools: [web]\n",
    ["disallowed_tools", "command"]
);
invalid_case!(
    command_backend_rejects_group_explicit_restrictions,
    "command: [echo]\nsteps:\n  run:\n    group: shared\ngroups:\n  shared:\n    steps:\n      inner:\n        prompt: review\n        disallowed_tools: [web]\n",
    ["disallowed_tools", "command"]
);
invalid_case!(
    command_backend_rejects_parallel_prompt_explicit_restrictions,
    "command: [echo]\nsteps:\n  checks:\n    parallel:\n      child:\n        prompt: review\n        disallowed_tools: [web]\n",
    ["disallowed_tools", "command"]
);

#[test]
fn command_backend_accepts_explicit_empty_restrictions() {
    valid(
        "command: [echo]\ndisallowed_tools: []\nsteps:\n  run:\n    prompt: review\n    disallowed_tools: []\n",
    );
}

fn schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        serde_json::from_str(include_str!("../cruise-schema.json"))
            .unwrap_or_else(|error| panic!("published schema: {error}"))
    })
}

fn schema_validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        jsonschema::validator_for(schema())
            .unwrap_or_else(|error| panic!("compile published schema: {error}"))
    })
}

#[test]
fn schema_and_deserialization_agree_on_shell_prefix_whitespace() {
    for (restriction, canonical) in [
        ("shell:git push", Some("shell:git push")),
        ("shell:  git   push  ", Some("shell:git push")),
        (
            "shell:/usr/bin/npm publish --tag=next @scope/pkg+test_1.0%20",
            Some("shell:/usr/bin/npm publish --tag=next @scope/pkg+test_1.0%20"),
        ),
        ("shell:", None),
        ("shell:   ", None),
        ("shell:git\tpush", None),
        ("shell:git push\n", None),
        ("shell:git\rpush", None),
        ("shell:git;push", None),
        ("shell:git *", None),
        ("web\n", None),
    ] {
        for workflow_level in [true, false] {
            let mut document = json!({"sdk":"jcode", "steps":{"run":{"prompt":"review"}}});
            let target = if workflow_level {
                &mut document
            } else {
                &mut document["steps"]["run"]
            };
            target["disallowed_tools"] = json!([restriction]);
            assert_eq!(
                schema_validator().is_valid(&document),
                canonical.is_some(),
                "schema: {document}"
            );
            let yaml = serde_yaml::to_string(&document).unwrap_or_else(|error| panic!("{error}"));
            let parsed = WorkflowConfig::from_yaml(&yaml);
            assert_eq!(parsed.is_ok(), canonical.is_some(), "parser: {document}");
            if let Some(canonical) = canonical {
                let config = parsed.unwrap_or_else(|error| panic!("{error}"));
                validate_config(&config).unwrap_or_else(|error| panic!("{error}"));
                let value = data(&config);
                let target = if workflow_level {
                    &value
                } else {
                    &value["steps"]["run"]
                };
                assert_eq!(target["disallowed_tools"], json!([canonical]));
                let yaml = serde_yaml::to_string(&config).unwrap_or_else(|error| panic!("{error}"));
                assert_eq!(data(&parse(&yaml)), value);
            }
        }
    }
}

#[test]
fn schema_and_runtime_agree_on_empty_and_nonempty_step_restrictions() {
    let dir = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(
        dir.path().join("callee.yaml"),
        "steps:\n  inner:\n    prompt: review\n",
    )
    .unwrap_or_else(|error| panic!("{error}"));
    for (name, step) in [
        ("command", json!({"command":"echo ok"})),
        (
            "option",
            json!({"option":[{"selector":"done", "next":"end"}]}),
        ),
        ("group", json!({"group":"shared"})),
        ("workflow_call", json!({"workflow_call":"./callee.yaml"})),
        ("parallel", json!({"parallel":{"child":{"prompt":"check"}}})),
        ("squash-merge", json!({"squash-merge":true, "flag":"merge"})),
        ("prompt", json!({"prompt":"review"})),
    ] {
        for nonempty in [false, true] {
            let mut step = step.clone();
            step["disallowed_tools"] = if nonempty { json!(["web"]) } else { json!([]) };
            let mut document = json!({
                "sdk":"jcode",
                "groups":{"shared":{"steps":{"inner":{"prompt":"review"}}}},
                "steps":{"run":{"prompt":"review"}}
            });
            if name == "squash-merge" {
                document["flags"] = json!({"merge":{"description":"merge changes"}});
                document["after-pr"] = json!({"merge":step});
            } else {
                document["steps"]["run"] = step;
            }
            let expected = !nonempty || name == "prompt";
            assert_eq!(
                schema_validator().is_valid(&document),
                expected,
                "schema: {document}"
            );
            let yaml = serde_yaml::to_string(&document).unwrap_or_else(|error| panic!("{error}"));
            let config = parse(&yaml);
            let result = if name == "workflow_call" {
                resolve_workflow_calls_without_app_model_defaults(
                    config,
                    dir.path(),
                    &WorkflowDefaults::default(),
                )
                .and_then(|resolved| validate_config(&resolved))
            } else {
                validate_config(&config)
            };
            assert_eq!(result.is_ok(), expected, "runtime: {document}: {result:?}");
        }
    }
}

fn conditional<'a>(definition: &'a Value, field: &str) -> &'a Value {
    definition["allOf"]
        .as_array()
        .unwrap_or_else(|| panic!("schema definition needs allOf"))
        .iter()
        .find(|rule| {
            rule["if"]["required"]
                .as_array()
                .is_some_and(|required| required.contains(&json!(field)))
        })
        .unwrap_or_else(|| panic!("schema needs conditional for {field}"))
}

#[test]
fn schema_exposes_string_restriction_arrays_on_workflow_and_step() {
    let schema = schema();
    for properties in [
        &schema["properties"],
        &schema["$defs"]["StepConfig"]["properties"],
    ] {
        let property = &properties["disallowed_tools"];
        assert_eq!(property["type"], "array");
        assert_eq!(property["items"]["$ref"], "#/$defs/ToolRestriction");
    }
    let restriction = &schema["$defs"]["ToolRestriction"];
    assert_eq!(restriction["type"], "string");
    assert!(
        restriction["pattern"]
            .as_str()
            .is_some_and(|pattern| !pattern.is_empty()),
        "schema must constrain restriction vocabulary rather than accept arbitrary strings"
    );
}

#[test]
fn schema_step_restrictions_require_a_prompt_form() {
    let rule = conditional(&schema()["$defs"]["StepConfig"], "disallowed_tools");
    let alternatives = rule["then"]["oneOf"]
        .as_array()
        .unwrap_or_else(|| panic!("restrictions must require a prompt form"));
    for field in ["prompt", "prompt_file"] {
        assert!(
            alternatives
                .iter()
                .any(|alternative| alternative["required"] == json!([field])),
            "schema must accept {field} restrictions"
        );
    }
}

#[test]
fn schema_step_restrictions_exclude_command_and_option_forms() {
    let rule = conditional(&schema()["$defs"]["StepConfig"], "disallowed_tools");
    for field in ["command", "option"] {
        assert_eq!(
            rule["then"]["properties"][field]["type"], "null",
            "restrictions cannot accompany {field} steps"
        );
    }
}

macro_rules! schema_placement {
    ($name:ident, $context:expr) => {
        #[test]
        fn $name() {
            let rule = conditional(&schema()["$defs"]["StepConfig"], $context);
            assert_eq!(
                rule["then"]["properties"]["disallowed_tools"]["maxItems"], 0,
                "{} must not accept nonempty restrictions",
                $context
            );
        }
    };
}
schema_placement!(
    schema_parallel_wrapper_rejects_nonempty_restrictions,
    "parallel"
);
schema_placement!(schema_group_caller_rejects_nonempty_restrictions, "group");
schema_placement!(
    schema_workflow_call_rejects_nonempty_restrictions,
    "workflow_call"
);
schema_placement!(
    schema_squash_merge_rejects_nonempty_restrictions,
    "squash-merge"
);

#[test]
fn schema_parallel_prompt_children_expose_restrictions() {
    assert_eq!(
        schema()["$defs"]["ParallelChild"]["properties"]["disallowed_tools"]["$ref"],
        "#/$defs/StepConfig/properties/disallowed_tools"
    );
}

#[test]
fn schema_parallel_command_children_reject_nonempty_restrictions() {
    let rule = conditional(&schema()["$defs"]["ParallelChild"], "command");
    assert_eq!(
        rule["then"]["properties"]["disallowed_tools"]["maxItems"],
        0
    );
}
