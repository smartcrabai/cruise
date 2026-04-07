use std::io::{IsTerminal, Read};
use std::path::Path;
use std::process::Stdio;

use console::style;
use inquire::InquireError;

use indexmap::IndexMap;

use crate::cli::{
    DEFAULT_MAX_RETRIES, DEFAULT_RATE_LIMIT_RETRIES, PLAN_STDIN_SENTINEL, PlanArgs, PlanWorkerArgs,
};
use crate::config::{StepConfig, WorkflowConfig, validate_config};
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};
use crate::resolver::ConfigSource;
use crate::session::{PLAN_VAR, SessionManager, SessionState, get_cruise_home};
use crate::variable::VariableStore;

const PLAN_PROMPT_TEMPLATE: &str = include_str!("../prompts/plan.md");
const FIX_PLAN_PROMPT_TEMPLATE: &str = include_str!("../prompts/fix-plan.md");
const ASK_PLAN_PROMPT_TEMPLATE: &str = include_str!("../prompts/ask-plan.md");

pub async fn run(args: PlanArgs) -> Result<()> {
    // Resolve config first so the path is visible before prompting for input.
    let (yaml, source) = crate::resolver::resolve_config(args.config.as_deref())?;
    eprintln!("{}", style(source.display_string()).dim());

    // noninteractive is true whenever stdin is not a terminal (pipe, redirect,
    // or backward-compat path where cli.rs already consumed stdin and placed
    // the content in args.input).  This prevents inquire from attempting to
    // read interactive input from a non-TTY file descriptor.
    let noninteractive = !std::io::stdin().is_terminal();
    let input = read_plan_input(args.input, noninteractive)?;

    if args.dry_run {
        eprintln!(
            "{}",
            style(format!("Would plan: \"{}\"", input.trim())).dim()
        );
        return Ok(());
    }
    let config = WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
    validate_config(&config)?;

    let manager = SessionManager::new(get_cruise_home()?);
    let mut session = create_planning_session(&manager, &source, &yaml, input.trim().to_string())?;

    // Set up variables with the session plan path.
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path.clone());

    if let Err(e) = generate_plan_markdown(
        &config,
        &mut vars,
        &plan_path,
        args.rate_limit_retries,
        Some(session.base_dir.as_path()),
    )
    .await
    {
        eprintln!(
            "\n{} Plan generation failed. Session {} discarded.",
            style("✗").red().bold(),
            session.id
        );
        if let Err(del_err) = manager.delete(&session.id) {
            eprintln!("warning: failed to clean up session: {del_err}");
        }
        return Err(e);
    }

    // Approve-plan loop.
    run_approve_loop(
        &config,
        &manager,
        &mut session,
        &plan_path,
        &mut vars,
        args.rate_limit_retries,
        noninteractive,
    )
    .await
}

pub async fn launch_background_plan(plan_input: String) -> Result<()> {
    let (yaml, source) = crate::resolver::resolve_config(None)?;
    eprintln!("{}", style(source.display_string()).dim());

    let config = WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
    validate_config(&config)?;

    let input = read_background_plan_input(plan_input)?;
    let manager = SessionManager::new(get_cruise_home()?);
    let session = create_planning_session(&manager, &source, &yaml, input)?;

    spawn_plan_worker(&session.id, DEFAULT_RATE_LIMIT_RETRIES)?;

    eprintln!(
        "\n{} Session {} created. Planning in background.",
        style("✓").green().bold(),
        session.id
    );
    eprintln!("  Check status with: {}", style("cruise list").cyan());
    eprintln!(
        "  Run once ready: {}",
        style(format!("cruise run {}", session.id)).cyan()
    );
    Ok(())
}

pub async fn run_plan_worker(args: PlanWorkerArgs) -> Result<()> {
    let manager = SessionManager::new(get_cruise_home()?);
    let mut session = match manager.load(&args.session) {
        Ok(session) => session,
        Err(CruiseError::SessionError(_)) => return Ok(()),
        Err(err) => return Err(err),
    };
    session.plan_error = None;
    manager.save(&session)?;

    let result = generate_plan_for_session(&manager, &session, args.rate_limit_retries).await;
    match result {
        Ok(plan_markdown) => {
            crate::metadata::refresh_session_title_from_plan(&mut session, &plan_markdown);
            session.plan_error = None;
            manager.save(&session)?;
            Ok(())
        }
        Err(err) => {
            let plan_error = err.to_string();
            session.plan_error = Some(plan_error.clone());
            manager.save(&session)?;
            Err(CruiseError::Other(plan_error))
        }
    }
}

