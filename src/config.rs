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

    /// LLM API configuration for direct OpenAI-compatible API calls (optional).
    #[serde(default)]
    pub llm: Option<LlmApiConfigYaml>,
}

/// LLM API configuration loaded from the config file.
///
/// All fields are optional; missing values fall back to environment variables
/// (`CRUISE_LLM_*`) or built-in defaults.  Prefer `CRUISE_LLM_API_KEY` over
/// storing a key in the YAML file to avoid committing secrets.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct LlmApiConfigYaml {
    /// API key for the OpenAI-compatible service.
    pub api_key: Option<String>,
    /// Base URL of the API endpoint. Default: `https://api.openai.com/v1`
    pub endpoint: Option<String>,
    /// Model name to use for generation. Default: `gpt-4o`
    pub model: Option<String>,
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

/// Action to take when no tracked file changes are detected after a step.
///
/// Exactly one of `fail` or `retry` must be true.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct NoFileChangesCondition {
    /// If true, abort the workflow with an error when no file changes are detected.
    #[serde(default)]
    pub fail: bool,

    /// If true, re-execute the current step when no file changes are detected.
    #[serde(default)]
    pub retry: bool,
}

/// Conditional execution rule.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct IfCondition {
    /// Only execute this step if the given step's snapshot differs from the current state.
    #[serde(rename = "file-changed")]
    pub file_changed: Option<String>,

    /// Action to take when no tracked file changes are detected after this step.
    #[serde(rename = "no-file-changes")]
    pub no_file_changes: Option<NoFileChangesCondition>,
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
    ///
    /// # Errors
    ///
    /// Returns an error if the YAML is invalid or does not match the expected schema.
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }

    /// Build the built-in default workflow config in code (no YAML file required).
    #[must_use]
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
            llm: None,
        }
    }
}

/// Validate that `fail-if-no-file-changes` is not used in `after-pr` steps.
///
/// `after-pr` steps are executed in a warning-only context: any error is
/// downgraded to a printed warning and the workflow continues.  A step with
/// `fail-if-no-file-changes: true` would therefore never abort the run as
/// intended.  Reject it explicitly at validation time instead.
///
/// # Errors
///
/// Returns an error if any `after-pr` step uses `fail-if-no-file-changes`.
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

/// Validate `if.no-file-changes` usage across all steps and groups.
///
/// Enforces:
/// - `fail` and `retry` cannot both be true in the same `no-file-changes` object.
/// - An empty (all-false) `no-file-changes` object is rejected.
/// - `if.no-file-changes` in `after-pr` steps is rejected.
/// - `if.no-file-changes` in group-level `if` is rejected.
/// - Legacy `fail-if-no-file-changes` and new `if.no-file-changes` cannot both be set on the same step.
///
/// # Errors
///
/// Returns an error if any validation rule is violated.
pub fn validate_if_conditions(config: &WorkflowConfig) -> crate::error::Result<()> {
    use crate::error::CruiseError;

    // Reject no-file-changes at group level.
    for (group_name, group) in &config.groups {
        if let Some(ref if_cond) = group.if_condition
            && if_cond.no_file_changes.is_some()
        {
            return Err(CruiseError::InvalidStepConfig(format!(
                "group '{group_name}' uses if.no-file-changes, which is not supported at the group level",
            )));
        }
    }

    // Reject no-file-changes in after-pr steps.
    for (name, step) in &config.after_pr {
        if let Some(ref if_cond) = step.if_condition
            && if_cond.no_file_changes.is_some()
        {
            return Err(CruiseError::InvalidStepConfig(format!(
                "step '{name}' in after-pr uses if.no-file-changes, which is not supported in after-pr steps",
            )));
        }
    }

    // Validate regular steps.
    for (name, step) in &config.steps {
        // Reject legacy + new coexistence.
        if step.fail_if_no_file_changes
            && let Some(ref if_cond) = step.if_condition
            && if_cond.no_file_changes.is_some()
        {
            return Err(CruiseError::InvalidStepConfig(format!(
                "step '{name}' uses both fail-if-no-file-changes and if.no-file-changes; use only one",
            )));
        }

        if let Some(ref if_cond) = step.if_condition
            && let Some(ref nfc) = if_cond.no_file_changes
        {
            // Mutually exclusive: fail and retry cannot both be true.
            if nfc.fail && nfc.retry {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "step '{name}' if.no-file-changes has both fail and retry set to true; they are mutually exclusive",
                )));
            }
            // At least one of fail or retry must be set.
            if !nfc.fail && !nfc.retry {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "step '{name}' if.no-file-changes requires either fail or retry to be true",
                )));
            }
        }
    }

    Ok(())
}

