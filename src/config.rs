use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DEFAULT_PR_LANGUAGE: &str = "English";

/// Top-level workflow configuration.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WorkflowConfig {
    /// LLM invocation command (e.g. `["claude", "--model", "{model}", "-p"]`).
    pub command: Vec<String>,

    /// Default model for prompt steps (e.g. "sonnet"). Per-step model overrides this.
    pub model: Option<String>,

    /// Model to use for the built-in plan step (falls back to `model`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_model: Option<String>,

    /// Language to use for built-in PR title/body generation.
    #[serde(default = "default_pr_language")]
    pub pr_language: String,

    /// Environment variables applied to all steps.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Group definitions. Groups share if conditions and `max_retries`.
    #[serde(default)]
    pub groups: HashMap<String, GroupConfig>,

    /// Step definitions. `IndexMap` preserves YAML key order.
    pub steps: IndexMap<String, StepConfig>,

    /// Steps to run after PR creation. Same format as `steps`.
    #[serde(default, rename = "after-pr")]
    pub after_pr: IndexMap<String, StepConfig>,
}

/// A command value that can be either a single string or a list of strings.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum StringOrVec {
    Single(String),
    Multiple(Vec<String>),
}

/// Skip condition: static boolean or a variable reference.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub enum SkipCondition {
    /// Always skip (true) or never skip (false).
    Static(bool),
    /// Skip if the named variable resolves to "true".
    Variable(String),
}

/// Per-step configuration. All fields are optional.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct StepConfig {
    /// Model to use (prompt steps only).
    pub model: Option<String>,

    /// Prompt body (prompt steps only).
    pub prompt: Option<String>,

    /// Message displayed to the user before this step runs (prompt steps only).
    pub instruction: Option<String>,

    /// Plan file path to display as context in option steps.
    pub plan: Option<String>,

    /// List of choices (option steps only).
    pub option: Option<Vec<OptionItem>>,

    /// Shell command(s) to run (command steps only).
    pub command: Option<StringOrVec>,

    /// Explicit next step name, overriding sequential order.
    pub next: Option<String>,

    /// Skip condition: static bool or variable reference.
    pub skip: Option<SkipCondition>,

    /// Conditional execution rule.
    #[serde(rename = "if")]
    pub if_condition: Option<IfCondition>,

    /// Environment variables applied to this step (overrides top-level env).
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Group this step belongs to.
    pub group: Option<String>,

    /// If true, the step fails immediately when no tracked file changes are detected.
    #[serde(default, rename = "fail-if-no-file-changes")]
    pub fail_if_no_file_changes: bool,
}

/// A single item in an option step.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OptionItem {
    /// Selector label shown in the menu.
    pub selector: Option<String>,

    /// Free-text input label (shows a text prompt when selected).
    #[serde(rename = "text-input")]
    pub text_input: Option<String>,

    /// Step to go to when this item is selected (None = end of workflow).
    pub next: Option<String>,
}

/// Conditional execution rule.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct IfCondition {
    /// Only execute this step if the given step's snapshot differs from the current state.
    #[serde(rename = "file-changed")]
    pub file_changed: Option<String>,
}

/// Group configuration for grouping related steps.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct GroupConfig {
    /// Conditional execution rule applied to the entire group.
    #[serde(rename = "if")]
    pub if_condition: Option<IfCondition>,

    /// Maximum number of retries for this group before skipping.
    pub max_retries: Option<usize>,

    /// Steps that belong to this group (new explicit-block style).
    #[serde(default)]
    pub steps: IndexMap<String, StepConfig>,
}

fn default_pr_language() -> String {
    DEFAULT_PR_LANGUAGE.to_string()
}

