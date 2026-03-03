use dialoguer::{Input, Select};

use crate::config::{OptionConfig, TextInputConfig};
use crate::error::Result;

/// Result of executing an option step.
#[derive(Debug, Clone)]
pub struct OptionResult {
    /// Next step name chosen by the user (None = end of workflow).
    pub next_step: Option<String>,

    /// Text entered by the user when the text-input option was selected.
    pub text_input: Option<String>,
}

/// Display an interactive selection menu and return the user's choice.
pub fn run_option(
    options: &[OptionConfig],
    text_input_config: Option<&TextInputConfig>,
    description: Option<&str>,
) -> Result<OptionResult> {
    if let Some(desc) = description {
        println!("\n{desc}");
    }

    // Build the label list shown to the user.
    let mut labels: Vec<String> = options.iter().map(|o| o.label.clone()).collect();

    if let Some(ti) = text_input_config {
        labels.push(ti.label.clone());
    }

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

    // Handle text-input selection.
    if let Some(ti) = text_input_config {
        if selection == options.len() {
            let text: String = Input::new()
                .with_prompt(&ti.label)
                .interact_text()
                .map_err(|e| crate::error::CruiseError::Other(format!("input error: {e}")))?;

            return Ok(OptionResult {
                next_step: ti.next.clone(),
                text_input: Some(text),
            });
        }
    }

    let selected = &options[selection];
    Ok(OptionResult {
        next_step: selected.next.clone(),
        text_input: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{OptionConfig, TextInputConfig};

    #[test]
    fn test_option_config_structure() {
        let options = vec![
            OptionConfig {
                label: "Option A".to_string(),
                next: Some("step_a".to_string()),
            },
            OptionConfig {
                label: "Option B".to_string(),
                next: None,
            },
        ];

        assert_eq!(options[0].label, "Option A");
        assert_eq!(options[0].next, Some("step_a".to_string()));
        assert_eq!(options[1].next, None);
    }

    #[test]
    fn test_text_input_config_structure() {
        let config = TextInputConfig {
            label: "Enter text".to_string(),
            next: Some("next_step".to_string()),
        };

        assert_eq!(config.label, "Enter text");
        assert_eq!(config.next, Some("next_step".to_string()));
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
