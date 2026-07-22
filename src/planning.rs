use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use console::style;

use crate::ask_handler::AskHandler;
use crate::cancellation::CancellationToken;
use crate::config::WorkflowConfig;
use crate::error::Result;
use crate::executor::{Executor, PromptRun};
use crate::step::prompt::{PromptResult, StreamCallbacks};
use crate::variable::VariableStore;

/// Environment variable key for pi's HTTP request timeout.
const PI_HTTP_REQUEST_TIMEOUT_SECS: &str = "PI_HTTP_REQUEST_TIMEOUT_SECS";

/// Default 30-minute timeout for planning requests (1800 seconds).
/// Avoids failing after pi's 60-second default HTTP request timeout when the
/// selected seher plan backend uses pi.
const DEFAULT_PI_PLANNING_REQUEST_TIMEOUT_SECS: &str = "1800";

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

/// Template variable holding the ambiguity-handling guidance for the current
/// planning turn. Resolved from [`PlanPromptCtx::interactive`] on every
/// plan-related turn so interactive and headless (CI) runs get different
/// instructions.
const PLAN_CLARIFICATION_VAR: &str = "plan.clarification";

/// Guidance for interactive runs: the user can be reached via the `ask_user`
/// tool, so ambiguous requirements should be clarified rather than guessed.
const PLAN_CLARIFICATION_INTERACTIVE: &str = "**Whenever a requirement is genuinely ambiguous \
and the answer changes the plan, call the `ask_user` tool to ask the user a focused question \
instead of guessing. Prefer asking over assuming.**";

/// Guidance for non-interactive runs (piped stdin, CI, GitHub Actions): no
/// `ask_user` tool is registered, so the agent must never stop to wait for
/// clarification — it decides on stated assumptions and still submits a plan.
const PLAN_CLARIFICATION_NONINTERACTIVE: &str = "**This run is non-interactive: the `ask_user` \
tool is not available and no one can answer questions. Never defer work or end your turn to \
wait for clarification — when a requirement is ambiguous, choose the most reasonable \
interpretation, state the assumption explicitly in the plan, and continue.**";

/// Ambiguity-handling guidance for the current run's interactivity.
#[must_use]
fn clarification_guidance(interactive: bool) -> &'static str {
    if interactive {
        PLAN_CLARIFICATION_INTERACTIVE
    } else {
        PLAN_CLARIFICATION_NONINTERACTIVE
    }
}

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
    vars.set_named_value(PLAN_LANGUAGE_VAR, config.effective_plan_language());
    vars
}

/// Whether SDK-mode planning should drive the plan through the interactive
/// custom tools (`submit_plan` / `update_plan` / `ask_user`).
///
/// True only when an SDK backend is selected (`sdk: seher` or `sdk: pi`) *and*
/// `interactive_planning` is left enabled. When false the planning turns fall
/// back to the file-writing (`command`-style) templates and register no custom
/// tools. Under `sdk: seher` this keeps tool-incapable providers (e.g.
/// `sdk: claude-terminal`, `sdk: claude-headless`) eligible; `sdk: pi` always
/// supports custom tools, so this mainly matters there for the file-based
/// planning behavior itself, not provider eligibility.
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

