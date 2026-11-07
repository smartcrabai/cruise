use std::path::Path;

use cruise::config::WorkflowConfig;
use cruise::prompt_render::{
    COMMIT_GUARD_PROMPT_NOTICE, assemble_step_prompt, render_generate_prompt, render_plan_template,
    render_pr_template,
};
use cruise::tool_policy::ToolPolicy;
use cruise::variable::{VariableStore, referenced_names};

#[test]
fn plan_renders_path_language_and_interactive_guidance() {
    let config = WorkflowConfig::from_yaml(
        "languages:\n  plan: Japanese\nsteps:\n  example:\n    command: echo hi\n",
    )
    .unwrap();
    let path = Path::new("plans/example.md");
    for (interactive, expected) in [(false, "This run is non-interactive"), (true, "ask_user")] {
        let mut vars = VariableStore::default();
        let rendered = render_plan_template(
            "{plan}|{plan.language}|{plan.clarification}",
            &mut vars,
            &config,
            path,
            interactive,
        )
        .unwrap();
        assert!(
            rendered.starts_with("plans/example.md|Japanese|"),
            "{rendered}"
        );
        assert!(rendered.contains(expected), "{rendered}");
    }
}

#[test]
fn plan_rejects_undefined_variable() {
    let config = WorkflowConfig::from_yaml("steps:\n  example:\n    command: echo hi\n").unwrap();
    assert!(
        render_plan_template(
            "{nope}",
            &mut VariableStore::default(),
            &config,
            Path::new("plan.md"),
            false
        )
        .is_err()
    );
}

#[test]
fn pr_uses_requested_language_and_rejects_missing_variables() {
    let mut vars = VariableStore::default();
    assert_eq!(
        render_pr_template("Write in {pr.language}", &mut vars, "Japanese").unwrap(),
        "Write in Japanese"
    );
    assert!(render_pr_template("{pr.number}", &mut vars, "English").is_err());
}

#[test]
fn generate_replaces_schema_before_description() {
    assert_eq!(
        render_generate_prompt("{schema}|{description}", "schema", "literal {schema}"),
        "schema|literal {schema}"
    );
}

#[test]
fn step_commit_guard_is_conditional_and_exact() {
    let vars = VariableStore::new("task".into());
    let policy = ToolPolicy::default();
    assert_eq!(
        assemble_step_prompt("{input}", &vars, true, &policy).unwrap(),
        "task"
    );
    assert_eq!(
        assemble_step_prompt("{input}", &vars, false, &policy).unwrap(),
        format!("task{COMMIT_GUARD_PROMPT_NOTICE}")
    );
}

#[test]
fn step_appends_tool_notice_after_commit_guard() {
    let vars = VariableStore::new("task".into());
    let policy = ToolPolicy {
        deny_web: true,
        ..ToolPolicy::default()
    };
    let notice = policy
        .prompt_notice()
        .expect("web denial must produce a notice");
    assert_eq!(
        assemble_step_prompt("{input}", &vars, false, &policy).unwrap(),
        format!("task{COMMIT_GUARD_PROMPT_NOTICE}\n\n{notice}")
    );
    assert_eq!(
        assemble_step_prompt("{input}", &vars, true, &policy).unwrap(),
        format!("task\n\n{notice}")
    );
}

#[test]
fn step_file_references_are_literal_and_require_artifact_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("research")).unwrap();
    std::fs::write(dir.path().join("research/plan.md"), "artifact {x}").unwrap();
    let policy = ToolPolicy::default();
    let mut vars = VariableStore::default();
    assert!(assemble_step_prompt("{file:research/plan.md}", &vars, true, &policy).is_err());
    vars.set_named_value("x", "expanded".into());
    vars.set_artifacts_root(dir.path().to_path_buf());
    assert_eq!(
        assemble_step_prompt("before {file:research/plan.md} after", &vars, true, &policy).unwrap(),
        "before artifact {x} after"
    );
}

#[test]
fn referenced_names_uses_runtime_brace_tokenization() {
    assert_eq!(
        referenced_names("{a} {file:x.md} {{a}} }}"),
        vec!["a", "file:x.md"]
    );
    assert!(referenced_names("unclosed {").is_empty());
    assert_eq!(referenced_names("{a} } {b}"), vec!["a", "b"]);
}
