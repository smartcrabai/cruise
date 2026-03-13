use crate::config::{OptionItem, StepConfig, StringOrVec};
use crate::error::{CruiseError, Result};

pub mod command;
pub mod option;
pub mod prompt;

/// Discriminated union of all step kinds.
#[derive(Debug, Clone)]
pub enum StepKind {
    /// Calls an LLM via the configured command.
    Prompt(PromptStep),
    /// Runs one or more shell commands.
    Command(CommandStep),
    /// Presents an interactive selection menu.
    Option(OptionStep),
}

/// Parameters for a prompt step.
#[derive(Debug, Clone)]
pub struct PromptStep {
    pub model: Option<String>,
    pub prompt: String,
    pub instruction: Option<String>,
}

/// Parameters for a command step.
#[derive(Debug, Clone)]
pub struct CommandStep {
    /// One or more shell commands to run sequentially.
    pub command: Vec<String>,
}

/// A single choice in an option step.
#[derive(Debug, Clone)]
pub enum OptionChoice {
    /// A regular selector item.
    Selector { label: String, next: Option<String> },
    /// A free-text input item.
    TextInput { label: String, next: Option<String> },
}

impl OptionChoice {
    pub fn label(&self) -> &str {
        match self {
            OptionChoice::Selector { label, .. } | OptionChoice::TextInput { label, .. } => label,
        }
    }
}

/// Parameters for an option step.
#[derive(Debug, Clone)]
pub struct OptionStep {
    pub choices: Vec<OptionChoice>,
    /// Template string that resolves to a file path; contents shown as context.
    pub plan: Option<String>,
}

impl TryFrom<StepConfig> for StepKind {
    type Error = CruiseError;

    fn try_from(config: StepConfig) -> Result<Self> {
        // Prompt step: `prompt` field is present.
        if let Some(prompt) = config.prompt {
            return Ok(StepKind::Prompt(PromptStep {
                model: config.model,
                prompt,
                instruction: config.instruction,
            }));
        }

        // Command step: `command` field is present without `option`.
        if let Some(cmd) = config.command
            && config.option.is_none()
        {
            let commands = match cmd {
                StringOrVec::Single(s) => vec![s],
                StringOrVec::Multiple(v) => v,
            };
            if commands.is_empty() {
                return Err(CruiseError::InvalidStepConfig(
                    "command step must have at least one command".to_string(),
                ));
            }
            return Ok(StepKind::Command(CommandStep { command: commands }));
        }

        // Option step: `option` field is present.
        if let Some(items) = config.option {
            let choices = items_to_choices(items)?;
            return Ok(StepKind::Option(OptionStep {
                choices,
                plan: config.plan,
            }));
        }

        Err(CruiseError::InvalidStepConfig(
            "step must have a prompt, command, or option field".to_string(),
        ))
    }
}

