use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use console::style;

use crate::ask_handler::AskHandler;
use crate::cancellation::CancellationToken;
use crate::config::{DEFAULT_PLAN_LANGUAGE, WorkflowConfig};
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
/// "Grill me" planning variant: the agent interviews the user one question at a
/// time (via `ask_user`) until every design branch is resolved, then submits the
/// plan. SDK-only — it relies on the interactive `ask_user` tool.
pub const PLAN_GRILL_PROMPT_TEMPLATE_SDK: &str = include_str!("../prompts/plan-grill-sdk.md");

const PLAN_LANGUAGE_VAR: &str = "plan.language";

/// Build the variable store used by all plan-related flows.
///
/// Registers both `{plan}` (the session plan file path) and `{plan.language}`
/// (normalized from `plan_language`, defaulting to English if blank) so CLI and
/// GUI planning prompts resolve the same variables.
#[must_use]
pub fn setup_plan_vars(
    session_input: String,
    plan_path: PathBuf,
    config: &WorkflowConfig,
) -> VariableStore {
    let mut vars = VariableStore::new(session_input);
    vars.set_named_file(crate::session::PLAN_VAR, plan_path);
    let lang = config.plan_language.trim();
    let lang = if lang.is_empty() {
        DEFAULT_PLAN_LANGUAGE
    } else {
        lang
    };
    vars.set_named_value(PLAN_LANGUAGE_VAR, lang.to_string());
    vars
}

/// Whether SDK-mode planning should drive the plan through the interactive
/// custom tools (`submit_plan` / `update_plan` / `ask_user`).
///
/// True only when the SDK backend is selected *and* `interactive_planning` is
/// left enabled. When false the planning turns fall back to the file-writing
/// (`command`-style) templates and register no custom tools, so tool-incapable
/// providers (e.g. `sdk: claude-terminal`, `sdk: claude-headless`) stay
/// eligible.
#[must_use]
pub fn sdk_plan_tools_enabled(config: &WorkflowConfig) -> bool {
    config.sdk.is_some() && config.interactive_planning
}

/// Select the plan-generation template for the configured backend.
#[must_use]
pub fn plan_template(config: &WorkflowConfig) -> &'static str {
    if sdk_plan_tools_enabled(config) {
        PLAN_PROMPT_TEMPLATE_SDK
    } else {
        PLAN_PROMPT_TEMPLATE
    }
}

/// Select the initial plan template, accounting for the "grill me" mode.
///
/// Grill mode is SDK-only and interactive-only (enforced by the caller before
/// reaching here); when `grill` is set it returns the grill template. Otherwise
/// it falls back to [`plan_template`].
#[must_use]
pub fn initial_plan_template(config: &WorkflowConfig, grill: bool) -> &'static str {
    if grill && sdk_plan_tools_enabled(config) {
        PLAN_GRILL_PROMPT_TEMPLATE_SDK
    } else {
        plan_template(config)
    }
}

/// Select the fix-plan template for the configured backend.
#[must_use]
pub fn fix_plan_template(config: &WorkflowConfig) -> &'static str {
    if sdk_plan_tools_enabled(config) {
        FIX_PLAN_PROMPT_TEMPLATE_SDK
    } else {
        FIX_PLAN_PROMPT_TEMPLATE
    }
}