impl WorkflowConfig {
    /// Parse a workflow config from a YAML string.
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }

    /// Build the built-in default workflow config in code (no YAML file required).
    pub fn default_builtin() -> Self {
        let mut steps = IndexMap::new();

        steps.insert(
            "write-tests".to_string(),
            StepConfig {
                prompt: Some(include_str!("../prompts/write-test-first.md").to_string()),
                ..Default::default()
            },
        );

        steps.insert(
            "implement".to_string(),
            StepConfig {
                prompt: Some(include_str!("../prompts/implement-after-tests.md").to_string()),
                ..Default::default()
            },
        );

        Self {
            command: vec![
                "claude".to_string(),
                "--model".to_string(),
                "{model}".to_string(),
                "-p".to_string(),
            ],
            model: Some("sonnet".to_string()),
            plan_model: Some("opus".to_string()),
            pr_language: default_pr_language(),
            env: HashMap::new(),
            groups: HashMap::new(),
            steps,
            after_pr: IndexMap::new(),
        }
    }
}

/// Validate that `fail-if-no-file-changes` is not used in `after-pr` steps.
///
/// `after-pr` steps are executed in a warning-only context: any error is
/// downgraded to a printed warning and the workflow continues.  A step with
/// `fail-if-no-file-changes: true` would therefore never abort the run as
/// intended.  Reject it explicitly at validation time instead.
pub fn validate_fail_if_no_file_changes(config: &WorkflowConfig) -> crate::error::Result<()> {
    use crate::error::CruiseError;
    for (name, step) in &config.after_pr {
        if step.fail_if_no_file_changes {
            return Err(CruiseError::InvalidStepConfig(format!(
                "step '{name}' in after-pr uses fail-if-no-file-changes, which is not supported in after-pr steps"
            )));
        }
    }
    Ok(())
}

/// Validate group configuration:
/// - All step `group` references must point to defined groups.
/// - Steps with a group must not have individual `if` conditions.
/// - Steps inside group definitions must not have nested group references or individual `if` conditions.
pub fn validate_groups(config: &WorkflowConfig) -> crate::error::Result<()> {
    validate_step_groups(&config.steps, &config.groups)?;
    validate_step_groups(&config.after_pr, &config.groups)?;
    validate_group_inner_steps(&config.groups)?;
    Ok(())
}

fn validate_step_groups(
    steps: &IndexMap<String, StepConfig>,
    groups: &std::collections::HashMap<String, GroupConfig>,
) -> crate::error::Result<()> {
    use crate::error::CruiseError;

    for (step_name, step) in steps {
        if let Some(group_name) = step.group.as_deref() {
            if !groups.contains_key(group_name) {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "step '{step_name}' references undefined group '{group_name}'"
                )));
            }
            if step.if_condition.is_some() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "step '{step_name}' has both a group and an individual 'if' condition; use only the group's 'if'"
                )));
            }
        }
    }

    Ok(())
}

