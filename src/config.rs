use indexmap::IndexMap;
use serde::Deserialize;
use std::path::PathBuf;

/// Top-level workflow configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct WorkflowConfig {
    /// LLM invocation command (e.g. ["claude", "-p"]).
    pub command: Vec<String>,

    /// File path bound to the `plan` variable.
    pub plan: Option<PathBuf>,

    /// Step definitions. IndexMap preserves YAML key order.
    pub steps: IndexMap<String, StepConfig>,
}

/// Per-step configuration. All fields are optional.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct StepConfig {
    /// Model to use (prompt steps only).
    pub model: Option<String>,

    /// Prompt body (prompt steps only).
    pub prompt: Option<String>,

    /// System prompt (prompt steps only).
    pub instruction: Option<String>,

    /// Variable name to store LLM output into (prompt steps only).
    pub output: Option<String>,

    /// Human-readable description displayed during execution.
    pub description: Option<String>,

    /// List of choices (option steps only).
    pub option: Option<Vec<OptionConfig>>,

    /// Free-text input configuration (option steps only).
    #[serde(rename = "text-input")]
    pub text_input: Option<TextInputConfig>,

    /// Shell command to run (command steps only).
    pub command: Option<String>,

    /// Explicit next step name, overriding sequential order.
    pub next: Option<String>,

    /// When true, always skip this step.
    pub skip: Option<bool>,

    /// Conditional execution rule.
    #[serde(rename = "if")]
    pub if_condition: Option<IfCondition>,
}

/// A single choice in an option step.
#[derive(Debug, Deserialize, Clone)]
pub struct OptionConfig {
    /// Display label shown to the user.
    pub label: String,

    /// Step to go to when selected (None = end of workflow).
    pub next: Option<String>,
}

/// Free-text input configuration for an option step.
#[derive(Debug, Deserialize, Clone)]
pub struct TextInputConfig {
    /// Prompt label shown to the user.
    pub label: String,

    /// Step to go to after input.
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
      - label: "Approve and continue"
        next: implement
      - label: "Revise the plan"
        next: planning
    text-input:
      label: "Other (text input)"
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
    fn test_command_step_fields() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap();
        let run_tests = config.steps.get("run_tests").unwrap();
        assert_eq!(run_tests.command, Some("cargo test".to_string()));
    }

    #[test]
    fn test_option_step_fields() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap();
        let review = config.steps.get("review_plan").unwrap();
        let options = review.option.as_ref().unwrap();
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].label, "Approve and continue");
        assert_eq!(options[0].next, Some("implement".to_string()));
        assert_eq!(options[1].next, Some("planning".to_string()));

        let text_input = review.text_input.as_ref().unwrap();
        assert_eq!(text_input.label, "Other (text input)");
        assert_eq!(text_input.next, Some("planning".to_string()));
    }

    #[test]
    fn test_if_condition_fields() {
        let config = WorkflowConfig::from_yaml(SAMPLE_YAML).unwrap();
        let commit = config.steps.get("commit").unwrap();
        let if_cond = commit.if_condition.as_ref().unwrap();
        assert_eq!(if_cond.file_changed, Some("implement".to_string()));
    }

    #[test]
    fn test_skip_field() {
        let yaml = r#"
command: [claude, -p]
steps:
  optional_step:
    command: cargo fmt
    skip: true
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        let step = config.steps.get("optional_step").unwrap();
        assert_eq!(step.skip, Some(true));
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
}