/// Select the ask-plan template for the configured backend.
#[must_use]
pub fn ask_plan_template(config: &WorkflowConfig) -> &'static str {
    if sdk_plan_tools_enabled(config) {
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
    /// "Grill me" mode: drive initial planning with the interview-style template
    /// ([`PLAN_GRILL_PROMPT_TEMPLATE_SDK`]). SDK + interactive only; validated by
    /// the caller. Affects only the initial plan turn (not fix/ask).
    pub grill: bool,
    /// Cooperative cancellation token forwarded to the executor.
    ///
    /// CLI plan flows set this and race the LLM call against Ctrl+C. The GUI
    /// passes `None` to avoid terminal-signal cancellation in a non-TTY context.
    pub cancel_token: Option<&'a CancellationToken>,
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
    // Tool-based planning is gated on `interactive_planning`; when it is off the
    // plan is written to `{plan}` directly via the file-writing templates and no
    // custom tools are registered, keeping tool-incapable providers eligible.
    let plan_tools_enabled = sdk_plan_tools_enabled(ctx.config);
    let tools = if plan_tools_enabled && register_plan_tools {
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
            cancel_token: ctx.cancel_token,
            working_dir: ctx.working_dir,
            stream: stream_callbacks,
            tools,
            resume: resume.clone(),
        })
        .await;
    drop(spinner);

    let outcome = outcome?;
    // Carry the seher session id forward only in the tool-based interactive flow,
    // where plan/fix/ask turns share one tool-capable conversation (pi or claude).
    // The tool-less flow is stateless (templates read `{plan}` from disk each
    // turn), so leaving `resume` empty keeps `require_tools` false and
    // tool-incapable providers (e.g. claude-terminal, claude-headless) eligible
    // for the fix/ask turns too.
    if plan_tools_enabled && outcome.session_id.is_some() {
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

    /// Build an SDK config with `interactive_planning: false`.
    fn sdk_config_no_interactive() -> WorkflowConfig {
        WorkflowConfig::from_yaml(
            "sdk: seher\ninteractive_planning: false\nsteps:\n  s1:\n    prompt: hi\n",
        )
        .unwrap_or_else(|e| panic!("{e:?}"))
    }

    #[test]
    fn interactive_planning_defaults_to_true_for_sdk() {
        // Omitting the field keeps the tool-based interactive flow (SDK templates).
        let config = config_with(Some("seher"), None);
        assert!(config.interactive_planning);
        assert!(sdk_plan_tools_enabled(&config));
    }

    #[test]
    fn sdk_plan_tools_disabled_when_interactive_planning_off() {
        let config = sdk_config_no_interactive();
        assert!(!sdk_plan_tools_enabled(&config));
    }

    #[test]
    fn templates_fall_back_to_command_variants_when_interactive_planning_off() {
        // Tool-less SDK planning reuses the file-writing (command-style) templates
        // so the agent writes `{plan}` directly with no custom tools.
        let config = sdk_config_no_interactive();
        assert_eq!(plan_template(&config), PLAN_PROMPT_TEMPLATE);
        assert_eq!(fix_plan_template(&config), FIX_PLAN_PROMPT_TEMPLATE);
        assert_eq!(ask_plan_template(&config), ASK_PLAN_PROMPT_TEMPLATE);
    }

    #[test]
    fn grill_ignored_when_interactive_planning_off() {
        // Grill needs the `ask_user` tool, which is not registered in the
        // tool-less flow; the initial template falls back to the file-writing one.
        let config = sdk_config_no_interactive();
        assert_eq!(initial_plan_template(&config, true), PLAN_PROMPT_TEMPLATE);
    }

    #[test]
    fn sdk_and_command_templates_differ() {
        // Guards against the two variant sets accidentally pointing at the same file.
        assert_ne!(PLAN_PROMPT_TEMPLATE, PLAN_PROMPT_TEMPLATE_SDK);
        assert_ne!(FIX_PLAN_PROMPT_TEMPLATE, FIX_PLAN_PROMPT_TEMPLATE_SDK);
        assert_ne!(ASK_PLAN_PROMPT_TEMPLATE, ASK_PLAN_PROMPT_TEMPLATE_SDK);
    }

    // -- grill template selection ---------------------------------------------

    #[test]
    fn initial_plan_template_uses_grill_variant_for_sdk_when_enabled() {
        let config = config_with(Some("seher"), None);
        assert_eq!(
            initial_plan_template(&config, true),
            PLAN_GRILL_PROMPT_TEMPLATE_SDK
        );
    }

    #[test]
    fn initial_plan_template_uses_standard_sdk_variant_when_grill_off() {
        let config = config_with(Some("seher"), None);
        assert_eq!(
            initial_plan_template(&config, false),
            PLAN_PROMPT_TEMPLATE_SDK
        );
    }

    #[test]
    fn initial_plan_template_ignores_grill_without_sdk() {
        // Grill is SDK-only; the command backend falls back to the standard plan
        // template even if the flag is set (the CLI rejects this combo earlier).
        let config = config_with(None, Some("echo"));
        assert_eq!(initial_plan_template(&config, true), PLAN_PROMPT_TEMPLATE);
    }

    #[test]
    fn grill_template_differs_from_standard_sdk_plan() {
        assert_ne!(PLAN_GRILL_PROMPT_TEMPLATE_SDK, PLAN_PROMPT_TEMPLATE_SDK);
    }

    // -- cancel_token field in PlanPromptCtx -----------------------------------

    use crate::ask_handler::NoninteractiveAskHandler;
    use crate::cancellation::CancellationToken;
    use crate::error::CruiseError;
    use crate::variable::VariableStore;
    use std::sync::Arc;

    fn make_ctx_no_token<'a>(config: &'a WorkflowConfig, plan_path: &'a Path) -> PlanPromptCtx<'a> {
        PlanPromptCtx {
            config,
            ask: Arc::new(NoninteractiveAskHandler),
            plan_path,
            interactive: false,
            rate_limit_retries: 0,
            working_dir: None,
            grill: false,
            cancel_token: None,
        }
    }

    /// Given: a `PlanPromptCtx` built without a cancel token
    /// When: the field is inspected
    /// Then: `cancel_token` is None
    #[test]
    fn plan_prompt_ctx_cancel_token_is_none_when_not_set() {
        let tmp = make_temp_dir();
        let plan_path = tmp.path().join("plan.md");
        let config = config_with(None, Some("\"echo\""));

        let ctx = make_ctx_no_token(&config, &plan_path);

        assert!(ctx.cancel_token.is_none());
    }

    /// Given: a `CancellationToken` passed into `PlanPromptCtx`
    /// When: the field is inspected
    /// Then: `cancel_token` is Some, pointing at the original token's state
    #[test]
    fn plan_prompt_ctx_cancel_token_stored_when_provided() {
        let tmp = make_temp_dir();
        let plan_path = tmp.path().join("plan.md");
        let config = config_with(None, Some("\"echo\""));
        let token = CancellationToken::new();

        let ctx = PlanPromptCtx {
            config: &config,
            ask: Arc::new(NoninteractiveAskHandler),
            plan_path: &plan_path,
            interactive: false,
            rate_limit_retries: 0,
            working_dir: None,
            grill: false,
            cancel_token: Some(&token),
        };

        assert!(ctx.cancel_token.is_some());
        // Cancelling the token is visible through the stored reference.
        token.cancel();
        assert!(
            ctx.cancel_token
                .unwrap_or_else(|| panic!("cancel_token was set above"))
                .is_cancelled()
        );
    }

    /// Given: no cancel token, command backend = cat
    /// When: `run_plan_prompt_template` is called with a simple template
    /// Then: returns Ok (baseline — no regression)
    #[cfg(unix)]
    #[tokio::test]
    async fn run_plan_prompt_template_with_no_cancel_token_completes() {
        let _guard = crate::test_support::lock_process();
        let tmp = make_temp_dir();
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "").unwrap_or_else(|e| panic!("{e:?}"));
        let config = config_with(None, Some("\"cat\""));
        let ctx = make_ctx_no_token(&config, &plan_path);
        let mut vars = VariableStore::new("test input".to_string());
        let mut resume = None;

        let result =
            run_plan_prompt_template(&ctx, &mut vars, "hello", "test", None, &mut resume, false)
                .await;

        assert!(result.is_ok(), "expected Ok, got: {result:?}");
    }

    /// Given: a pre-cancelled token and a blocking command (sleep 100)
    /// When: `run_plan_prompt_template` is called
    /// Then: returns `CruiseError::Interrupted` before the 5-second timeout
    ///
    /// If this test times out, the `cancel_token` is not forwarded to `PromptRun`.
    #[cfg(unix)]
    #[tokio::test]
    async fn run_plan_prompt_template_pre_cancelled_token_returns_interrupted() {
        let _guard = crate::test_support::lock_process();
        let tmp = make_temp_dir();
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "").unwrap_or_else(|e| panic!("{e:?}"));
        let config = config_with(None, Some("\"sleep\", \"100\""));
        let token = CancellationToken::new();
        token.cancel();

        let ctx = PlanPromptCtx {
            config: &config,
            ask: Arc::new(NoninteractiveAskHandler),
            plan_path: &plan_path,
            interactive: false,
            rate_limit_retries: 0,
            working_dir: None,
            grill: false,
            cancel_token: Some(&token),
        };
        let mut vars = VariableStore::new("test input".to_string());
        let mut resume = None;

        let timed = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run_plan_prompt_template(&ctx, &mut vars, "hello", "test", None, &mut resume, false),
        )
        .await;

        assert!(
            timed.is_ok(),
            "timed out — cancel_token is not forwarded to PromptRun"
        );
        assert!(
            matches!(
                timed.unwrap_or_else(|e| panic!("{e:?}")),
                Err(CruiseError::Interrupted)
            ),
            "expected CruiseError::Interrupted"
        );
    }
}