/// Convert a list of `OptionItem` into `OptionChoice` values.
fn items_to_choices(items: Vec<OptionItem>) -> Result<Vec<OptionChoice>> {
    items
        .into_iter()
        .map(|item| {
            if item.selector.is_some() && item.text_input.is_some() {
                return Err(CruiseError::InvalidStepConfig(
                    "option item must have either 'selector' or 'text-input', not both".to_string(),
                ));
            }
            if let Some(label) = item.selector {
                Ok(OptionChoice::Selector {
                    label,
                    next: item.next,
                })
            } else if let Some(label) = item.text_input {
                Ok(OptionChoice::TextInput {
                    label,
                    next: item.next,
                })
            } else {
                Err(CruiseError::InvalidStepConfig(
                    "option item must have a 'selector' or 'text-input' field".to_string(),
                ))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{IfCondition, OptionItem, StepConfig, StringOrVec};

    fn make_prompt_step() -> StepConfig {
        StepConfig {
            prompt: Some("Hello {input}".to_string()),
            model: Some("claude-opus-4-5".to_string()),
            instruction: Some("Be helpful".to_string()),
            ..Default::default()
        }
    }

    fn make_command_step() -> StepConfig {
        StepConfig {
            command: Some(StringOrVec::Single("cargo test".to_string())),
            ..Default::default()
        }
    }

    fn make_option_step() -> StepConfig {
        StepConfig {
            option: Some(vec![
                OptionItem {
                    selector: Some("Continue".to_string()),
                    text_input: None,
                    next: Some("next_step".to_string()),
                },
                OptionItem {
                    selector: Some("Cancel".to_string()),
                    text_input: None,
                    next: None,
                },
            ]),
            plan: Some("{plan}".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_prompt_step_conversion() {
        let config = make_prompt_step();
        let kind = StepKind::try_from(config).unwrap_or_else(|e| panic!("{e:?}"));
        match kind {
            StepKind::Prompt(step) => {
                assert_eq!(step.prompt, "Hello {input}");
                assert_eq!(step.model, Some("claude-opus-4-5".to_string()));
                assert_eq!(step.instruction, Some("Be helpful".to_string()));
            }
            _ => panic!("Expected Prompt step"),
        }
    }

    #[test]
    fn test_command_step_conversion_single() {
        let config = make_command_step();
        let kind = StepKind::try_from(config).unwrap_or_else(|e| panic!("{e:?}"));
        match kind {
            StepKind::Command(step) => {
                assert_eq!(step.command, vec!["cargo test"]);
            }
            _ => panic!("Expected Command step"),
        }
    }

    #[test]
    fn test_command_step_conversion_multiple() {
        let config = StepConfig {
            command: Some(StringOrVec::Multiple(vec![
                "cargo fmt".to_string(),
                "cargo test".to_string(),
            ])),
            ..Default::default()
        };
        let kind = StepKind::try_from(config).unwrap_or_else(|e| panic!("{e:?}"));
        match kind {
            StepKind::Command(step) => {
                assert_eq!(step.command, vec!["cargo fmt", "cargo test"]);
            }
            _ => panic!("Expected Command step"),
        }
    }

    #[test]
    fn test_option_step_conversion() {
        let config = make_option_step();
        let kind = StepKind::try_from(config).unwrap_or_else(|e| panic!("{e:?}"));
        match kind {
            StepKind::Option(step) => {
                assert_eq!(step.choices.len(), 2);
                match &step.choices[0] {
                    OptionChoice::Selector { label, next } => {
                        assert_eq!(label, "Continue");
                        assert_eq!(next, &Some("next_step".to_string()));
                    }
                    OptionChoice::TextInput { .. } => panic!("Expected Selector"),
                }
                match &step.choices[1] {
                    OptionChoice::Selector { next, .. } => {
                        assert_eq!(next, &None);
                    }
                    OptionChoice::TextInput { .. } => panic!("Expected Selector"),
                }
            }
            StepKind::Prompt(_) | StepKind::Command(_) => panic!("Expected Option step"),
        }
    }

    #[test]
    fn test_text_input_choice_conversion() {
        let config = StepConfig {
            option: Some(vec![OptionItem {
                selector: None,
                text_input: Some("Enter text".to_string()),
                next: Some("next".to_string()),
            }]),
            ..Default::default()
        };
        let kind = StepKind::try_from(config).unwrap_or_else(|e| panic!("{e:?}"));
        match kind {
            StepKind::Option(step) => {
                assert_eq!(step.choices.len(), 1);
                match &step.choices[0] {
                    OptionChoice::TextInput { label, next } => {
                        assert_eq!(label, "Enter text");
                        assert_eq!(next, &Some("next".to_string()));
                    }
                    OptionChoice::Selector { .. } => panic!("Expected TextInput choice"),
                }
            }
            StepKind::Prompt(_) | StepKind::Command(_) => panic!("Expected Option step"),
        }
    }

    #[test]
    fn test_option_item_both_fields_error() {
        // error if both selector and text_input are present
        let config = StepConfig {
            option: Some(vec![OptionItem {
                selector: Some("Pick".to_string()),
                text_input: Some("Enter".to_string()),
                next: None,
            }]),
            ..Default::default()
        };
        let err = StepKind::try_from(config)
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        assert!(matches!(err, CruiseError::InvalidStepConfig(_)));
    }

    #[test]
    fn test_command_step_empty_list_error() {
        // empty command list is an error
        let config = StepConfig {
            command: Some(StringOrVec::Multiple(vec![])),
            ..Default::default()
        };
        let err = StepKind::try_from(config)
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        assert!(matches!(err, CruiseError::InvalidStepConfig(_)));
    }

    #[test]
    fn test_invalid_option_item() {
        let config = StepConfig {
            option: Some(vec![OptionItem {
                selector: None,
                text_input: None,
                next: None,
            }]),
            ..Default::default()
        };
        let err = StepKind::try_from(config)
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        assert!(matches!(err, CruiseError::InvalidStepConfig(_)));
    }

    #[test]
    fn test_invalid_step_conversion() {
        let config = StepConfig::default();
        let err = StepKind::try_from(config)
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        assert!(matches!(err, CruiseError::InvalidStepConfig(_)));
    }

    #[test]
    fn test_prompt_takes_priority_over_command() {
        // When both prompt and command are present, prompt wins.
        let config = StepConfig {
            prompt: Some("Hello".to_string()),
            command: Some(StringOrVec::Single("cargo test".to_string())),
            ..Default::default()
        };
        let kind = StepKind::try_from(config).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(matches!(kind, StepKind::Prompt(_)));
    }

    #[test]
    fn test_step_with_if_condition() {
        let config = StepConfig {
            command: Some(StringOrVec::Single("git commit".to_string())),
            if_condition: Some(IfCondition {
                file_changed: Some("implement".to_string()),
                no_file_changes: None,
            }),
            ..Default::default()
        };
        // IfCondition does not affect StepKind conversion; the engine handles it.
        let kind = StepKind::try_from(config).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(matches!(kind, StepKind::Command(_)));
    }
}