/// Read task input from CLI arg, piped stdin, or interactive prompt.
fn read_plan_input(input: Option<String>, noninteractive: bool) -> Result<String> {
    let stdin_input = if input.is_none() && noninteractive {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(CruiseError::IoError)?;
        Some(s)
    } else {
        None
    };
    resolve_input(input, stdin_input, || {
        if noninteractive {
            return Err(CruiseError::Other(
                "no input provided: stdin is not a terminal and no --input flag was given"
                    .to_string(),
            ));
        }
        prompt_for_plan_input()
    })
}

fn read_background_plan_input(input: String) -> Result<String> {
    if input == PLAN_STDIN_SENTINEL {
        if std::io::stdin().is_terminal() {
            return Err(CruiseError::Other(format!(
                "--plan {PLAN_STDIN_SENTINEL} requires piped stdin"
            )));
        }
        let mut stdin_input = String::new();
        std::io::stdin()
            .read_to_string(&mut stdin_input)
            .map_err(CruiseError::IoError)?;
        let trimmed = stdin_input.trim().to_string();
        if trimmed.is_empty() {
            return Err(CruiseError::Other(
                "no input provided on stdin for background planning".to_string(),
            ));
        }
        return Ok(trimmed);
    }

    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        return Err(CruiseError::Other(
            "background planning input cannot be empty".to_string(),
        ));
    }
    Ok(trimmed)
}

async fn approve_with_title(
    session: &mut SessionState,
    manager: &SessionManager,
    plan_content: &str,
    llm_api: Option<&crate::llm_api::LlmApiConfig>,
) -> Result<()> {
    if let Some(api_config) = llm_api {
        match crate::llm_api::generate_session_title(api_config, &session.input, plan_content).await
        {
            Ok(title) => session.title = Some(title),
            Err(e) => {
                eprintln!("warning: session title generation via API failed: {e}");
                crate::metadata::refresh_session_title_from_plan(session, plan_content);
            }
        }
    } else {
        crate::metadata::refresh_session_title_from_plan(session, plan_content);
    }
    session.approve();
    manager.save(session)
}

fn create_planning_session(
    manager: &SessionManager,
    source: &ConfigSource,
    yaml: &str,
    input: String,
) -> Result<SessionState> {
    let session_id = SessionManager::new_session_id();
    let base_dir = std::env::current_dir()?;
    let mut session =
        SessionState::new(session_id.clone(), base_dir, source.display_string(), input);
    session.config_path = source.path().cloned();
    manager.create(&session)?;

    if session.config_path.is_none() {
        let session_dir = manager.sessions_dir().join(&session_id);
        std::fs::write(session_dir.join("config.yaml"), yaml)?;
    }

    Ok(session)
}

fn spawn_plan_worker(session_id: &str, rate_limit_retries: usize) -> Result<()> {
    let exe = std::env::current_exe().map_err(CruiseError::IoError)?;
    std::process::Command::new(exe)
        .arg("plan-worker")
        .arg("--session")
        .arg(session_id)
        .arg("--rate-limit-retries")
        .arg(rate_limit_retries.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| CruiseError::ProcessSpawnError(format!("failed to spawn plan worker: {e}")))?;
    Ok(())
}

async fn generate_plan_for_session(
    manager: &SessionManager,
    session: &SessionState,
    rate_limit_retries: usize,
) -> Result<String> {
    let config = manager.load_config(session)?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path.clone());
    generate_plan_markdown(
        &config,
        &mut vars,
        &plan_path,
        rate_limit_retries,
        Some(session.base_dir.as_path()),
    )
    .await
}

async fn generate_plan_markdown(
    config: &WorkflowConfig,
    vars: &mut VariableStore,
    plan_path: &Path,
    rate_limit_retries: usize,
    working_dir: Option<&Path>,
) -> Result<String> {
    let prompt_result = crate::planning::run_plan_prompt_template(
        config,
        vars,
        PLAN_PROMPT_TEMPLATE,
        "[plan] creating plan...",
        rate_limit_retries,
        working_dir,
    )
    .await?;
    crate::metadata::resolve_plan_content(plan_path, &prompt_result.output, &prompt_result.stderr)
}

/// Present a `MultiSelect` prompt so the user can choose which steps to skip.
/// Returns an empty `Vec` when there are no steps, the user cancels, or
/// an interruption is received (so the approve flow can continue unblocked).
fn select_steps_to_skip(steps: &IndexMap<String, StepConfig>) -> Result<Vec<String>> {
    let step_names: Vec<&str> = steps.keys().map(std::string::String::as_str).collect();
    if step_names.is_empty() {
        return Ok(vec![]);
    }
    match inquire::MultiSelect::new(
        "Steps to skip (Space to toggle, Enter to confirm):",
        step_names,
    )
    .with_help_message("No selection = run all steps")
    .prompt()
    {
        Ok(selected) => Ok(selected
            .into_iter()
            .map(std::string::ToString::to_string)
            .collect()),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(vec![]),
        Err(e) => Err(CruiseError::Other(format!("selection error: {e}"))),
    }
}

