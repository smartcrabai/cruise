use indexmap::IndexMap;
use serde::Deserialize;
use std::path::PathBuf;

/// Top-level workflow configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct WorkflowConfig {
    /// LLM invocation command (e.g. ["claude", "--model", "{model}", "-p"]).
    pub command: Vec<String>,

    /// Default model for prompt steps (e.g. "sonnet"). Per-step model overrides this.
    pub model: Option<String>,

    /// File path bound to the `plan` variable.
    pub plan: Option<PathBuf>,

    /// Step definitions. IndexMap preserves YAML key order.
    pub steps: IndexMap<String, StepConfig>,
}

/// A command value that can be either a single string or a list of strings.
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum StringOrVec {
    Single(String),
    Multiple(Vec<String>),
}

/// Skip condition: static boolean or a variable reference.
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum SkipCondition {
    /// Always skip (true) or never skip (false).
    Static(bool),
    /// Skip if the named variable resolves to "true".
    Variable(String),
}

/// Per-step configuration. All fields are optional.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct StepConfig {
    /// Model to use (prompt steps only).
    pub model: Option<String>,

    /// Prompt body (prompt steps only).
    pub prompt: Option<String>,

    /// Message displayed to the user before this step runs (prompt steps only).
    pub instruction: Option<String>,

    /// Variable name to store LLM output into (prompt steps only).
    pub output: Option<String>,

    /// Human-readable description displayed during execution.
    pub description: Option<String>,

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
}

/// A single item in an option step.
#[derive(Debug, Deserialize, Clone)]
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
#[derive(Debug, Deserialize, Clone)]
pub struct IfCondition {
    /// Only execute this step if the given step's snapshot differs from the current state.
    #[serde(rename = "file-changed")]
    pub file_changed: Option<String>,
}

impl WorkflowConfig {
    /// Parse a workflow config from a YAML string.
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_YAML: &str = r#"
command:
  - claude
  - -p

plan: plan.md

steps:
  planning:
    model: claude-opus-4-5
    instruction: "You are a senior engineer."
    prompt: "Plan the implementation of: {input}"
    output: plan

  review_plan:
    description: "Review the plan"
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
        assert_eq!(config.plan, Some(PathBuf::from("plan.md")));
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
        assert_eq!(planning.output, Some("plan".to_string()));
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
    fn test_minimal_config() {
        let yaml = r#"
command: [claude, -p]
steps:
  only_step:
    prompt: "Hello {input}"
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        assert_eq!(config.plan, None);
        assert_eq!(config.steps.len(), 1);
    }

    #[test]
    fn test_parse_cruise_yaml() {
        let yaml = include_str!("../cruise.yaml");
        let config = WorkflowConfig::from_yaml(yaml).expect("failed to parse cruise.yaml");
        assert_eq!(config.command, vec!["claude", "--model", "{model}", "-p"]);
        assert_eq!(config.model, Some("sonnet".to_string()));
        assert_eq!(config.plan, Some(PathBuf::from(".cruise/plan.md")));
        assert!(!config.steps.is_empty(), "steps is empty");
    }
}
