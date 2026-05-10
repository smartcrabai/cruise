use std::collections::HashMap;
use std::path::Path;

use console::style;

use crate::config::WorkflowConfig;
use crate::engine::resolve_command_with_model;
use crate::error::Result;
use crate::step::prompt::{PromptResult, run_prompt};
use crate::variable::VariableStore;

/// Resolve and execute a plan-related prompt template with the workflow's LLM command.
///
/// # Errors
///
/// Returns an error if variable resolution fails, the LLM command fails, or a rate limit is hit
/// and retries are exhausted.
#[expect(clippy::too_many_arguments)]
pub async fn run_plan_prompt_template<G: Fn(&str) + Send + Sync, H: Fn(&str) + Send + Sync>(
    config: &WorkflowConfig,
    vars: &mut VariableStore,
    template: &str,
    label: &str,
    rate_limit_retries: usize,
    working_dir: Option<&Path>,
    on_stdout: Option<&G>,
    on_stderr: Option<&H>,
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
            on_stdout,
            on_stderr,
        )
        .await
    };
    drop(spinner);
    result
}
