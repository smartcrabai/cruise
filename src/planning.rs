use std::collections::HashMap;
use std::path::Path;

use console::style;

use crate::config::WorkflowConfig;
use crate::engine::resolve_command_with_model;
use crate::error::Result;
use crate::step::prompt::{PromptResult, StreamCallbacks, run_prompt};
use crate::variable::VariableStore;

/// Resolve and execute a plan-related prompt template with the workflow's LLM command.
///
/// # Errors
///
/// Returns an error if variable resolution fails, the LLM command fails, or a rate limit is hit
/// and retries are exhausted.
pub async fn run_plan_prompt_template(
    config: &WorkflowConfig,
    vars: &mut VariableStore,
    template: &str,
    label: &str,
    rate_limit_retries: usize,
    working_dir: Option<&Path>,
    stream_callbacks: Option<&StreamCallbacks<'_>>,
) -> Result<PromptResult> {
    let prompt = vars.resolve(template)?;
    let plan_model = config.plan_model.clone().or_else(|| config.model.clone());
    let effective_model = plan_model.as_deref();
    let has_placeholder = config.command.iter().any(|s| s.contains("{model}"));
    let (resolved_command, model_arg) = if has_placeholder {
        (
            resolve_command_with_model(&config.command, effective_model),
            None,
        )
    } else {
        (config.command.clone(), effective_model.map(str::to_string))
    };

    let env = HashMap::new();
    eprintln!("\n{} {}", style("▶").cyan().bold(), style(label).bold());
    let spinner = crate::spinner::Spinner::start("Cruising...");
    let result = {
        let on_retry = |msg: &str| spinner.suspend(|| eprintln!("{msg}"));
        run_prompt(
            &resolved_command,
            model_arg.as_deref(),
            &prompt,
            rate_limit_retries,
            &env,
            Some(&on_retry),
            None,
            working_dir,
            stream_callbacks,
        )
        .await
    };
    drop(spinner);
    result
}

/// Write the user input directly as plan.md, bypassing LLM generation.
///
/// Returns the plan markdown content (== trimmed `input`).
///
/// # Errors
///
/// Returns an error if `input` is empty or whitespace-only, or if the file cannot be written.
pub fn write_input_as_plan(plan_path: &Path, input: &str) -> Result<String> {
    let content = input.trim().to_string();
    if content.is_empty() {
        return Err(crate::error::CruiseError::Other(
            "cannot use empty input as plan".to_string(),
        ));
    }
    std::fs::write(plan_path, &content)
        .map_err(|e| crate::error::CruiseError::Other(format!("failed to write plan: {e}")))?;
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_temp_dir() -> TempDir {
        TempDir::new().unwrap_or_else(|e| panic!("{e:?}"))
    }

    #[test]
    fn write_input_as_plan_writes_trimmed_content_to_file() {
        // Given: a temp dir and input with surrounding whitespace
        let dir = make_temp_dir();
        let plan_path = dir.path().join("plan.md");

        // When
        let content = write_input_as_plan(&plan_path, "  implement feature X  ")
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: trimmed content returned and written to file
        assert_eq!(content, "implement feature X");
        assert_eq!(
            std::fs::read_to_string(&plan_path).unwrap_or_else(|e| panic!("{e:?}")),
            "implement feature X"
        );
    }

    #[test]
    fn write_input_as_plan_returns_err_for_empty_input() {
        // Given: empty string input
        let dir = make_temp_dir();
        let plan_path = dir.path().join("plan.md");

        // When / Then: error returned, file not created
        assert!(write_input_as_plan(&plan_path, "").is_err());
        assert!(!plan_path.exists());
    }

    #[test]
    fn write_input_as_plan_returns_err_for_whitespace_only_input() {
        // Given: whitespace-only input (spaces, newline, tab)
        let dir = make_temp_dir();
        let plan_path = dir.path().join("plan.md");

        // When / Then: error returned
        assert!(write_input_as_plan(&plan_path, "   \n\t  ").is_err());
    }

    #[test]
    fn write_input_as_plan_preserves_multiline_markdown() {
        // Given: multi-line markdown content
        let dir = make_temp_dir();
        let plan_path = dir.path().join("plan.md");
        let input = "# Plan\n\n- step 1\n- step 2";

        // When
        let content = write_input_as_plan(&plan_path, input).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: internal newlines preserved; file content matches return value
        assert_eq!(content, "# Plan\n\n- step 1\n- step 2");
        assert_eq!(
            std::fs::read_to_string(&plan_path).unwrap_or_else(|e| panic!("{e:?}")),
            content
        );
    }

    #[test]
    fn write_input_as_plan_returns_err_on_invalid_path() {
        // Given: a path whose parent directory does not exist
        let plan_path = std::path::Path::new("/nonexistent/dir/plan.md");

        // When / Then: error returned because fs::write fails
        assert!(write_input_as_plan(plan_path, "some content").is_err());
    }
}
