use cruise::config::WorkflowConfig;
use cruise::error::CruiseError;
use cruise::workflow::compile;
use serde_json::json;

fn config(extra: &str) -> WorkflowConfig {
    WorkflowConfig::from_yaml(&format!("{extra}\nsteps:\n  main:\n    command: echo ok\n")).unwrap()
}

#[test]
fn template_keys_round_trip_and_null_is_unset() {
    let value = serde_json::to_value(config(
        "commit_message_template: 'feat: {title}'\npr_body_template: '{body}'",
    ))
    .unwrap();
    assert_eq!(value["commit_message_template"], json!("feat: {title}"));
    assert_eq!(value["pr_body_template"], json!("{body}"));
    let nulls = serde_json::to_value(config(
        "commit_message_template: null\npr_body_template: null",
    ))
    .unwrap();
    assert!(nulls.get("commit_message_template").is_none());
    assert!(nulls.get("pr_body_template").is_none());
}

#[test]
fn published_schema_advertises_template_keys() {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../cruise-schema.json")).unwrap();
    for key in ["commit_message_template", "pr_body_template"] {
        assert!(
            schema["properties"].get(key).is_some(),
            "missing schema property {key}"
        );
    }
}

#[test]
fn allowed_variables_and_escaped_braces_compile() {
    for yaml in [
        "commit_message_template: 'feat: {title} {issue} {session_id} {branch}'",
        "pr_body_template: '{title} {body} {issue} {session_id} {branch}'",
        "commit_message_template: '{{title}} {session_id}'",
    ] {
        assert!(compile(config(yaml)).is_ok(), "{yaml}");
    }
}

#[test]
fn disallowed_variables_are_rejected_with_context() {
    for (key, variable) in [
        ("commit_message_template", "body"),
        ("pr_body_template", "plan"),
        ("commit_message_template", "input"),
        ("pr_body_template", "input"),
        ("commit_message_template", "prev.output"),
        ("pr_body_template", "prev.output"),
        ("commit_message_template", "pr.url"),
        ("pr_body_template", "pr.url"),
        ("commit_message_template", "file:x"),
        ("pr_body_template", "file:x"),
        ("commit_message_template", "inputs.x"),
        ("pr_body_template", "inputs.x"),
        ("commit_message_template", "model"),
        ("pr_body_template", "model"),
        ("pr_body_template", "pr.language"),
        ("commit_message_template", "plan.language"),
    ] {
        let yaml = format!("{key}: '{{{variable}}}'");
        match compile(config(&yaml)) {
            Err(CruiseError::ConfigParseError(message)) => {
                assert!(
                    message.contains(key)
                        && message.contains(variable)
                        && message.contains("allowed:"),
                    "{message}"
                );
            }
            other => panic!("expected config error for {yaml}, got {other:?}"),
        }
    }
}

#[test]
fn malformed_template_syntax_keeps_existing_error_types() {
    for key in ["commit_message_template", "pr_body_template"] {
        for (value, empty) in [("{}", true), ("{title", false), ("title}", false)] {
            let result = compile(config(&format!("{key}: '{value}'")));
            assert!(
                if empty {
                    matches!(result, Err(CruiseError::EmptyVariableReference))
                } else {
                    matches!(result, Err(CruiseError::InvalidTemplateSyntax(_)))
                },
                "{key}: {value}"
            );
        }
    }
}
