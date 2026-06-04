use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use console::style;

use crate::ask_handler::AskHandler;
use crate::config::WorkflowConfig;
use crate::error::Result;
use crate::executor::{Executor, PromptRun};
use crate::step::prompt::{PromptResult, StreamCallbacks};
use crate::variable::VariableStore;

// Built-in plan/fix/ask prompt templates, embedded at compile time. The `*_SDK`
// variants drive the agent via the `submit_plan` / `update_plan` / `ask_user`
// tools instead of writing the plan file directly. Shared by the CLI
// (`plan_cmd`) and the GUI (`src-tauri`) so the two never drift.
pub const PLAN_PROMPT_TEMPLATE: &str = include_str!("../prompts/plan.md");
pub const FIX_PLAN_PROMPT_TEMPLATE: &str = include_str!("../prompts/fix-plan.md");
pub const ASK_PLAN_PROMPT_TEMPLATE: &str = include_str!("../prompts/ask-plan.md");
pub const PLAN_PROMPT_TEMPLATE_SDK: &str = include_str!("../prompts/plan-sdk.md");
pub const FIX_PLAN_PROMPT_TEMPLATE_SDK: &str = include_str!("../prompts/fix-plan-sdk.md");
pub const ASK_PLAN_PROMPT_TEMPLATE_SDK: &str = include_str!("../prompts/ask-plan-sdk.md");

/// Select the plan-generation template for the configured backend.
#[must_use]
pub fn plan_template(config: &WorkflowConfig) -> &'static str {
    if config.sdk.is_some() {
        PLAN_PROMPT_TEMPLATE_SDK
    } else {
        PLAN_PROMPT_TEMPLATE
    }
}

/// Select the fix-plan template for the configured backend.
#[must_use]
pub fn fix_plan_template(config: &WorkflowConfig) -> &'static str {
    if config.sdk.is_some() {
        FIX_PLAN_PROMPT_TEMPLATE_SDK
    } else {
        FIX_PLAN_PROMPT_TEMPLATE
    }
}

/// Select the ask-plan template for the configured backend.
#[must_use]
pub fn ask_plan_template(config: &WorkflowConfig) -> &'static str {
    if config.sdk.is_some() {
        ASK_PLAN_PROMPT_TEMPLATE_SDK
    } else {
        ASK_PLAN_PROMPT_TEMPLATE
    }
}

/// Backend-stable context for a plan-related prompt.
///
/// Bundles the workflow config, the interactive UI handler (used by the SDK
/// `ask_user` tool), the session plan path (target of `submit_plan` /
/// `update_plan`), and execution settings. Built once per planning session and
/// reused across the plan / fix / ask turns.
pub struct PlanPromptCtx<'a> {
    /// Workflow configuration (selects command vs SDK backend, model/mode keys).
    pub config: &'a WorkflowConfig,
    /// UI handler backing the SDK `ask_user` tool (CLI or GUI).
    pub ask: Arc<dyn AskHandler>,
    /// Session `plan.md` path (where SDK plan tools read/write).
    pub plan_path: &'a Path,
    /// Whether the user can be reached interactively (controls which SDK tools
    /// are registered). Non-TTY runs pass `false`.
    pub interactive: bool,
    /// Maximum rate-limit retries.
    pub rate_limit_retries: usize,
    /// Working directory for the command / agent.
    pub working_dir: Option<&'a Path>,
}

impl PlanPromptCtx<'_> {
    /// Build the executor for this planning context.
    #[must_use]
    fn executor(&self) -> Executor {
        Executor::new(self.config.sdk.as_deref(), &self.config.command)
    }
}