/// Interactive approve-plan loop: show plan, let user approve/fix/ask/execute.
/// When `noninteractive` is true (e.g. stdin was piped), auto-approves the plan
/// without prompting so that inquire never tries to read from a non-TTY stdin.
async fn run_approve_loop(
    config: &WorkflowConfig,
    manager: &SessionManager,
    session: &mut SessionState,
    plan_path: &std::path::Path,
    vars: &mut VariableStore,
    rate_limit_retries: usize,
    noninteractive: bool,
) -> Result<()> {
    let llm_api = crate::llm_api::resolve_llm_api_config(config.llm.as_ref());
    let working_dir = session.base_dir.clone();

    // Read the plan once up front; re-read only after Fix modifies it.
    let mut plan_content = match crate::metadata::read_plan_markdown(plan_path) {
        Ok(content) => content,
        Err(err) => {
            eprintln!(
                "\n{} Generated plan is missing or empty. Session {} discarded.",
                style("✗").red().bold(),
                session.id
            );
            if let Err(del_err) = manager.delete(&session.id) {
                eprintln!("warning: failed to clean up session: {del_err}");
            }
            return Err(err);
        }
    };

    loop {
        crate::display::print_bordered(&plan_content, Some("plan.md"));

        if noninteractive {
            approve_with_title(session, manager, &plan_content, llm_api.as_ref()).await?;
            eprintln!(
                "\n{} Session {} created.",
                style("✓").green().bold(),
                session.id
            );
            eprintln!(
                "  Run with: {}",
                style(format!("cruise run {}", session.id)).cyan()
            );
            return Ok(());
        }

        let options = vec!["Approve", "Fix", "Ask", "Execute now"];
        let selected = match inquire::Select::new("Action:", options).prompt() {
            Ok(s) => s,
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                eprintln!("\nCancelled. Session {} discarded.", session.id);
                manager.delete(&session.id)?;
                return Ok(());
            }
            Err(e) => return Err(CruiseError::Other(format!("selection error: {e}"))),
        };

        match selected {
            "Approve" => {
                session.skipped_steps = select_steps_to_skip(&config.steps)?;
                approve_with_title(session, manager, &plan_content, llm_api.as_ref()).await?;
                eprintln!(
                    "\n{} Session {} created.",
                    style("✓").green().bold(),
                    session.id
                );
                eprintln!(
                    "  Run with: {}",
                    style(format!("cruise run {}", session.id)).cyan()
                );
                return Ok(());
            }
            "Fix" => {
                let text = match prompt_multiline("Describe the changes needed:")? {
                    InputResult::Submitted(t) => t,
                    InputResult::Cancelled => continue,
                };
                vars.set_prev_input(Some(text));
                run_fix_plan(
                    config,
                    vars,
                    rate_limit_retries,
                    Some(working_dir.as_path()),
                )
                .await?;
                plan_content = crate::metadata::read_plan_markdown(plan_path)?;
            }
            "Ask" => {
                let text = match prompt_multiline("Your question:")? {
                    InputResult::Submitted(t) => t,
                    InputResult::Cancelled => continue,
                };
                vars.set_prev_input(Some(text));
                run_ask_plan(
                    config,
                    vars,
                    rate_limit_retries,
                    Some(working_dir.as_path()),
                )
                .await?;
            }
            "Execute now" => {
                session.skipped_steps = select_steps_to_skip(&config.steps)?;
                approve_with_title(session, manager, &plan_content, llm_api.as_ref()).await?;
                eprintln!(
                    "\n{} Executing session {}...",
                    style("→").cyan(),
                    session.id
                );
                let run_args = crate::cli::RunArgs {
                    session: Some(session.id.clone()),
                    all: false,
                    max_retries: DEFAULT_MAX_RETRIES,
                    rate_limit_retries,
                    dry_run: false,
                };
                return crate::run_cmd::run(run_args).await;
            }
            _ => {}
        }
    }
}

/// Generate a plan for the given session (writes `plan.md`).
///
/// Used by the Tauri GUI backend to run the plan-generation step without
/// the interactive approve loop.  The caller is responsible for creating
/// the session and wiring up the `VariableStore` (including setting `plan`
/// to the session's `plan_path`).
#[expect(dead_code, reason = "Used by Tauri GUI backend")]
pub async fn generate_plan(
    config: &crate::config::WorkflowConfig,
    vars: &mut crate::variable::VariableStore,
    rate_limit_retries: usize,
) -> crate::error::Result<()> {
    crate::planning::run_plan_prompt_template(
        config,
        vars,
        PLAN_PROMPT_TEMPLATE,
        "[plan] creating plan...",
        rate_limit_retries,
        None,
    )
    .await?;
    Ok(())
}