fn validate_group_inner_steps(
    groups: &std::collections::HashMap<String, GroupConfig>,
) -> crate::error::Result<()> {
    use crate::error::CruiseError;

    for (group_name, group) in groups {
        for (sub_name, sub_step) in &group.steps {
            if sub_step.group.is_some() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "nested group call inside group '{group_name}' at step '{sub_name}' is not allowed"
                )));
            }
            if sub_step.if_condition.is_some() {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "group step '{group_name}/{sub_name}' has an individual 'if' condition, \
                     which is not allowed inside group steps"
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_YAML: &str = r#"
command:
  - claude
  - -p

steps:
  planning:
    model: claude-opus-4-5
    instruction: "You are a senior engineer."
    prompt: "Plan the implementation of: {input}"

  review_plan:
    plan: "{plan}"
    option:
      - selector: "Approve and continue"
        next: implement
      - selector: "Revise the plan"
        next: planning
      - text-input: "Other (text input)"
        next: planning

  implement:
    prompt: "Implement based on the plan: {plan}"

  run_tests:
    command: cargo test

  commit:
    command: "git commit -am 'feat: {input}'"
    if:
      file-changed: implement
"#;

    #[test]
    fn test_parse_workflow_config() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.command, vec!["claude", "-p"]);
        assert_eq!(config.model, None);
        assert_eq!(config.plan_model, None);
        assert_eq!(config.pr_language, DEFAULT_PR_LANGUAGE);
    }

    #[test]
    fn test_plan_model_field() {
        let yaml = r"
command: [claude, -p]
model: sonnet
plan_model: opus
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.model, Some("sonnet".to_string()));
        assert_eq!(config.plan_model, Some("opus".to_string()));
    }

    #[test]
    fn test_pr_language_field() {
        let yaml = r"
command: [claude, -p]
pr_language: Japanese
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.pr_language, "Japanese");
    }

    #[test]
    fn test_pr_language_defaults_to_english_when_omitted() {
        let yaml = r"
command: [claude, -p]
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.pr_language, DEFAULT_PR_LANGUAGE);
    }

    #[test]
    fn test_step_order_preserved() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        let step_names: Vec<&str> = config.steps.keys().map(std::string::String::as_str).collect();
        assert_eq!(
            step_names,
            vec![
                "planning",
                "review_plan",
                "implement",
                "run_tests",
                "commit"
            ]
        );
    }

    #[test]
    fn test_prompt_step_fields() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        let planning = config.steps.get("planning").unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(planning.model, Some("claude-opus-4-5".to_string()));
        assert_eq!(
            planning.instruction,
            Some("You are a senior engineer.".to_string())
        );
        assert!(planning.prompt.is_some());
    }

    #[test]
    fn test_command_step_single() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        let run_tests = config.steps.get("run_tests").unwrap_or_else(|| panic!("unexpected None"));
        match run_tests.command.as_ref().unwrap_or_else(|| panic!("unexpected None")) {
            StringOrVec::Single(s) => assert_eq!(s, "cargo test"),
            StringOrVec::Multiple(_) => panic!("Expected Single command"),
        }
    }

    #[test]
    fn test_command_list_field() {
        let yaml = r"
command: [claude, -p]
steps:
  multi:
    command:
      - cargo fmt
      - cargo test
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config.steps.get("multi").unwrap_or_else(|| panic!("unexpected None"));
        match step.command.as_ref().unwrap_or_else(|| panic!("unexpected None")) {
            StringOrVec::Multiple(cmds) => {
                assert_eq!(cmds.len(), 2);
                assert_eq!(cmds[0], "cargo fmt");
                assert_eq!(cmds[1], "cargo test");
            }
            StringOrVec::Single(_) => panic!("Expected Multiple commands"),
        }
    }

    #[test]
    fn test_option_step_fields() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        let review = config.steps.get("review_plan").unwrap_or_else(|| panic!("unexpected None"));
        let options = review.option.as_ref().unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(options.len(), 3);
        assert_eq!(
            options[0].selector,
            Some("Approve and continue".to_string())
        );
        assert_eq!(options[0].next, Some("implement".to_string()));
        assert_eq!(options[1].next, Some("planning".to_string()));
        assert_eq!(
            options[2].text_input,
            Some("Other (text input)".to_string())
        );
        assert_eq!(options[2].next, Some("planning".to_string()));
    }

    #[test]
    fn test_if_condition_fields() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap_or_else(|e| panic!("{e:?}"));
        let commit = config.steps.get("commit").unwrap_or_else(|| panic!("unexpected None"));
        let if_cond = commit.if_condition.as_ref().unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(if_cond.file_changed, Some("implement".to_string()));
    }

    #[test]
    fn test_skip_static_field() {
        let yaml = r"
command: [claude, -p]
steps:
  optional_step:
    command: cargo fmt
    skip: true
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config.steps.get("optional_step").unwrap_or_else(|| panic!("unexpected None"));
        assert!(matches!(step.skip, Some(SkipCondition::Static(true))));
    }

    #[test]
    fn test_skip_variable_field() {
        let yaml = r"
command: [claude, -p]
steps:
  conditional_skip:
    command: cargo fmt
    skip: prev.success
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let step = config.steps.get("conditional_skip").unwrap_or_else(|| panic!("unexpected None"));
        match &step.skip {
            Some(SkipCondition::Variable(name)) => assert_eq!(name, "prev.success"),
            _ => panic!("Expected Variable skip condition"),
        }
    }

    #[test]
    fn test_top_level_env() {
        let yaml = r"
command: [claude, -p]
env:
  ANTHROPIC_API_KEY: sk-test
  PROJECT_NAME: myproject
steps:
  step1:
    command: echo hello
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            config.env.get("ANTHROPIC_API_KEY"),
            Some(&"sk-test".to_string())
        );
        assert_eq!(
            config.env.get("PROJECT_NAME"),
            Some(&"myproject".to_string())
        );
    }

    #[test]
    fn test_step_level_env() {
        let yaml = r"
command: [claude, -p]
steps:
  build:
    command: cargo build
    env:
      RUST_LOG: debug
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let build = config.steps.get("build").unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(build.env.get("RUST_LOG"), Some(&"debug".to_string()));
    }

    #[test]
    fn test_env_defaults_empty() {
        let yaml = r"
command: [claude, -p]
steps:
  step1:
    command: echo hello
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(config.env.is_empty());
        let step = config.steps.get("step1").unwrap_or_else(|| panic!("unexpected None"));
        assert!(step.env.is_empty());
    }

    #[test]
    fn test_minimal_config() {
        let yaml = r#"
command: [claude, -p]
steps:
  only_step:
    prompt: "Hello {input}"
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(config.steps.len(), 1);
    }

    #[test]
    fn test_parse_cruise_yaml() {
        let yaml = include_str!("../cruise.yaml");
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("failed to parse cruise.yaml: {e:?}"));
        assert_eq!(config.command, vec!["claude", "--model", "{model}", "-p"]);
        assert_eq!(config.model, Some("sonnet".to_string()));
        assert!(!config.steps.is_empty(), "steps is empty");
    }

    #[test]
    fn test_empty_steps() {
        let yaml = "command: [echo]\nsteps: {}";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(config.steps.is_empty());
    }

    #[test]
    fn test_default_builtin_config() {
        let config = WorkflowConfig::default_builtin();
        assert_eq!(config.command, vec!["claude", "--model", "{model}", "-p"]);
        assert_eq!(config.model, Some("sonnet".to_string()));
        assert_eq!(config.plan_model, Some("opus".to_string()));
        assert_eq!(config.pr_language, DEFAULT_PR_LANGUAGE);
        assert_eq!(config.steps.len(), 2);

        let write_test = config.steps.get("write-tests").unwrap_or_else(|| panic!("unexpected None"));
        assert!(write_test.prompt.as_deref().unwrap_or_else(|| panic!("unexpected None")).contains("{plan}"));

        let implement = config.steps.get("implement").unwrap_or_else(|| panic!("unexpected None"));
        assert!(implement.prompt.as_deref().unwrap_or_else(|| panic!("unexpected None")).contains("{plan}"));
    }

    #[test]
    fn test_default_builtin_serializes_pr_language() {
        let yaml = serde_yaml::to_string(&WorkflowConfig::default_builtin()).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(yaml.contains("pr_language: English"));
    }

    #[test]
    fn test_missing_steps_error() {
        let yaml = "command: [echo]";
        let result = WorkflowConfig::from_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_command_type_mismatch() {
        let yaml = "command: [echo]\nsteps:\n  s1:\n    command: {foo: bar}";
        let result = WorkflowConfig::from_yaml(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_fields_ignored() {
        // Old configs with `state` or `worktree` fields should still parse.
        let yaml = "command: [echo]\nworktree: true\nstate: .cruise/state.json\nsteps:\n  s1:\n    command: echo hi";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!config.steps.is_empty());
    }

    #[test]
    fn test_group_config_parse() {
        let yaml = r"
command: [claude, -p]
groups:
  review:
    if:
      file-changed: test
    max_retries: 3
steps:
  test:
    command: cargo test
  simplify:
    group: review
    prompt: /simplify
  ai-antipattern:
    group: review
    prompt: /ai-antipattern
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(config.groups.contains_key("review"));
        let review = &config.groups["review"];
        assert_eq!(review.max_retries, Some(3));
        assert!(review.if_condition.is_some());
        assert_eq!(
            review.if_condition.as_ref().unwrap_or_else(|| panic!("unexpected None")).file_changed,
            Some("test".to_string())
        );
        let simplify = config.steps.get("simplify").unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(simplify.group, Some("review".to_string()));
    }

    #[test]
    fn test_validate_groups_ok() {
        let yaml = r"
command: [claude, -p]
groups:
  review:
    max_retries: 2
    steps:
      simplify:
        prompt: /simplify
      ai-antipattern:
        prompt: /ai-antipattern
steps:
  build:
    command: cargo build
  review-pass:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(validate_groups(&config).is_ok());
    }

    #[test]
    fn test_validate_groups_undefined_group() {
        let yaml = r"
command: [claude, -p]
groups: {}
steps:
  step1:
    group: nonexistent
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_groups(&config);
        assert!(result.is_err());
        assert!(result.map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})")).to_string().contains("undefined group"));
    }

    #[test]
    fn test_validate_groups_multiple_call_sites_ok() {
        // New-style: same group invoked from multiple non-consecutive call sites is valid
        let yaml = r"
command: [claude, -p]
groups:
  review:
    max_retries: 2
    steps:
      simplify:
        prompt: /simplify
steps:
  test1:
    command: cargo test --lib
  review-after-lib:
    group: review
  test2:
    command: cargo test --doc
  review-after-doc:
    group: review
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(validate_groups(&config).is_ok());
    }

    #[test]
    fn test_validate_groups_step_has_individual_if() {
        let yaml = r"
command: [claude, -p]
groups:
  review:
    max_retries: 2
    steps:
      step1:
        command: echo hi
steps:
  call-review:
    group: review
    if:
      file-changed: step1
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_groups(&config);
        assert!(result.is_err());
        assert!(result.map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})")).to_string().contains("individual 'if'"));
    }

    #[test]
    fn test_after_pr_field_parse() {
        // Given: YAML with after-pr steps containing pr.number / pr.url placeholders
        let yaml = r#"
command: [claude, -p]
steps:
  implement:
    prompt: "Implement: {input}"
  test:
    command: cargo test
after-pr:
  notify:
    command: "echo 'PR #{pr.number} created: {pr.url}'"
  label:
    command: "gh pr edit {pr.number} --add-label enhancement"
"#;
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: after_pr has 2 steps in order
        assert_eq!(config.after_pr.len(), 2);
        let keys: Vec<&str> = config.after_pr.keys().map(std::string::String::as_str).collect();
        assert_eq!(keys, vec!["notify", "label"]);
    }

    #[test]
    fn test_after_pr_field_default_empty() {
        // Given: YAML without after-pr field
        let yaml = r#"
command: [claude, -p]
steps:
  implement:
    prompt: "Implement: {input}"
"#;
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: after_pr defaults to empty IndexMap
        assert!(config.after_pr.is_empty());
    }

    #[test]
    fn test_after_pr_step_fields() {
        // Given: YAML where after-pr step uses command field
        let yaml = r#"
command: [claude, -p]
steps:
  build:
    command: cargo build
after-pr:
  notify:
    command: "echo done"
"#;
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: after_pr step has the command field set
        let notify = config.after_pr.get("notify").unwrap_or_else(|| panic!("unexpected None"));
        match notify.command.as_ref().unwrap_or_else(|| panic!("unexpected None")) {
            StringOrVec::Single(s) => assert_eq!(s, "echo done"),
            StringOrVec::Multiple(_) => panic!("Expected Single command"),
        }
    }

    #[test]
    fn test_fail_if_no_file_changes_default_false() {
        // Given: a step without the fail-if-no-file-changes field
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: the field defaults to false
        let implement = config.steps.get("implement").unwrap_or_else(|| panic!("unexpected None"));
        assert!(!implement.fail_if_no_file_changes);
    }

    #[test]
    fn test_fail_if_no_file_changes_explicit_true() {
        // Given: a step with fail-if-no-file-changes: true
        let yaml = r#"
command: [echo]
steps:
  implement:
    prompt: "Implement: {input}"
    fail-if-no-file-changes: true
"#;
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: the field is true
        let implement = config.steps.get("implement").unwrap_or_else(|| panic!("unexpected None"));
        assert!(implement.fail_if_no_file_changes);
    }

    #[test]
    fn test_validate_fail_if_no_file_changes_rejects_after_pr_usage() {
        // Given: an after-pr step with fail-if-no-file-changes: true
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
after-pr:
  notify:
    command: echo done
    fail-if-no-file-changes: true
";
        // When: validate_fail_if_no_file_changes is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_fail_if_no_file_changes(&config);
        // Then: returns an error because after-pr + fail-if-no-file-changes is unsupported
        assert!(result.is_err());
        assert!(
            result.map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})")).to_string().contains("after-pr"),
            "error message should mention after-pr"
        );
    }

    #[test]
    fn test_validate_fail_if_no_file_changes_ok_for_normal_steps() {
        // Given: a normal step with fail-if-no-file-changes: true (no after-pr usage)
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    fail-if-no-file-changes: true
";
        // When: validate_fail_if_no_file_changes is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_fail_if_no_file_changes(&config);
        // Then: no error
        assert!(result.is_ok());
    }

    // --- New group schema: groups.<name>.steps ---

    #[test]
    fn test_group_config_with_steps_parse() {
        // Given: YAML with groups that define steps inside them
        let yaml = r"
command: [claude, -p]
groups:
  review:
    if:
      file-changed: test
    max_retries: 3
    steps:
      simplify:
        prompt: /simplify
      coderabbit:
        prompt: /cr
steps:
  test:
    command: cargo test
  review-pass:
    group: review
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: group has steps with correct count and order
        let review = &config.groups["review"];
        assert_eq!(review.max_retries, Some(3));
        assert_eq!(review.steps.len(), 2);
        let step_names: Vec<&str> = review.steps.keys().map(std::string::String::as_str).collect();
        assert_eq!(step_names, vec!["simplify", "coderabbit"]);
    }

    #[test]
    fn test_group_call_step_parse() {
        // Given: YAML where a top-level step is a pure group call (no prompt/command)
        let yaml = r"
command: [claude, -p]
groups:
  review:
    steps:
      simplify:
        prompt: /simplify
steps:
  test:
    command: cargo test
  review-pass:
    group: review
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: group call step only has group set
        let review_pass = config.steps.get("review-pass").unwrap_or_else(|| panic!("unexpected None"));
        assert_eq!(review_pass.group, Some("review".to_string()));
        assert!(review_pass.prompt.is_none());
        assert!(review_pass.command.is_none());
    }

    #[test]
    fn test_group_call_same_group_multiple_call_sites_parse() {
        // Given: YAML where same group is invoked from two different top-level steps
        let yaml = r"
command: [claude, -p]
groups:
  review:
    steps:
      simplify:
        prompt: /simplify
steps:
  test1:
    command: cargo test --lib
  review-after-lib:
    group: review
  test2:
    command: cargo test --doc
  review-after-doc:
    group: review
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: both call sites reference the same group
        assert_eq!(
            config.steps["review-after-lib"].group,
            Some("review".to_string())
        );
        assert_eq!(
            config.steps["review-after-doc"].group,
            Some("review".to_string())
        );
        // And: step order in top-level steps is preserved
        let keys: Vec<&str> = config.steps.keys().map(std::string::String::as_str).collect();
        assert_eq!(
            keys,
            vec!["test1", "review-after-lib", "test2", "review-after-doc"]
        );
    }
}
