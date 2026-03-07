use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level workflow configuration.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct WorkflowConfig {
    /// LLM invocation command (e.g. ["claude", "--model", "{model}", "-p"]).
    pub command: Vec<String>,

    /// Default model for prompt steps (e.g. "sonnet"). Per-step model overrides this.
    pub model: Option<String>,

    /// Model to use for the built-in plan step (falls back to `model`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_model: Option<String>,

    /// Environment variables applied to all steps.
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Group definitions. Groups share if conditions and max_retries.
    #[serde(default)]
    pub groups: HashMap<String, GroupConfig>,

    /// Step definitions. IndexMap preserves YAML key order.
    pub steps: IndexMap<String, StepConfig>,
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
            "write-test".to_string(),
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
            env: HashMap::new(),
            groups: HashMap::new(),
            steps,
        }
    }
}

/// Validate group configuration:
/// - All step `group` references must point to defined groups.
/// - Steps in the same group must be consecutive in the IndexMap.
/// - Steps with a group must not have individual `if` conditions.
pub fn validate_groups(config: &WorkflowConfig) -> crate::error::Result<()> {
    use crate::error::CruiseError;
    use std::collections::HashSet;

    let mut current_group: Option<&str> = None;
    let mut seen_groups: HashSet<&str> = HashSet::new();

    for (step_name, step) in &config.steps {
        match step.group.as_deref() {
            Some(group_name) => {
                if !config.groups.contains_key(group_name) {
                    return Err(CruiseError::InvalidStepConfig(format!(
                        "step '{}' references undefined group '{}'",
                        step_name, group_name
                    )));
                }
                if step.if_condition.is_some() {
                    return Err(CruiseError::InvalidStepConfig(format!(
                        "step '{}' has both a group and an individual 'if' condition; use only the group's 'if'",
                        step_name
                    )));
                }
                if current_group != Some(group_name) {
                    if seen_groups.contains(group_name) {
                        return Err(CruiseError::InvalidStepConfig(format!(
                            "steps in group '{}' are not consecutive (step '{}' is out of order)",
                            group_name, step_name
                        )));
                    }
                    if let Some(prev) = current_group {
                        seen_groups.insert(prev);
                    }
                    current_group = Some(group_name);
                }
            }
            None => {
                if let Some(prev) = current_group.take() {
                    seen_groups.insert(prev);
                }
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
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap();
        assert_eq!(config.command, vec!["claude", "-p"]);
        assert_eq!(config.model, None);
        assert_eq!(config.plan_model, None);
    }

    #[test]
    fn test_plan_model_field() {
        let yaml = r#"
command: [claude, -p]
model: sonnet
plan_model: opus
steps:
  s1:
    command: echo hi
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.model, Some("sonnet".to_string()));
        assert_eq!(config.plan_model, Some("opus".to_string()));
    }

    #[test]
    fn test_step_order_preserved() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap();
        let step_names: Vec<&str> = config.steps.keys().map(|s| s.as_str()).collect();
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
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap();
        let planning = config.steps.get("planning").unwrap();
        assert_eq!(planning.model, Some("claude-opus-4-5".to_string()));
        assert_eq!(
            planning.instruction,
            Some("You are a senior engineer.".to_string())
        );
        assert!(planning.prompt.is_some());
    }

    #[test]
    fn test_command_step_single() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap();
        let run_tests = config.steps.get("run_tests").unwrap();
        match run_tests.command.as_ref().unwrap() {
            StringOrVec::Single(s) => assert_eq!(s, "cargo test"),
            _ => panic!("Expected Single command"),
        }
    }

    #[test]
    fn test_command_list_field() {
        let yaml = r#"
command: [claude, -p]
steps:
  multi:
    command:
      - cargo fmt
      - cargo test
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        let step = config.steps.get("multi").unwrap();
        match step.command.as_ref().unwrap() {
            StringOrVec::Multiple(cmds) => {
                assert_eq!(cmds.len(), 2);
                assert_eq!(cmds[0], "cargo fmt");
                assert_eq!(cmds[1], "cargo test");
            }
            _ => panic!("Expected Multiple commands"),
        }
    }

    #[test]
    fn test_option_step_fields() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap();
        let review = config.steps.get("review_plan").unwrap();
        let options = review.option.as_ref().unwrap();
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
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap();
        let commit = config.steps.get("commit").unwrap();
        let if_cond = commit.if_condition.as_ref().unwrap();
        assert_eq!(if_cond.file_changed, Some("implement".to_string()));
    }

    #[test]
    fn test_skip_static_field() {
        let yaml = r#"
command: [claude, -p]
steps:
  optional_step:
    command: cargo fmt
    skip: true
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        let step = config.steps.get("optional_step").unwrap();
        assert!(matches!(step.skip, Some(SkipCondition::Static(true))));
    }

    #[test]
    fn test_skip_variable_field() {
        let yaml = r#"
command: [claude, -p]
steps:
  conditional_skip:
    command: cargo fmt
    skip: prev.success
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        let step = config.steps.get("conditional_skip").unwrap();
        match &step.skip {
            Some(SkipCondition::Variable(name)) => assert_eq!(name, "prev.success"),
            _ => panic!("Expected Variable skip condition"),
        }
    }

    #[test]
    fn test_top_level_env() {
        let yaml = r#"
command: [claude, -p]
env:
  ANTHROPIC_API_KEY: sk-test
  PROJECT_NAME: myproject
steps:
  step1:
    command: echo hello
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
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
        let yaml = r#"
command: [claude, -p]
steps:
  build:
    command: cargo build
    env:
      RUST_LOG: debug
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        let build = config.steps.get("build").unwrap();
        assert_eq!(build.env.get("RUST_LOG"), Some(&"debug".to_string()));
    }

    #[test]
    fn test_env_defaults_empty() {
        let yaml = r#"
command: [claude, -p]
steps:
  step1:
    command: echo hello
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        assert!(config.env.is_empty());
        let step = config.steps.get("step1").unwrap();
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
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.steps.len(), 1);
    }

    #[test]
    fn test_parse_cruise_yaml() {
        let yaml = include_str!("../cruise.yaml");
        let config = WorkflowConfig::from_yaml(yaml).expect("failed to parse cruise.yaml");
        assert_eq!(config.command, vec!["claude", "--model", "{model}", "-p"]);
        assert_eq!(config.model, Some("sonnet".to_string()));
        assert!(!config.steps.is_empty(), "steps is empty");
    }

    #[test]
    fn test_empty_steps() {
        let yaml = "command: [echo]\nsteps: {}";
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        assert!(config.steps.is_empty());
    }

    #[test]
    fn test_default_builtin_config() {
        let config = WorkflowConfig::default_builtin();
        assert_eq!(config.command, vec!["claude", "--model", "{model}", "-p"]);
        assert_eq!(config.model, Some("sonnet".to_string()));
        assert_eq!(config.plan_model, Some("opus".to_string()));
        assert_eq!(config.steps.len(), 2);

        let write_test = config.steps.get("write-test").unwrap();
        assert!(write_test.prompt.as_deref().unwrap().contains("{plan}"));

        let implement = config.steps.get("implement").unwrap();
        assert!(implement.prompt.as_deref().unwrap().contains("{plan}"));
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
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        assert!(!config.steps.is_empty());
    }

    #[test]
    fn test_group_config_parse() {
        let yaml = r#"
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
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        assert!(config.groups.contains_key("review"));
        let review = &config.groups["review"];
        assert_eq!(review.max_retries, Some(3));
        assert!(review.if_condition.is_some());
        assert_eq!(
            review.if_condition.as_ref().unwrap().file_changed,
            Some("test".to_string())
        );
        let simplify = config.steps.get("simplify").unwrap();
        assert_eq!(simplify.group, Some("review".to_string()));
    }

    #[test]
    fn test_validate_groups_ok() {
        let yaml = r#"
command: [claude, -p]
groups:
  review:
    max_retries: 2
steps:
  build:
    command: cargo build
  simplify:
    group: review
    prompt: /simplify
  ai-antipattern:
    group: review
    prompt: /ai-antipattern
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        assert!(validate_groups(&config).is_ok());
    }

    #[test]
    fn test_validate_groups_undefined_group() {
        let yaml = r#"
command: [claude, -p]
groups: {}
steps:
  step1:
    group: nonexistent
    command: echo hi
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        let result = validate_groups(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("undefined group"));
    }

    #[test]
    fn test_validate_groups_non_consecutive() {
        let yaml = r#"
command: [claude, -p]
groups:
  review:
    max_retries: 2
steps:
  step_a:
    group: review
    command: echo a
  step_b:
    command: echo b
  step_c:
    group: review
    command: echo c
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        let result = validate_groups(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not consecutive"));
    }

    #[test]
    fn test_validate_groups_step_has_individual_if() {
        let yaml = r#"
command: [claude, -p]
groups:
  review:
    max_retries: 2
steps:
  step1:
    group: review
    command: echo hi
    if:
      file-changed: step1
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        let result = validate_groups(&config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("individual 'if'"));
    }
}