/// Replan an existing session using the built-in fix-plan prompt.
pub async fn replan_session(
    manager: &SessionManager,
    session: &mut SessionState,
    feedback: String,
    rate_limit_retries: usize,
) -> Result<()> {
    let config = manager.load_config(session)?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path.clone());
    vars.set_prev_input(Some(feedback));
    let working_dir = session.base_dir.clone();
    run_fix_plan(
        &config,
        &mut vars,
        rate_limit_retries,
        Some(working_dir.as_path()),
    )
    .await?;

    let plan_markdown = crate::metadata::read_plan_markdown(&plan_path)?;
    crate::metadata::refresh_session_title_from_plan(session, &plan_markdown);
    session.plan_error = None;
    manager.save(session)?;
    Ok(())
}

/// Run the built-in fix-plan prompt.
async fn run_fix_plan(
    config: &WorkflowConfig,
    vars: &mut VariableStore,
    rate_limit_retries: usize,
    working_dir: Option<&Path>,
) -> Result<()> {
    run_plan_prompt(
        config,
        vars,
        rate_limit_retries,
        FIX_PLAN_PROMPT_TEMPLATE,
        "[fix-plan] applying fixes...",
        working_dir,
    )
    .await
}

/// Run the built-in ask-plan prompt.
async fn run_ask_plan(
    config: &WorkflowConfig,
    vars: &mut VariableStore,
    rate_limit_retries: usize,
    working_dir: Option<&Path>,
) -> Result<()> {
    run_plan_prompt(
        config,
        vars,
        rate_limit_retries,
        ASK_PLAN_PROMPT_TEMPLATE,
        "[ask-plan] answering question...",
        working_dir,
    )
    .await
}

/// Shared implementation for fix-plan and ask-plan: resolve the given
/// `template`, display `label`, and run it as a prompt step.
async fn run_plan_prompt(
    config: &WorkflowConfig,
    vars: &mut VariableStore,
    rate_limit_retries: usize,
    template: &str,
    label: &str,
    working_dir: Option<&Path>,
) -> Result<()> {
    let result = crate::planning::run_plan_prompt_template(
        config,
        vars,
        template,
        label,
        rate_limit_retries,
        working_dir,
    )
    .await?;
    vars.set_prev_output(Some(result.output));
    vars.set_prev_stderr(Some(result.stderr));
    vars.set_prev_input(None);
    Ok(())
}

fn resolve_input<F>(
    arg: Option<String>,
    stdin_input: Option<String>,
    interactive: F,
) -> Result<String>
where
    F: FnOnce() -> Result<String>,
{
    if let Some(input) = arg {
        return Ok(input);
    }

    if let Some(input) = stdin_input {
        let trimmed = input.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    interactive()
}

/// Prompt interactively for the initial plan input.
fn prompt_for_plan_input() -> Result<String> {
    prompt_multiline("What would you like to implement?")?.into_result()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_input_from_arg() {
        // Given: a CLI arg is provided
        let result = resolve_input(Some("add feature X".to_string()), None, || {
            panic!("interactive prompt should not run")
        });
        assert_eq!(result.unwrap_or_else(|e| panic!("{e:?}")), "add feature X");
    }

    #[test]
    fn test_resolve_input_from_stdin() {
        // Given: stdin input is present and no CLI arg is provided
        let result = resolve_input(None, Some("  add feature from pipe\n".to_string()), || {
            panic!("interactive prompt should not run")
        });
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")),
            "add feature from pipe"
        );
    }

    #[test]
    fn test_resolve_input_without_arg_or_stdin_uses_interactive_result() {
        // Given: no CLI arg or stdin input is available
        let result = resolve_input(None, None, || Ok("resume in place".to_string()));
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")),
            "resume in place"
        );
    }

    // ── resolve_input with multiline stdin ───────────────────────────────────

    #[test]
    fn test_resolve_input_multiline_from_stdin_preserves_internal_newlines() {
        // Given: multi-line stdin input (piped, etc.)
        let stdin = "line1\nline2\nline3\n".to_string();
        let result = resolve_input(None, Some(stdin), || {
            panic!("interactive prompt should not run")
        });
        // Then: only leading/trailing whitespace is trimmed, internal newlines are preserved
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")),
            "line1\nline2\nline3"
        );
    }

    #[test]
    fn test_resolve_input_multiline_trims_only_leading_trailing_whitespace() {
        // Given: multi-line stdin input with extra whitespace at start and end
        let stdin = "  line1\nline2  \n".to_string();
        let result = resolve_input(None, Some(stdin), || {
            panic!("interactive prompt should not run")
        });
        // Then: only leading/trailing whitespace is removed, internal newlines are preserved
        assert_eq!(result.unwrap_or_else(|e| panic!("{e:?}")), "line1\nline2");
    }
}
