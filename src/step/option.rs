use inquire::{InquireError, Select};

use crate::error::{CruiseError, Result};
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
///
/// # Errors
///
/// Returns an error if the user interaction fails or the choices list is invalid.
pub fn run_option(choices: &[OptionChoice], description: Option<&str>) -> Result<OptionResult> {
    if let Some(desc) = description {
        crate::display::print_bordered(desc, Some("Plan"));
    }

    // Build the label list shown to the user.
    let labels: Vec<&str> = choices.iter().map(super::OptionChoice::label).collect();

    if labels.is_empty() {
        // Nothing to select — continue to the next step.
        return Ok(OptionResult {
            next_step: None,
            text_input: None,
        });
    }

    let selected_label = match Select::new("Select an option", labels).prompt() {
        Ok(label) => label,
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            return Err(CruiseError::StepPaused);
        }
        Err(e) => {
            return Err(crate::error::CruiseError::Other(format!(
                "selection error: {e}"
            )));
        }
    };

    let selection = choices
        .iter()
        .position(|c| c.label() == selected_label)
        .ok_or_else(|| crate::error::CruiseError::Other("selected item not found".to_string()))?;

    match &choices[selection] {
        OptionChoice::Selector { next, .. } => Ok(OptionResult {
            next_step: next.clone(),
            text_input: None,
        }),
        OptionChoice::TextInput { label, next } => {
            let text = crate::multiline_input::prompt_multiline(label)?.into_result()?;

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
            OptionChoice::TextInput { .. } => panic!("Expected Selector"),
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
            OptionChoice::Selector { .. } => panic!("Expected TextInput"),
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
    fn test_option_result_no_fields() {
        let result = OptionResult {
            next_step: None,
            text_input: None,
        };
        assert!(result.next_step.is_none());
    }
}