/// Resolve environment variables for plan-related prompts.
///
/// 1. Resolves workflow top-level `env:` with template semantics (same as normal steps).
/// 2. Adds `PI_HTTP_REQUEST_TIMEOUT_SECS=1800` only when:
///    - The resolved workflow env does NOT already contain `PI_HTTP_REQUEST_TIMEOUT_SECS`
///    - The ambient process environment does NOT define `PI_HTTP_REQUEST_TIMEOUT_SECS`
///
/// This preserves the override precedence:
/// - Explicit workflow `env:` wins (inserted first)
/// - Ambient `PI_HTTP_REQUEST_TIMEOUT_SECS` wins (cruise doesn't override)
/// - The 1800-second default is used only when neither source specifies a value
fn resolve_planning_env(
    config: &WorkflowConfig,
    vars: &VariableStore,
) -> Result<HashMap<String, String>> {
    // Resolve workflow top-level env with template semantics
    let mut env = crate::engine::resolve_env(&config.env, &HashMap::new(), vars)?;

    // Insert default only when no workflow or ambient override exists
    if !env.contains_key(PI_HTTP_REQUEST_TIMEOUT_SECS)
        && std::env::var_os(PI_HTTP_REQUEST_TIMEOUT_SECS).is_none()
    {
        env.insert(
            PI_HTTP_REQUEST_TIMEOUT_SECS.to_string(),
            DEFAULT_PI_PLANNING_REQUEST_TIMEOUT_SECS.to_string(),
        );
    }

    Ok(env)
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
/// Returns an error if variable resolution fails, the backend fails, a rate
/// limit is hit and retries are exhausted, or — when the plan tools were
/// registered (`sdk:` planning with `register_plan_tools`) — the agent ended
/// its turn without a successful `submit_plan` / `update_plan` call (the
/// captured output is not a plan and is refused as a fallback).
pub async fn run_plan_prompt_template(
    ctx: &PlanPromptCtx<'_>,
    vars: &mut VariableStore,
    template: &str,
    label: &str,
    stream_callbacks: Option<&StreamCallbacks<'_>>,
    resume: &mut Option<String>,
    register_plan_tools: bool,
) -> Result<PromptResult> {
    // The ambiguity guidance depends on whether a user can answer `ask_user`
    // this run; refresh it every turn so plan/fix templates never instruct the
    // agent to call a tool that is not registered.
    vars.set_named_value(
        PLAN_CLARIFICATION_VAR,
        clarification_guidance(ctx.interactive).to_string(),
    );
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
    let (tools, plan_persisted) = if plan_tools_enabled && register_plan_tools {
        let set = crate::sdk_tools::planning_tools(
            ctx.plan_path.to_path_buf(),
            Arc::clone(&ctx.ask),
            ctx.interactive,
        );
        (set.tools, Some(set.plan_persisted))
    } else {
        (Vec::new(), None)
    };

    let env = resolve_planning_env(ctx.config, vars)?;
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
    // Lazily read the backend transcript only for the error path — a
    // successful turn must not pay for loading the whole JSONL.
    ensure_plan_persisted(plan_persisted.as_deref(), || {
        outcome
            .session_id
            .as_deref()
            .and_then(|session_id| read_sdk_transcript(ctx.working_dir, session_id))
    })?;
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

/// Guard against a tool-based planning turn that ended without the agent
/// persisting the plan.
///
/// When the plan tools were registered (`submit_plan` / `update_plan`), the
/// agent must persist the plan through them before its turn ends. If neither
/// tool completed, the agent's captured output is usually not a plan at all —
/// clarifying questions it could never get answered, or a "handoff" note — and
/// adopting it as `plan.md` posts that non-plan to the user. Fail the turn
/// instead. `transcript` is invoked only on this failure path and, when the
/// backend transcript records a terminal error, it is appended for diagnosis.
///
/// Turns without plan tools (command backend, `interactive_planning: false`,
/// or the read-only Ask flow) pass `None` and are never guarded: the
/// captured-output fallback remains valid there.
fn ensure_plan_persisted(
    plan_persisted: Option<&AtomicBool>,
    transcript: impl FnOnce() -> Option<String>,
) -> Result<()> {
    let Some(flag) = plan_persisted else {
        return Ok(());
    };
    if flag.load(Ordering::SeqCst) {
        return Ok(());
    }
    let mut msg = "planning agent ended its turn without persisting the plan: tool-based \
        planning requires a successful `submit_plan` (or `update_plan`) call before the turn \
        ends, but neither tool completed. Refusing to adopt the agent's final message as the \
        plan. This typically means the model stopped to wait for clarification it could not \
        receive, or it does not support the planning tools."
        .to_string();
    if let Some(jsonl) = transcript()
        && let Some(backend_error) = extract_terminal_error_from_transcript(&jsonl)
    {
        use std::fmt::Write as _;
        let _ = write!(msg, " Backend transcript error: {backend_error}");
    }
    Err(crate::error::CruiseError::Other(msg))
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

/// Extract the last terminal error message from a JSONL transcript.
///
/// Scans line-by-line for JSON objects where `.message.stopReason == "error"`
/// and `.message.errorMessage` is non-empty. Returns the last such message
/// found, or `None` if no terminal error exists.
///
/// This is a pure function (no I/O) to facilitate unit testing.
#[must_use]
pub fn extract_terminal_error_from_transcript(jsonl: &str) -> Option<String> {
    let mut last_error = None;
    for line in jsonl.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Defensive parsing: skip malformed lines without failing
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        // Look for message.stopReason == "error" and message.errorMessage present
        let Some(message) = value.get("message") else {
            continue;
        };
        let Some(stop_reason) = message.get("stopReason").and_then(|v| v.as_str()) else {
            continue;
        };
        if stop_reason != "error" {
            continue;
        }
        let Some(error_message) = message.get("errorMessage").and_then(|v| v.as_str()) else {
            continue;
        };
        if !error_message.is_empty() {
            last_error = Some(error_message.to_string());
        }
    }
    last_error
}

/// Resolve plan content with backend transcript error fallback.
///
/// First attempts the standard `metadata::resolve_plan_content` fallback chain
/// (plan file → stdout → stderr). If all sources are empty and a transcript is
/// provided, checks for a terminal error in the backend transcript and returns
/// that as a descriptive error instead of the generic "no output" message.
///
/// # Errors
///
/// Returns an error if no source produced content. When a transcript with a
/// terminal error is available, the error message includes the backend's error
/// (e.g., `context_length_exceeded`).
pub fn resolve_generated_plan_content(
    plan_path: &Path,
    stdout: &str,
    stderr: &str,
    transcript: Option<&str>,
) -> Result<String> {
    match crate::metadata::resolve_plan_content(plan_path, stdout, stderr) {
        Ok(content) => Ok(content),
        Err(original_err) => {
            // Original error means plan/stdout/stderr were all empty.
            // Try to extract a more useful error from the transcript.
            if let Some(jsonl) = transcript
                && let Some(backend_error) = extract_terminal_error_from_transcript(jsonl)
            {
                return Err(crate::error::CruiseError::Other(format!(
                    "planning backend failed after producing no plan output: {backend_error}"
                )));
            }
            Err(original_err)
        }
    }
}

/// Maximum bytes read from an SDK transcript for error diagnosis.
///
/// The transcript is only scanned for the *last* terminal error, so reading
/// the tail is sufficient and bounds memory/parse cost for very long agent
/// sessions.
const MAX_TRANSCRIPT_DIAGNOSTIC_BYTES: u64 = 4 * 1024 * 1024;

/// Try to read an SDK transcript file for the given session ID.
///
/// Reads at most the last [`MAX_TRANSCRIPT_DIAGNOSTIC_BYTES`] of the file;
/// the diagnostics consumer only needs the most recent records. Returns
/// `None` if the transcript cannot be found or read (non-fatal).
#[must_use]
pub fn read_sdk_transcript(working_dir: Option<&Path>, session_id: &str) -> Option<String> {
    use std::io::{Read as _, Seek as _, SeekFrom};

    let transcript_path = seher::sdk::pi_session_path(working_dir, session_id);
    let mut file = std::fs::File::open(&transcript_path).ok()?;
    let len = file.metadata().ok()?.len();
    let capped = len > MAX_TRANSCRIPT_DIAGNOSTIC_BYTES;
    if capped {
        file.seek(SeekFrom::End(
            -MAX_TRANSCRIPT_DIAGNOSTIC_BYTES.cast_signed(),
        ))
        .ok()?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if capped {
        // The seek can land mid-line, so drop the first partial record to
        // keep JSONL lines whole for the terminal-error scan.
        let start = text.find('\n').map_or(0, |i| i + 1);
        text.drain(..start);
    }
    Some(text)
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
    fn templates_select_sdk_variants_with_pi() {
        // sdk_plan_tools_enabled / template selection are driven off
        // `config.sdk.is_some()`, not the SDK kind, so `sdk: pi` picks the same
        // tool-based templates as `sdk: seher`.
        let config = config_with(Some("pi"), None);
        assert!(sdk_plan_tools_enabled(&config));
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

    // -- extract_terminal_error_from_transcript ---------------------------------

    #[test]
    fn extract_terminal_error_returns_error_from_valid_jsonl() {
        let jsonl = r#"{"message":{"stopReason":"ok","content":"hello"}}
{"message":{"stopReason":"error","errorMessage":"context_length_exceeded: token limit 100000 exceeded"}}"#;
        let result = extract_terminal_error_from_transcript(jsonl);
        assert_eq!(
            result,
            Some("context_length_exceeded: token limit 100000 exceeded".to_string())
        );
    }

    #[test]
    fn extract_terminal_error_returns_last_error_when_multiple_exist() {
        let jsonl = r#"{"message":{"stopReason":"error","errorMessage":"first error"}}
{"message":{"stopReason":"ok","content":"some output"}}
{"message":{"stopReason":"error","errorMessage":"final context_length_exceeded error"}}"#;
        let result = extract_terminal_error_from_transcript(jsonl);
        assert_eq!(
            result,
            Some("final context_length_exceeded error".to_string())
        );
    }

    #[test]
    fn extract_terminal_error_returns_none_for_empty_input() {
        assert_eq!(extract_terminal_error_from_transcript(""), None);
    }

    #[test]
    fn extract_terminal_error_returns_none_for_no_error_lines() {
        let jsonl = r#"{"message":{"stopReason":"ok","content":"hello"}}
{"message":{"stopReason":"ok","content":"world"}}"#;
        assert_eq!(extract_terminal_error_from_transcript(jsonl), None);
    }

    #[test]
    fn extract_terminal_error_returns_none_for_malformed_json() {
        let jsonl = r#"not valid json
{"message":{"stopReason":"error","errorMessage":"this is valid but after bad line"}}"#;
        // The valid line should still be parsed
        let result = extract_terminal_error_from_transcript(jsonl);
        assert_eq!(result, Some("this is valid but after bad line".to_string()));
    }

    #[test]
    fn extract_terminal_error_returns_none_when_error_message_missing() {
        let jsonl = r#"{"message":{"stopReason":"error"}}"#;
        assert_eq!(extract_terminal_error_from_transcript(jsonl), None);
    }

    #[test]
    fn extract_terminal_error_returns_none_when_error_message_empty() {
        let jsonl = r#"{"message":{"stopReason":"error","errorMessage":""}}"#;
        assert_eq!(extract_terminal_error_from_transcript(jsonl), None);
    }

    #[test]
    fn extract_terminal_error_returns_none_when_stop_reason_not_error() {
        let jsonl = r#"{"message":{"stopReason":"max_tokens","errorMessage":"truncated"}}"#;
        assert_eq!(extract_terminal_error_from_transcript(jsonl), None);
    }

    #[test]
    fn extract_terminal_error_ignores_non_message_lines() {
        let jsonl = r#"{"type":"start","session":"abc123"}
{"message":{"stopReason":"error","errorMessage":"API error: context_length_exceeded"}}"#;
        let result = extract_terminal_error_from_transcript(jsonl);
        assert_eq!(
            result,
            Some("API error: context_length_exceeded".to_string())
        );
    }

    // -- resolve_generated_plan_content ----------------------------------------

    #[test]
    fn resolve_generated_plan_content_returns_content_when_plan_file_exists() {
        let tmp = make_temp_dir();
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "# Existing Plan\n\nSteps here.")
            .unwrap_or_else(|e| panic!("{e:?}"));

        let result = resolve_generated_plan_content(&plan_path, "", "", None)
            .unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(result, "# Existing Plan\n\nSteps here.");
    }

    #[test]
    fn resolve_generated_plan_content_returns_stdout_when_nonempty() {
        let tmp = make_temp_dir();
        let plan_path = tmp.path().join("plan.md");

        let result = resolve_generated_plan_content(&plan_path, "# Plan from stdout", "", None)
            .unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(result, "# Plan from stdout");
    }

    #[test]
    fn resolve_generated_plan_content_falls_back_to_transcript_error_when_all_empty() {
        let tmp = make_temp_dir();
        let plan_path = tmp.path().join("plan.md");
        let transcript = r#"{"message":{"stopReason":"error","errorMessage":"API error: context_length_exceeded: token limit 200000 exceeded"}}"#;

        let result = resolve_generated_plan_content(&plan_path, "", "", Some(transcript));

        assert!(result.is_err(), "expected Err, got: {result:?}");
        let Err(err) = result else {
            panic!("expected Err, got: {result:?}")
        };
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("context_length_exceeded"),
            "error should mention context_length_exceeded: {err_msg}"
        );
        assert!(
            err_msg.contains("planning backend failed"),
            "error should identify the source: {err_msg}"
        );
    }

    #[test]
    fn resolve_generated_plan_content_preserves_original_error_when_no_transcript() {
        let tmp = make_temp_dir();
        let plan_path = tmp.path().join("plan.md");

        let result = resolve_generated_plan_content(&plan_path, "", "", None);

        assert!(result.is_err(), "expected Err, got: {result:?}");
        let Err(err) = result else {
            panic!("expected Err, got: {result:?}")
        };
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("plan generation produced no output"),
            "should keep original error when no transcript: {err_msg}"
        );
    }

    #[test]
    fn resolve_generated_plan_content_preserves_original_error_when_transcript_has_no_error() {
        let tmp = make_temp_dir();
        let plan_path = tmp.path().join("plan.md");
        let transcript = r#"{"message":{"stopReason":"ok","content":"some output"}}"#;

        let result = resolve_generated_plan_content(&plan_path, "", "", Some(transcript));

        assert!(result.is_err(), "expected Err, got: {result:?}");
        let Err(err) = result else {
            panic!("expected Err, got: {result:?}")
        };
        let err_msg = err.to_string();
        assert!(
            err_msg.contains("plan generation produced no output"),
            "should keep original error when transcript has no error: {err_msg}"
        );
    }

    #[test]
    fn resolve_generated_plan_content_ignores_transcript_when_content_available() {
        let tmp = make_temp_dir();
        let plan_path = tmp.path().join("plan.md");
        let transcript =
            r#"{"message":{"stopReason":"error","errorMessage":"context_length_exceeded"}}"#;

        let result =
            resolve_generated_plan_content(&plan_path, "# Plan from stdout", "", Some(transcript))
                .unwrap_or_else(|e| panic!("{e:?}"));

        // Transcript error should be ignored since we got valid content
        assert_eq!(result, "# Plan from stdout");
    }

    // -- clarification guidance ------------------------------------------------

    #[test]
    fn clarification_guidance_interactive_prefers_ask_user() {
        let guidance = clarification_guidance(true);
        assert!(guidance.contains("ask_user"), "got: {guidance}");
        assert!(guidance.contains("Prefer asking"), "got: {guidance}");
    }

    #[test]
    fn clarification_guidance_noninteractive_forbids_waiting() {
        let guidance = clarification_guidance(false);
        assert!(guidance.contains("non-interactive"), "got: {guidance}");
        assert!(
            guidance.contains("not available"),
            "must state ask_user is unavailable: {guidance}"
        );
        assert!(
            !guidance.contains("Prefer asking"),
            "must not instruct asking when no one can answer: {guidance}"
        );
    }

    #[test]
    fn sdk_templates_resolve_clarification_var_in_both_modes() {
        let config = config_with(Some("seher"), None);
        for interactive in [true, false] {
            let mut vars = setup_plan_vars("task".to_string(), PathBuf::from("plan.md"), &config);
            // The fix template also references {prev.input} (the change request).
            vars.set_prev_input(Some("change request".to_string()));
            vars.set_named_value(
                PLAN_CLARIFICATION_VAR,
                clarification_guidance(interactive).to_string(),
            );
            for template in [PLAN_PROMPT_TEMPLATE_SDK, FIX_PLAN_PROMPT_TEMPLATE_SDK] {
                let resolved = vars
                    .resolve(template)
                    .unwrap_or_else(|e| panic!("template must resolve: {e:?}"));
                assert!(
                    !resolved.contains("{plan.clarification}"),
                    "placeholder must be substituted"
                );
                assert!(
                    resolved.contains(clarification_guidance(interactive)),
                    "resolved template must carry the {interactive}-mode guidance"
                );
            }
        }
    }

    /// Given: a command backend and a template referencing `{plan.clarification}`
    /// When: `run_plan_prompt_template` runs in either interactivity mode
    /// Then: the variable is set before resolution, so the resolved prompt
    ///       (echoed back by `cat`) equals the mode's guidance
    #[cfg(unix)]
    #[tokio::test]
    async fn run_plan_prompt_template_sets_clarification_var_before_resolve() {
        let _guard = crate::test_support::lock_process();
        for interactive in [true, false] {
            let tmp = make_temp_dir();
            let plan_path = tmp.path().join("plan.md");
            let config = config_with(None, Some("\"cat\""));
            let ctx = PlanPromptCtx {
                interactive,
                ..make_ctx_no_token(&config, &plan_path)
            };
            let mut vars = VariableStore::new("test input".to_string());
            let mut resume = None;

            let result = run_plan_prompt_template(
                &ctx,
                &mut vars,
                "{plan.clarification}",
                "test",
                None,
                &mut resume,
                false,
            )
            .await
            .unwrap_or_else(|e| panic!("expected Ok: {e:?}"));

            assert_eq!(
                result.output.trim(),
                clarification_guidance(interactive),
                "wrong guidance for interactive={interactive}"
            );
        }
    }

    // -- ensure_plan_persisted --------------------------------------------------

    #[test]
    fn ensure_plan_persisted_passes_without_plan_tools() {
        // Turns without registered plan tools (command backend, tool-less
        // planning, read-only Ask flow) are never guarded.
        assert!(ensure_plan_persisted(None, || panic!("transcript must not be read")).is_ok());
    }

    #[test]
    fn ensure_plan_persisted_passes_when_plan_was_persisted() {
        let flag = AtomicBool::new(true);
        assert!(
            ensure_plan_persisted(Some(&flag), || panic!("transcript must not be read")).is_ok()
        );
    }

    #[test]
    fn ensure_plan_persisted_errors_when_agent_never_persisted() {
        let flag = AtomicBool::new(false);
        let result = ensure_plan_persisted(Some(&flag), || None);
        let Err(err) = result else {
            panic!("expected Err, got: {result:?}")
        };
        let msg = err.to_string();
        assert!(msg.contains("submit_plan"), "got: {msg}");
        assert!(msg.contains("without persisting"), "got: {msg}");
    }

    #[test]
    fn ensure_plan_persisted_appends_transcript_terminal_error() {
        let flag = AtomicBool::new(false);
        let transcript = r#"{"message":{"stopReason":"error","errorMessage":"context_length_exceeded: token limit exceeded"}}"#;
        let result = ensure_plan_persisted(Some(&flag), || Some(transcript.to_string()));
        let Err(err) = result else {
            panic!("expected Err, got: {result:?}")
        };
        let msg = err.to_string();
        assert!(
            msg.contains("context_length_exceeded"),
            "should include the backend terminal error: {msg}"
        );
    }

    #[test]
    fn ensure_plan_persisted_omits_backend_suffix_when_transcript_has_no_error() {
        // A transcript present but without a terminal error must still yield
        // the plain missing-persistence error, with no backend suffix.
        let flag = AtomicBool::new(false);
        let transcript = r#"{"message":{"stopReason":"ok","content":"handoff note"}}"#;
        let result = ensure_plan_persisted(Some(&flag), || Some(transcript.to_string()));
        let Err(err) = result else {
            panic!("expected Err, got: {result:?}")
        };
        let msg = err.to_string();
        assert!(msg.contains("without persisting"), "got: {msg}");
        assert!(
            !msg.contains("Backend transcript error"),
            "must not append a backend suffix: {msg}"
        );
    }

    // -- resolve_planning_env --------------------------------------------------

    use crate::test_support::{EnvGuard, lock_process};
    use std::fmt::Write as _;

    /// Helper: build a `WorkflowConfig` with top-level `env` entries.
    fn config_with_env(env_entries: &[(&str, &str)]) -> WorkflowConfig {
        let mut yaml = String::from("command: [\"echo\"]\n");
        if !env_entries.is_empty() {
            yaml.push_str("env:\n");
            for (k, v) in env_entries {
                let _ = writeln!(yaml, "  {k}: \"{v}\"");
            }
        }
        yaml.push_str("steps:\n  s1:\n    prompt: hi\n");
        WorkflowConfig::from_yaml(&yaml).unwrap_or_else(|e| panic!("{e:?}"))
    }

    /// Helper: build a `WorkflowConfig` with a template variable in `env`.
    fn config_with_env_template(key: &str, template: &str) -> WorkflowConfig {
        let yaml = format!(
            "command: [\"echo\"]\nenv:\n  {key}: \"{template}\"\nsteps:\n  s1:\n    prompt: hi\n"
        );
        WorkflowConfig::from_yaml(&yaml).unwrap_or_else(|e| panic!("{e:?}"))
    }

    /// Given: empty workflow env and no ambient `PI_HTTP_REQUEST_TIMEOUT_SECS`
    /// When: `resolve_planning_env` is called
    /// Then: returns map containing `PI_HTTP_REQUEST_TIMEOUT_SECS=1800`
    #[test]
    fn resolve_planning_env_inserts_default_when_no_env() {
        let _guard = lock_process();
        let _env_guard = EnvGuard::remove(PI_HTTP_REQUEST_TIMEOUT_SECS);
        let config = config_with_env(&[]);
        let vars = VariableStore::new("test".to_string());

        let env = resolve_planning_env(&config, &vars).unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(
            env.get(PI_HTTP_REQUEST_TIMEOUT_SECS)
                .map(std::string::String::as_str),
            Some(DEFAULT_PI_PLANNING_REQUEST_TIMEOUT_SECS),
            "should insert default timeout when no env is set"
        );
    }

    /// Given: workflow env sets `PI_HTTP_REQUEST_TIMEOUT_SECS=900`
    /// When: `resolve_planning_env` is called
    /// Then: returns `900`, not the default `1800`
    #[test]
    fn resolve_planning_env_preserves_workflow_env_override() {
        let _guard = lock_process();
        let _env_guard = EnvGuard::remove(PI_HTTP_REQUEST_TIMEOUT_SECS);
        let config = config_with_env(&[(PI_HTTP_REQUEST_TIMEOUT_SECS, "900")]);
        let vars = VariableStore::new("test".to_string());

        let env = resolve_planning_env(&config, &vars).unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(
            env.get(PI_HTTP_REQUEST_TIMEOUT_SECS)
                .map(std::string::String::as_str),
            Some("900"),
            "workflow env should override the default"
        );
    }

    /// Given: ambient `PI_HTTP_REQUEST_TIMEOUT_SECS=600` and no workflow key
    /// When: `resolve_planning_env` is called
    /// Then: returned map does NOT contain the key (pi reads ambient value itself)
    #[test]
    fn resolve_planning_env_does_not_override_ambient_env() {
        let _guard = lock_process();
        let _env_guard = EnvGuard::set(PI_HTTP_REQUEST_TIMEOUT_SECS, "600");
        let config = config_with_env(&[]);
        let vars = VariableStore::new("test".to_string());

        let env = resolve_planning_env(&config, &vars).unwrap_or_else(|e| panic!("{e:?}"));

        assert!(
            !env.contains_key(PI_HTTP_REQUEST_TIMEOUT_SECS),
            "should NOT insert default when ambient env is set; pi reads it directly"
        );
    }

    /// Given: workflow env has an unrelated key with a template variable
    /// When: `resolve_planning_env` is called
    /// Then: unrelated key is preserved and template is resolved
    #[test]
    fn resolve_planning_env_preserves_unrelated_workflow_env() {
        let _guard = lock_process();
        let _env_guard = EnvGuard::remove(PI_HTTP_REQUEST_TIMEOUT_SECS);
        let config = config_with_env_template("MY_VAR", "{input}");
        let vars = VariableStore::new("hello world".to_string());

        let env = resolve_planning_env(&config, &vars).unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(
            env.get("MY_VAR").map(std::string::String::as_str),
            Some("hello world"),
            "unrelated workflow env should be preserved and template resolved"
        );
        // Default should also be inserted since no PI_HTTP_REQUEST_TIMEOUT_SECS was set
        assert_eq!(
            env.get(PI_HTTP_REQUEST_TIMEOUT_SECS)
                .map(std::string::String::as_str),
            Some(DEFAULT_PI_PLANNING_REQUEST_TIMEOUT_SECS),
            "default timeout should be inserted alongside unrelated env"
        );
    }

    /// Given: workflow env sets both `PI_HTTP_REQUEST_TIMEOUT_SECS` and an unrelated key
    /// When: `resolve_planning_env` is called
    /// Then: both are preserved; the timeout override wins
    #[test]
    fn resolve_planning_env_preserves_override_with_other_env() {
        let _guard = lock_process();
        let _env_guard = EnvGuard::remove(PI_HTTP_REQUEST_TIMEOUT_SECS);
        let config = config_with_env(&[
            (PI_HTTP_REQUEST_TIMEOUT_SECS, "1200"),
            ("OTHER_KEY", "value"),
        ]);
        let vars = VariableStore::new("test".to_string());

        let env = resolve_planning_env(&config, &vars).unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(
            env.get(PI_HTTP_REQUEST_TIMEOUT_SECS)
                .map(std::string::String::as_str),
            Some("1200"),
            "workflow timeout override should be preserved"
        );
        assert_eq!(
            env.get("OTHER_KEY").map(std::string::String::as_str),
            Some("value"),
            "other workflow env should be preserved"
        );
    }

    /// Given: command backend and no env overrides
    /// When: `run_plan_prompt_template` is called
    /// Then: the default `PI_HTTP_REQUEST_TIMEOUT_SECS` is visible in the child process
    #[cfg(unix)]
    #[tokio::test]
    async fn run_plan_prompt_template_forwards_default_env_to_command_backend() {
        let _guard = lock_process();
        let _env_guard = EnvGuard::remove(PI_HTTP_REQUEST_TIMEOUT_SECS);
        let tmp = make_temp_dir();
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "").unwrap_or_else(|e| panic!("{e:?}"));

        // Use `sh -c 'printf %s "$PI_HTTP_REQUEST_TIMEOUT_SECS"'` as the command
        // to observe the env value in the child process.
        let config = config_with(
            None,
            Some("\"sh\", \"-c\", \"printf %s $PI_HTTP_REQUEST_TIMEOUT_SECS\""),
        );
        let ctx = make_ctx_no_token(&config, &plan_path);
        let mut vars = VariableStore::new("test input".to_string());
        let mut resume = None;

        let result =
            run_plan_prompt_template(&ctx, &mut vars, "hello", "test", None, &mut resume, false)
                .await;

        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        let prompt_result = result.unwrap_or_else(|e| panic!("{e:?}"));
        // The command should have printed "1800" to stdout, which becomes the output
        assert!(
            prompt_result.output.contains("1800"),
            "expected output to contain '1800', got: {:?}",
            prompt_result.output
        );
    }
}