/// Run all config validations (groups, fail-if-no-file-changes, if-conditions).
///
/// # Errors
///
/// Returns an error if any validation check fails.
pub fn validate_config(config: &WorkflowConfig) -> crate::error::Result<()> {
    validate_groups(config)?;
    validate_fail_if_no_file_changes(config)?;
    validate_if_conditions(config)?;
    Ok(())
}

/// Validate group configuration:
/// - All step `group` references must point to defined groups.
/// - Steps with a group must not have individual `if` conditions.
/// - Steps inside group definitions must not have nested group references or individual `if` conditions.
///
/// # Errors
///
/// Returns an error if any group configuration is invalid.
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
        let step_names: Vec<&str> = config
            .steps
            .keys()
            .map(std::string::String::as_str)
            .collect();
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
        let planning = config
            .steps
            .get("planning")
            .unwrap_or_else(|| panic!("unexpected None"));
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
        let run_tests = config
            .steps
            .get("run_tests")
            .unwrap_or_else(|| panic!("unexpected None"));
        match run_tests
            .command
            .as_ref()
            .unwrap_or_else(|| panic!("unexpected None"))
        {
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
        let step = config
            .steps
            .get("multi")
            .unwrap_or_else(|| panic!("unexpected None"));
        match step
            .command
            .as_ref()
            .unwrap_or_else(|| panic!("unexpected None"))
        {
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
        let review = config
            .steps
            .get("review_plan")
            .unwrap_or_else(|| panic!("unexpected None"));
        let options = review
            .option
            .as_ref()
            .unwrap_or_else(|| panic!("unexpected None"));
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
        let commit = config
            .steps
            .get("commit")
            .unwrap_or_else(|| panic!("unexpected None"));
        let if_cond = commit
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("unexpected None"));
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
        let step = config
            .steps
            .get("optional_step")
            .unwrap_or_else(|| panic!("unexpected None"));
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
        let step = config
            .steps
            .get("conditional_skip")
            .unwrap_or_else(|| panic!("unexpected None"));
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
        let build = config
            .steps
            .get("build")
            .unwrap_or_else(|| panic!("unexpected None"));
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
        let step = config
            .steps
            .get("step1")
            .unwrap_or_else(|| panic!("unexpected None"));
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
        let config = WorkflowConfig::from_yaml(yaml)
            .unwrap_or_else(|e| panic!("failed to parse cruise.yaml: {e:?}"));
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

        let write_test = config
            .steps
            .get("write-tests")
            .unwrap_or_else(|| panic!("unexpected None"));
        assert!(
            write_test
                .prompt
                .as_deref()
                .unwrap_or_else(|| panic!("unexpected None"))
                .contains("{plan}")
        );

        let implement = config
            .steps
            .get("implement")
            .unwrap_or_else(|| panic!("unexpected None"));
        assert!(
            implement
                .prompt
                .as_deref()
                .unwrap_or_else(|| panic!("unexpected None"))
                .contains("{plan}")
        );
    }

    #[test]
    fn test_default_builtin_serializes_pr_language() {
        let yaml = serde_yaml::to_string(&WorkflowConfig::default_builtin())
            .unwrap_or_else(|e| panic!("{e:?}"));
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
            review
                .if_condition
                .as_ref()
                .unwrap_or_else(|| panic!("unexpected None"))
                .file_changed,
            Some("test".to_string())
        );
        let simplify = config
            .steps
            .get("simplify")
            .unwrap_or_else(|| panic!("unexpected None"));
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
        assert!(
            result
                .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
                .to_string()
                .contains("undefined group")
        );
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
        assert!(
            result
                .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
                .to_string()
                .contains("individual 'if'")
        );
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
        let keys: Vec<&str> = config
            .after_pr
            .keys()
            .map(std::string::String::as_str)
            .collect();
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
        let notify = config
            .after_pr
            .get("notify")
            .unwrap_or_else(|| panic!("unexpected None"));
        match notify
            .command
            .as_ref()
            .unwrap_or_else(|| panic!("unexpected None"))
        {
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
        let implement = config
            .steps
            .get("implement")
            .unwrap_or_else(|| panic!("unexpected None"));
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
        let implement = config
            .steps
            .get("implement")
            .unwrap_or_else(|| panic!("unexpected None"));
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
            result
                .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
                .to_string()
                .contains("after-pr"),
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
        let step_names: Vec<&str> = review
            .steps
            .keys()
            .map(std::string::String::as_str)
            .collect();
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
        let review_pass = config
            .steps
            .get("review-pass")
            .unwrap_or_else(|| panic!("unexpected None"));
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
        let keys: Vec<&str> = config
            .steps
            .keys()
            .map(std::string::String::as_str)
            .collect();
        assert_eq!(
            keys,
            vec!["test1", "review-after-lib", "test2", "review-after-doc"]
        );
    }

    // --- if.no-file-changes parse tests ---

    #[test]
    fn test_if_no_file_changes_fail_parses() {
        // Given: a step with if.no-file-changes.fail: true
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes:
        fail: true
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: the no_file_changes condition is set with fail=true
        let implement = config
            .steps
            .get("implement")
            .unwrap_or_else(|| panic!("step not found"));
        let if_cond = implement
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("if_condition not set"));
        let no_change = if_cond
            .no_file_changes
            .as_ref()
            .unwrap_or_else(|| panic!("no_file_changes not set"));
        assert!(no_change.fail, "fail should be true");
        assert!(!no_change.retry, "retry should be false");
    }

    #[test]
    fn test_if_no_file_changes_retry_parses() {
        // Given: a step with if.no-file-changes.retry: true
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes:
        retry: true
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: the no_file_changes condition is set with retry=true
        let implement = config
            .steps
            .get("implement")
            .unwrap_or_else(|| panic!("step not found"));
        let if_cond = implement
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("if_condition not set"));
        let no_change = if_cond
            .no_file_changes
            .as_ref()
            .unwrap_or_else(|| panic!("no_file_changes not set"));
        assert!(!no_change.fail, "fail should be false");
        assert!(no_change.retry, "retry should be true");
    }

    #[test]
    fn test_if_no_file_changes_and_file_changed_coexist_in_parse() {
        // Given: a step with both if.file-changed and if.no-file-changes
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      file-changed: implement
      no-file-changes:
        retry: true
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        // Then: both fields are present
        let implement = config
            .steps
            .get("implement")
            .unwrap_or_else(|| panic!("step not found"));
        let if_cond = implement
            .if_condition
            .as_ref()
            .unwrap_or_else(|| panic!("if_condition not set"));
        assert_eq!(if_cond.file_changed, Some("implement".to_string()));
        assert!(
            if_cond
                .no_file_changes
                .as_ref()
                .unwrap_or_else(|| panic!("no_file_changes not set"))
                .retry
        );
    }

    // --- if.no-file-changes validation tests ---

    #[test]
    fn test_validate_if_conditions_rejects_fail_and_retry_both_true() {
        // Given: a step with both fail: true and retry: true
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes:
        fail: true
        retry: true
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: returns an error because fail and retry are mutually exclusive
        assert!(result.is_err(), "expected Err but got Ok");
        let msg = result.map_or_else(|e| e.to_string(), |()| panic!("expected Err"));
        assert!(
            msg.contains("fail") || msg.contains("retry"),
            "error should mention fail/retry, got: {msg}"
        );
    }

    #[test]
    fn test_validate_if_conditions_rejects_empty_no_file_changes() {
        // Given: a step with no-file-changes: {} (all defaults false)
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes: {}
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: returns an error because neither fail nor retry is set
        assert!(result.is_err(), "expected Err for empty no-file-changes");
    }

    #[test]
    fn test_validate_if_conditions_rejects_no_file_changes_in_after_pr() {
        // Given: an after-pr step with if.no-file-changes.fail: true
        let yaml = r"
command: [echo]
steps:
  build:
    command: cargo build
after-pr:
  notify:
    command: echo done
    if:
      no-file-changes:
        fail: true
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: returns an error because no-file-changes in after-pr is unsupported
        assert!(
            result.is_err(),
            "expected Err for after-pr + no-file-changes"
        );
        let msg = result.map_or_else(|e| e.to_string(), |()| panic!("expected Err"));
        assert!(
            msg.contains("after-pr") || msg.contains("notify"),
            "error should mention after-pr step, got: {msg}"
        );
    }

    #[test]
    fn test_validate_if_conditions_rejects_no_file_changes_in_group_if() {
        // Given: a group with if.no-file-changes set (group-level no-file-changes is unsupported)
        let yaml = r"
command: [echo]
groups:
  review:
    if:
      no-file-changes:
        fail: true
    steps:
      simplify:
        prompt: /simplify
steps:
  test:
    command: cargo test
  review-pass:
    group: review
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: returns an error because no-file-changes in group-level if is unsupported
        assert!(
            result.is_err(),
            "expected Err for group-level no-file-changes"
        );
        let msg = result.map_or_else(|e| e.to_string(), |()| panic!("expected Err"));
        assert!(
            msg.contains("group") || msg.contains("review"),
            "error should mention group, got: {msg}"
        );
    }

    #[test]
    fn test_validate_if_conditions_rejects_legacy_and_new_syntax_together() {
        // Given: a step with BOTH old fail-if-no-file-changes and new if.no-file-changes
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    fail-if-no-file-changes: true
    if:
      no-file-changes:
        fail: true
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: returns an error because both syntaxes cannot coexist
        assert!(
            result.is_err(),
            "expected Err when both legacy and new syntax are used"
        );
    }

    #[test]
    fn test_validate_if_conditions_ok_for_fail_true() {
        // Given: a step with if.no-file-changes.fail: true (valid)
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes:
        fail: true
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: no error
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
    }

    #[test]
    fn test_validate_if_conditions_ok_for_retry_true() {
        // Given: a step with if.no-file-changes.retry: true (valid)
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    if:
      no-file-changes:
        retry: true
  done:
    command: echo done
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: no error
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
    }

    #[test]
    fn test_validate_if_conditions_ok_for_legacy_fail_if_no_file_changes_alone() {
        // Given: a step with legacy fail-if-no-file-changes: true (no new syntax)
        let yaml = r"
command: [echo]
steps:
  implement:
    command: cargo build
    fail-if-no-file-changes: true
";
        // When: validate_if_conditions is called
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));
        let result = validate_if_conditions(&config);
        // Then: no error — legacy field alone is accepted (backward compatibility)
        assert!(
            result.is_ok(),
            "legacy fail-if-no-file-changes alone should pass validate_if_conditions, got: {result:?}"
        );
    }

    // --- JSON Schema tests ---

    fn load_schema() -> &'static serde_json::Value {
        use std::sync::OnceLock;
        static SCHEMA: OnceLock<serde_json::Value> = OnceLock::new();
        SCHEMA.get_or_init(|| {
            serde_json::from_str(include_str!("../cruise-schema.json"))
                .unwrap_or_else(|e| panic!("cruise-schema.json is not valid JSON: {e}"))
        })
    }

    /// Returns the "properties" object from a `$defs/{def_name}` definition.
    fn def_properties<'a>(
        schema: &'a serde_json::Value,
        def_name: &str,
    ) -> &'a serde_json::Map<String, serde_json::Value> {
        schema["$defs"][def_name]["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{def_name} properties not found in schema $defs"))
    }

    /// Asserts that all `expected_fields` exist as keys in `props`.
    fn assert_has_fields(
        props: &serde_json::Map<String, serde_json::Value>,
        expected_fields: &[&str],
        type_name: &str,
    ) {
        for field in expected_fields {
            assert!(
                props.contains_key(*field),
                "{type_name} schema must contain field '{field}'"
            );
        }
    }

    /// Asserts that `field_def` uses `oneOf` containing the given type variants.
    fn assert_oneof_types(
        field_def: &serde_json::Value,
        expected_types: &[&str],
        field_name: &str,
    ) {
        assert!(
            field_def.get("oneOf").is_some(),
            "{field_name} must use 'oneOf'; got: {field_def}"
        );
        let one_of = field_def["oneOf"]
            .as_array()
            .unwrap_or_else(|| panic!("{field_name} oneOf must be a JSON array"));
        for expected in expected_types {
            assert!(
                one_of.iter().any(|v| v["type"].as_str() == Some(expected)),
                "{field_name} oneOf must include '{expected}' variant"
            );
        }
    }

    #[test]
    fn test_schema_is_valid_json() {
        let schema = load_schema();
        assert!(schema.is_object(), "schema root must be a JSON object");
    }

    #[test]
    fn test_schema_has_meta_fields() {
        let schema = load_schema();
        assert!(
            schema.get("$schema").is_some(),
            "schema must have a $schema field"
        );
        assert_eq!(
            schema["type"].as_str(),
            Some("object"),
            "root type must be 'object'"
        );
        assert!(
            schema.get("properties").is_some(),
            "schema must have properties"
        );
    }

    #[test]
    fn test_schema_workflow_config_required_fields() {
        let schema = load_schema();
        let required = schema["required"]
            .as_array()
            .unwrap_or_else(|| panic!("schema must have a 'required' array"));
        assert!(
            required.iter().any(|v| v.as_str() == Some("command")),
            "'command' must be in required"
        );
        assert!(
            required.iter().any(|v| v.as_str() == Some("steps")),
            "'steps' must be in required"
        );
    }

    #[test]
    fn test_schema_workflow_config_has_expected_properties() {
        let schema = load_schema();
        let props = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("schema must have a 'properties' object"));
        assert_has_fields(
            props,
            &[
                "command",
                "model",
                "plan_model",
                "pr_language",
                "env",
                "groups",
                "steps",
                "after-pr",
            ],
            "WorkflowConfig",
        );
    }

    #[test]
    fn test_schema_command_is_array_of_strings() {
        let schema = load_schema();
        let command_prop = &schema["properties"]["command"];
        assert_eq!(
            command_prop["type"].as_str(),
            Some("array"),
            "command must have type 'array'"
        );
        assert_eq!(
            command_prop["items"]["type"].as_str(),
            Some("string"),
            "command items must have type 'string'"
        );
    }

    fn assert_object_map_property(schema: &serde_json::Value, prop_name: &str) {
        let prop = &schema["properties"][prop_name];
        assert_eq!(
            prop["type"].as_str(),
            Some("object"),
            "{prop_name} must have type 'object'"
        );
        assert!(
            prop.get("additionalProperties").is_some(),
            "{prop_name} must define additionalProperties"
        );
    }

    #[test]
    fn test_schema_steps_is_object_with_step_config() {
        let schema = load_schema();
        assert_object_map_property(schema, "steps");
    }

    #[test]
    fn test_schema_step_config_has_expected_properties() {
        let schema = load_schema();
        let step_props = def_properties(schema, "StepConfig");
        assert_has_fields(
            step_props,
            &[
                "model",
                "prompt",
                "instruction",
                "plan",
                "option",
                "command",
                "next",
                "skip",
                "if",
                "env",
                "group",
                "fail-if-no-file-changes",
            ],
            "StepConfig",
        );
    }

    #[test]
    fn test_schema_step_command_is_string_or_array() {
        let schema = load_schema();
        let step_props = def_properties(schema, "StepConfig");
        assert_oneof_types(&step_props["command"], &["string", "array"], "step command");
    }

    #[test]
    fn test_schema_step_skip_is_boolean_or_string() {
        let schema = load_schema();
        let step_props = def_properties(schema, "StepConfig");
        assert_oneof_types(&step_props["skip"], &["boolean", "string"], "step skip");
    }

    #[test]
    fn test_schema_if_condition_has_file_changed() {
        let schema = load_schema();
        let if_props = def_properties(schema, "IfCondition");
        assert_has_fields(if_props, &["file-changed"], "IfCondition");
    }

    #[test]
    fn test_schema_option_item_has_expected_properties() {
        let schema = load_schema();
        let option_item_props = def_properties(schema, "OptionItem");
        assert_has_fields(
            option_item_props,
            &["selector", "text-input", "next"],
            "OptionItem",
        );
    }

    #[test]
    fn test_schema_group_config_has_expected_properties() {
        let schema = load_schema();
        let group_props = def_properties(schema, "GroupConfig");
        assert_has_fields(group_props, &["if", "max_retries", "steps"], "GroupConfig");
    }

    #[test]
    fn test_schema_after_pr_is_object_with_step_config() {
        let schema = load_schema();
        assert_object_map_property(schema, "after-pr");
    }

    // ── LlmApiConfigYaml ─────────────────────────────────────────────────────

    #[test]
    fn test_llm_api_config_absent_when_no_llm_section() {
        // Given: workflow YAML with no `llm:` section
        let yaml = r"
command: [claude, -p]
steps:
  s1:
    command: echo hi
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: llm field defaults to None
        assert!(
            config.llm.is_none(),
            "expected config.llm to be None when llm: section is absent"
        );
    }

    #[test]
    fn test_llm_api_config_all_fields_deserialize() {
        // Given: workflow YAML with all three llm fields set
        let yaml = r#"
command: [claude, -p]
llm:
  api_key: "sk-test"
  endpoint: "https://my-api.com/v1"
  model: "gpt-4o-mini"
steps:
  s1:
    command: echo hi
"#;
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: all llm fields are present
        let llm = config
            .llm
            .unwrap_or_else(|| panic!("expected Some llm config"));
        assert_eq!(llm.api_key.as_deref(), Some("sk-test"));
        assert_eq!(llm.endpoint.as_deref(), Some("https://my-api.com/v1"));
        assert_eq!(llm.model.as_deref(), Some("gpt-4o-mini"));
    }

    #[test]
    fn test_llm_api_config_partial_fields_others_are_none() {
        // Given: workflow YAML with only `model` in the llm section
        let yaml = r"
command: [claude, -p]
llm:
  model: custom-model
steps:
  s1:
    command: echo hi
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: model is set; api_key and endpoint are None
        let llm = config
            .llm
            .unwrap_or_else(|| panic!("expected Some llm config"));
        assert!(llm.api_key.is_none(), "api_key should be None when not set");
        assert!(
            llm.endpoint.is_none(),
            "endpoint should be None when not set"
        );
        assert_eq!(llm.model.as_deref(), Some("custom-model"));
    }

    #[test]
    fn test_llm_api_config_empty_block_all_fields_none() {
        // Given: workflow YAML with an empty `llm:` block
        let yaml = r"
command: [claude, -p]
llm: {}
steps:
  s1:
    command: echo hi
";
        // When: parsed
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: llm is Some but all inner fields are None
        let llm = config
            .llm
            .unwrap_or_else(|| panic!("expected Some llm config"));
        assert!(llm.api_key.is_none());
        assert!(llm.endpoint.is_none());
        assert!(llm.model.is_none());
    }
}
