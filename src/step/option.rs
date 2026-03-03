use dialoguer::{Input, Select};

use crate::error::Result;
use crate::step::OptionChoice;

/// Result of executing an option step.
#[derive(Debug, Clone)]
pub struct OptionResult {
    /// Next step name chosen by the user (None = end of workflow).
    pub next_step: Option<String>,

    /// Text entered by the user when a text-input choice was selected.
    pub text_input: Option<String>,
}

/// Display an interactive selection menu and return the user's choice.
pub fn run_option(choices: &[OptionChoice], description: Option<&str>) -> Result<OptionResult> {
    if let Some(desc) = description {
        println!("\n{desc}");
    }

    // Build the label list shown to the user.
    let labels: Vec<&str> = choices.iter().map(|c| c.label()).collect();

    if labels.is_empty() {
        // Nothing to select — continue to the next step.
        return Ok(OptionResult {
            next_step: None,
            text_input: None,
        });
    }

    let selection = Select::new()
        .with_prompt("Select an option")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(|e| crate::error::CruiseError::Other(format!("selection error: {e}")))?;

    match &choices[selection] {
        OptionChoice::Selector { next, .. } => Ok(OptionResult {
            next_step: next.clone(),
            text_input: None,
        }),
        OptionChoice::TextInput { label, next } => {
            let text: String = Input::new()
                .with_prompt(label)
                .interact_text()
                .map_err(|e| crate::error::CruiseError::Other(format!("input error: {e}")))?;

            Ok(OptionResult {
                next_step: next.clone(),
                text_input: Some(text),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::step::OptionChoice;

    #[test]
    fn test_option_choice_selector() {
        let choice = OptionChoice::Selector {
            label: "Option A".to_string(),
            next: Some("step_a".to_string()),
        };
        match choice {
            OptionChoice::Selector { label, next } => {
                assert_eq!(label, "Option A");
                assert_eq!(next, Some("step_a".to_string()));
            }
            _ => panic!("Expected Selector"),
        }
    }

    #[test]
    fn test_option_choice_text_input() {
        let choice = OptionChoice::TextInput {
            label: "Enter text".to_string(),
            next: Some("next_step".to_string()),
        };
        match choice {
            OptionChoice::TextInput { label, next } => {
                assert_eq!(label, "Enter text");
                assert_eq!(next, Some("next_step".to_string()));
            }
            _ => panic!("Expected TextInput"),
        }
    }

    #[test]
    fn test_option_result_with_next() {
        let result = OptionResult {
            next_step: Some("implement".to_string()),
            text_input: None,
        };
        assert_eq!(result.next_step, Some("implement".to_string()));
        assert!(result.text_input.is_none());
    }

    #[test]
    fn test_option_result_with_text_input() {
        let result = OptionResult {
            next_step: Some("planning".to_string()),
            text_input: Some("user input".to_string()),
        };
        assert_eq!(result.text_input, Some("user input".to_string()));
    }

    #[test]
    fn test_option_result_cancel() {
        let result = OptionResult {
            next_step: None,
            text_input: None,
        };
        assert!(result.next_step.is_none());
    }
}