/// Resolve and execute a plan-related prompt template on the configured backend.
///
/// In SDK mode, when `register_plan_tools` is true the planning tools
/// (`ask_user` / `submit_plan` / `update_plan`) are injected so the agent can
/// write the plan. Read-only turns (the "Ask about the plan" flow) pass `false`
/// so no plan-writing tool is available and the saved plan cannot be mutated;
/// the agent answers from the plan it reads with pi's built-in tools.
///
/// `resume` carries the seher session id forward so the plan/fix/ask turns share
/// one conversation. `resume` is updated in place with the session id produced by
/// this turn (left untouched in command mode).
///
/// # Errors
///
/// Returns an error if variable resolution fails, the backend fails, or a rate
/// limit is hit and retries are exhausted.
pub async fn run_plan_prompt_template(
    ctx: &PlanPromptCtx<'_>,
    vars: &mut VariableStore,
    template: &str,
    label: &str,
    stream_callbacks: Option<&StreamCallbacks<'_>>,
    resume: &mut Option<String>,
    register_plan_tools: bool,
) -> Result<PromptResult> {
    let prompt = vars.resolve(template)?;
    let executor = ctx.executor();
    let model_or_mode = executor.plan_model_or_mode(
        ctx.config.plan_model.as_deref(),
        ctx.config.model.as_deref(),
    );
    let tools = if executor.is_sdk() && register_plan_tools {
        crate::sdk_tools::planning_tools(
            ctx.plan_path.to_path_buf(),
            Arc::clone(&ctx.ask),
            ctx.interactive,
        )
    } else {
        Vec::new()
    };

    let env = HashMap::new();
    eprintln!("\n{} {}", style("▶").cyan().bold(), style(label).bold());
    // The SDK backend surfaces progress through streamed deltas and `ask_user`
    // prompts, so a spinner would clobber interactive input; only spin for the
    // command backend.
    let spinner = (!executor.is_sdk()).then(|| crate::spinner::Spinner::start("Cruising..."));
    let on_retry = move |msg: &str| eprintln!("{msg}");
    let outcome = executor
        .run(PromptRun {
            prompt: &prompt,
            model_or_mode: model_or_mode.as_deref(),
            max_retries: ctx.rate_limit_retries,
            env: &env,
            on_retry: Some(&on_retry),
            cancel_token: None,
            working_dir: ctx.working_dir,
            stream: stream_callbacks,
            tools,
            resume: resume.clone(),
        })
        .await;
    drop(spinner);

    let outcome = outcome?;
    if outcome.session_id.is_some() {
        *resume = outcome.session_id;
    }
    Ok(outcome.result)
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

    // -- template selection ---------------------------------------------------

    fn config_with(sdk: Option<&str>, command: Option<&str>) -> WorkflowConfig {
        let mut yaml = String::new();
        if let Some(s) = sdk {
            yaml.push_str("sdk: ");
            yaml.push_str(s);
            yaml.push('\n');
        }
        if let Some(c) = command {
            yaml.push_str("command: [");
            yaml.push_str(c);
            yaml.push_str("]\n");
        }
        yaml.push_str("steps:\n  s1:\n    prompt: hi\n");
        WorkflowConfig::from_yaml(&yaml).unwrap_or_else(|e| panic!("{e:?}"))
    }

    #[test]
    fn templates_select_command_variants_without_sdk() {
        let config = config_with(None, Some("echo"));
        assert_eq!(plan_template(&config), PLAN_PROMPT_TEMPLATE);
        assert_eq!(fix_plan_template(&config), FIX_PLAN_PROMPT_TEMPLATE);
        assert_eq!(ask_plan_template(&config), ASK_PLAN_PROMPT_TEMPLATE);
    }

    #[test]
    fn templates_select_sdk_variants_with_sdk() {
        let config = config_with(Some("seher"), None);
        assert_eq!(plan_template(&config), PLAN_PROMPT_TEMPLATE_SDK);
        assert_eq!(fix_plan_template(&config), FIX_PLAN_PROMPT_TEMPLATE_SDK);
        assert_eq!(ask_plan_template(&config), ASK_PLAN_PROMPT_TEMPLATE_SDK);
    }

    #[test]
    fn sdk_and_command_templates_differ() {
        // Guards against the two variant sets accidentally pointing at the same file.
        assert_ne!(PLAN_PROMPT_TEMPLATE, PLAN_PROMPT_TEMPLATE_SDK);
        assert_ne!(FIX_PLAN_PROMPT_TEMPLATE, FIX_PLAN_PROMPT_TEMPLATE_SDK);
        assert_ne!(ASK_PLAN_PROMPT_TEMPLATE, ASK_PLAN_PROMPT_TEMPLATE_SDK);
    }
}
