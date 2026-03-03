use crate::config::StepConfig;
use crate::error::{CruiseError, Result};

pub mod command;
pub mod option;
pub mod prompt;

/// Discriminated union of all step kinds.
#[derive(Debug, Clone)]
pub enum StepKind {
    /// Calls an LLM via the configured command.
    Prompt(PromptStep),
    /// Runs a shell command.
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
    pub output: Option<String>,
    pub description: Option<String>,
}

/// Parameters for a command step.
#[derive(Debug, Clone)]
pub struct CommandStep {
    pub command: String,
    pub description: Option<String>,
}

/// Parameters for an option step.
#[derive(Debug, Clone)]
pub struct OptionStep {
    pub options: Vec<crate::config::OptionConfig>,
    pub text_input: Option<crate::config::TextInputConfig>,
    pub description: Option<String>,
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
                output: config.output,
                description: config.description,
            }));
        }

        // Command step: `command` field is present without `option`.
        if let Some(command) = config.command {
            if config.option.is_none() {
                return Ok(StepKind::Command(CommandStep {
                    command,
                    description: config.description,
                }));
            }
        }

        // Option step: `option` field is present.
        if let Some(options) = config.option {
            return Ok(StepKind::Option(OptionStep {
                options,
                text_input: config.text_input,
                description: config.description,
            }));
        }

        // Option step with text-input only.
        if let Some(text_input) = config.text_input {
            return Ok(StepKind::Option(OptionStep {
                options: vec![],
                text_input: Some(text_input),
                description: config.description,
            }));
        }

        Err(CruiseError::InvalidStepConfig(
            "step must have a prompt, command, or option field".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{IfCondition, OptionConfig, StepConfig, TextInputConfig};

    fn make_prompt_step() -> StepConfig {
        StepConfig {
            prompt: Some("Hello {input}".to_string()),
            model: Some("claude-opus-4-5".to_string()),
            instruction: Some("Be helpful".to_string()),
            output: Some("plan".to_string()),
            description: Some("Planning step".to_string()),
            ..Default::default()
        }
    }

    fn make_command_step() -> StepConfig {
        StepConfig {
            command: Some("cargo test".to_string()),
            description: Some("Run tests".to_string()),
            ..Default::default()
        }
    }

    fn make_option_step() -> StepConfig {
        StepConfig {
            option: Some(vec![
                OptionConfig {
                    label: "Continue".to_string(),
                    next: Some("next_step".to_string()),
                },
                OptionConfig {
                    label: "Cancel".to_string(),
                    next: None,
                },
            ]),
            description: Some("Choose an option".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_prompt_step_conversion() {
        let config = make_prompt_step();
        let kind = StepKind::try_from(config).unwrap();
        match kind {
            StepKind::Prompt(step) => {
                assert_eq!(step.prompt, "Hello {input}");
                assert_eq!(step.model, Some("claude-opus-4-5".to_string()));
                assert_eq!(step.instruction, Some("Be helpful".to_string()));
                assert_eq!(step.output, Some("plan".to_string()));
            }
            _ => panic!("Expected Prompt step"),
        }
    }

    #[test]
    fn test_command_step_conversion() {
        let config = make_command_step();
        let kind = StepKind::try_from(config).unwrap();
        match kind {
            StepKind::Command(step) => {
                assert_eq!(step.command, "cargo test");
                assert_eq!(step.description, Some("Run tests".to_string()));
            }
            _ => panic!("Expected Command step"),
        }
    }

    #[test]
    fn test_option_step_conversion() {
        let config = make_option_step();
        let kind = StepKind::try_from(config).unwrap();
        match kind {
            StepKind::Option(step) => {
                assert_eq!(step.options.len(), 2);
                assert_eq!(step.options[0].label, "Continue");
                assert_eq!(step.options[0].next, Some("next_step".to_string()));
                assert_eq!(step.options[1].next, None);
            }
            _ => panic!("Expected Option step"),
        }
    }

    #[test]
    fn test_text_input_only_step_conversion() {
        let config = StepConfig {
            text_input: Some(TextInputConfig {
                label: "Enter text".to_string(),
                next: Some("next".to_string()),
            }),
            description: Some("Text input step".to_string()),
            ..Default::default()
        };
        let kind = StepKind::try_from(config).unwrap();
        match kind {
            StepKind::Option(step) => {
                assert!(step.options.is_empty());
                assert!(step.text_input.is_some());
            }
            _ => panic!("Expected Option step"),
        }
    }

    #[test]
    fn test_invalid_step_conversion() {
        let config = StepConfig {
            description: Some("Only description".to_string()),
            ..Default::default()
        };
        let err = StepKind::try_from(config).unwrap_err();
        matches!(err, CruiseError::InvalidStepConfig(_));
    }

    #[test]
    fn test_prompt_takes_priority_over_command() {
        // When both prompt and command are present, prompt wins.
        let config = StepConfig {
            prompt: Some("Hello".to_string()),
            command: Some("cargo test".to_string()),
            ..Default::default()
        };
        let kind = StepKind::try_from(config).unwrap();
        matches!(kind, StepKind::Prompt(_));
    }

    #[test]
    fn test_step_with_if_condition() {
        let config = StepConfig {
            command: Some("git commit".to_string()),
            if_condition: Some(IfCondition {
                file_changed: Some("implement".to_string()),
            }),
            ..Default::default()
        };
        // IfCondition does not affect StepKind conversion; the engine handles it.
        let kind = StepKind::try_from(config).unwrap();
        matches!(kind, StepKind::Command(_));
    }
}
