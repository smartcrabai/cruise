use cruise::config::{WorkflowConfig, validate_config};
use cruise::error::CruiseError;
use cruise::workflow::compile;

fn parse(yaml: &str) -> WorkflowConfig {
    WorkflowConfig::from_yaml(yaml).unwrap_or_else(|error| panic!("YAML should parse: {error}"))
}

fn assert_invalid_gate(yaml: &str, expected: &str) {
    let config = parse(yaml);
    let error =
        validate_config(&config).expect_err("quality gate configuration should be rejected");
    match error {
        CruiseError::InvalidStepConfig(message) => {
            assert!(message.contains("quality_gates"), "{message}");
            assert!(message.contains(expected), "{message}");
        }
        other => panic!("expected InvalidStepConfig, got {other:?}"),
    }
}

#[test]
fn prompt_gates_parse_validate_and_compile() {
    let config = parse(
        "command: [echo]\nsteps:\n  implement:\n    prompt: implement\n    quality_gates:\n      - command: cargo test\n      - command: cargo clippy\n        timeout: 10m\n",
    );
    validate_config(&config).expect("prompt gates should validate");
    compile(config).expect("prompt gates should compile");
}

#[test]
fn group_internal_prompt_gates_are_allowed() {
    let config = parse(
        "command: [echo]\ngroups:\n  review:\n    steps:\n      inspect:\n        prompt: inspect\n        quality_gates: [{command: 'true'}]\nsteps:\n  run:\n    group: review\n",
    );
    validate_config(&config).expect("group internal gate should validate");
    compile(config).expect("group internal gate should compile");
}

#[test]
fn empty_gate_list_is_rejected() {
    assert_invalid_gate(
        "steps:\n  implement:\n    prompt: implement\n    quality_gates: []\n",
        "at least one",
    );
}

#[test]
fn blank_gate_command_is_rejected() {
    assert_invalid_gate(
        "steps:\n  implement:\n    prompt: implement\n    quality_gates: [{command: '   '}]\n",
        "must not be blank",
    );
}

#[test]
fn zero_gate_timeout_is_rejected() {
    let config = parse(
        "steps:\n  implement:\n    prompt: implement\n    quality_gates: [{command: 'true', timeout: '0'}]\n",
    );
    assert!(
        validate_config(&config).is_err(),
        "zero timeout must be rejected"
    );
}

#[test]
fn unknown_gate_field_is_rejected_by_yaml_parser() {
    let yaml = "steps:\n  implement:\n    prompt: implement\n    quality_gates: [{command: 'true', retries: 2}]\n";
    let error = WorkflowConfig::from_yaml(yaml).expect_err("unknown gate field must be rejected");
    assert!(error.to_string().contains("retries"), "{error}");
}

#[test]
fn missing_gate_command_is_rejected_by_yaml_parser() {
    let yaml = "steps:\n  implement:\n    prompt: implement\n    quality_gates: [{timeout: 10m}]\n";
    let error = WorkflowConfig::from_yaml(yaml).expect_err("gate command is required");
    assert!(error.to_string().contains("command"), "{error}");
}

#[test]
fn gates_are_rejected_on_non_prompt_and_reserved_locations() {
    let cases = [
        (
            "command",
            "steps:\n  invalid:\n    command: 'true'\n    quality_gates: [{command: 'true'}]\n",
        ),
        (
            "option",
            "steps:\n  invalid:\n    option: [{selector: done}]\n    quality_gates: [{command: 'true'}]\n",
        ),
        (
            "parallel parent",
            "steps:\n  invalid:\n    parallel:\n      child:\n        prompt: child\n    quality_gates: [{command: 'true'}]\n",
        ),
        (
            "parallel child",
            "steps:\n  fan:\n    parallel:\n      child:\n        prompt: child\n        quality_gates: [{command: 'true'}]\n",
        ),
        (
            "after-pr",
            "steps:\n  main:\n    prompt: main\nafter-pr:\n  invalid:\n    prompt: inspect\n    quality_gates: [{command: 'true'}]\n",
        ),
        (
            "group call",
            "groups:\n  g:\n    steps:\n      child:\n        prompt: child\nsteps:\n  invalid:\n    group: g\n    quality_gates: [{command: 'true'}]\n",
        ),
        (
            "workflow call",
            "steps:\n  invalid:\n    workflow_call: ./callee.yaml\n    quality_gates: [{command: 'true'}]\n",
        ),
        (
            "squash merge",
            "flags:\n  merge: {}\nsteps:\n  main:\n    prompt: main\nafter-pr:\n  invalid:\n    squash-merge: true\n    flag: merge\n    quality_gates: [{command: 'true'}]\n",
        ),
    ];
    for (name, yaml) in cases {
        let config = parse(yaml);
        let result = validate_config(&config).and_then(|()| compile(config).map(|_| ()));
        match result {
            Err(CruiseError::InvalidStepConfig(message)) => {
                assert!(message.contains("quality_gates"), "{name}: {message}")
            }
            other => panic!("{name}: expected quality_gates InvalidStepConfig, got {other:?}"),
        }
    }
}
