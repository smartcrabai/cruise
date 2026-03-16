use std::io::IsTerminal;

use console::style;
use inquire::InquireError;

use crate::cli::{DEFAULT_MAX_RETRIES, PlanArgs};
use crate::config::{WorkflowConfig, validate_config};
use crate::engine::{resolve_command_with_model, run_prompt_step};
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};
use crate::session::{SessionManager, SessionState, get_cruise_home};
use crate::step::PromptStep;
use crate::variable::VariableStore;

/// Name of the variable that holds the plan file path.
pub(crate) const PLAN_VAR: &str = "plan";
const PLAN_PROMPT_TEMPLATE: &str = include_str!("../prompts/plan.md");
const FIX_PLAN_PROMPT_TEMPLATE: &str = include_str!("../prompts/fix-plan.md");
const ASK_PLAN_PROMPT_TEMPLATE: &str = include_str!("../prompts/ask-plan.md");

pub async fn run(args: PlanArgs) -> Result<()> {
    // Resolve config first so the path is visible before prompting for input.
    let (yaml, source) = crate::resolver::resolve_config(args.config.as_deref())?;
    eprintln!("{}", style(source.display_string()).dim());

    // Resolve input: CLI arg, or read from stdin if piped.
    // noninteractive is true whenever stdin is not a terminal (pipe, redirect,
    // or backward-compat path where cli.rs already consumed stdin and placed
    // the content in args.input).  This prevents inquire from attempting to
    // read interactive input from a non-TTY file descriptor.
    let noninteractive = !std::io::stdin().is_terminal();
    let stdin_input = if args.input.is_none() && noninteractive {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(CruiseError::IoError)?;
        Some(s)
    } else {
        None
    };
    let input = resolve_input(args.input, stdin_input, || {
        if noninteractive {
            return Err(CruiseError::Other(
                "no input provided: stdin is not a terminal and no --input flag was given"
                    .to_string(),
            ));
        }
        prompt_for_plan_input()
    })?;

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

    // Set up session.
    let manager = SessionManager::new(get_cruise_home()?);

    let session_id = SessionManager::new_session_id();
    let base_dir = std::env::current_dir()?;
    let mut session = SessionState::new(
        session_id.clone(),
        base_dir,
        source.display_string(),
        input.trim().to_string(),
    );
    session.config_path = source.path().cloned();
    manager.create(&session)?;

    // Save config.yaml copy to session dir only for built-in config (no external file path).
    if session.config_path.is_none() {
        let session_dir = manager.sessions_dir().join(&session_id);
        std::fs::write(session_dir.join("config.yaml"), &yaml)?;
    }

    // Set up variables with the session plan path.
    let plan_path = session.plan_path(&manager.sessions_dir());

    let mut vars = VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path.clone());

    // Run the built-in plan step (LLM writes plan.md).
    let plan_model = config.plan_model.clone().or_else(|| config.model.clone());
    let plan_prompt = vars.resolve(PLAN_PROMPT_TEMPLATE)?;

    eprintln!(
        "\n{} {}",
        style("▶").cyan().bold(),
        style("[plan] creating plan...").bold()
    );

    let plan_step = PromptStep {
        model: plan_model,
        prompt: plan_prompt,
        instruction: None,
    };

    let spinner = crate::spinner::Spinner::start("Cruising...");
    let env = std::collections::HashMap::new();
    let result = {
        let on_retry = |msg: &str| spinner.suspend(|| eprintln!("{msg}"));
        let effective_model = plan_step.model.as_deref().or(config.model.as_deref());
        let has_placeholder = config.command.iter().any(|s| s.contains("{model}"));
        let (resolved_command, model_arg) = if has_placeholder {
            (
                resolve_command_with_model(&config.command, effective_model),
                None,
            )
        } else {
            (config.command.clone(), effective_model.map(str::to_string))
        };
        crate::step::prompt::run_prompt(
            &resolved_command,
            model_arg.as_deref(),
            &plan_step.prompt,
            args.rate_limit_retries,
            &env,
            Some(&on_retry),
        )
        .await
    };
    drop(spinner);
    let _output = result?.output;

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
    // Read the plan once up front; re-read only after Fix modifies it.
    let mut plan_content = match read_plan_non_empty(plan_path) {
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
            session.approve();
            manager.save(session)?;
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
                session.approve();
                manager.save(session)?;
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
                run_fix_plan(config, vars, rate_limit_retries).await?;
                plan_content = read_plan_non_empty(plan_path)?;
            }
            "Ask" => {
                let text = match prompt_multiline("Your question:")? {
                    InputResult::Submitted(t) => t,
                    InputResult::Cancelled => continue,
                };
                vars.set_prev_input(Some(text));
                run_ask_plan(config, vars, rate_limit_retries).await?;
            }
            "Execute now" => {
                session.approve();
                manager.save(session)?;
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

/// Replan an existing session using the built-in fix-plan prompt.
pub(crate) async fn replan_session(
    manager: &SessionManager,
    session: &SessionState,
    feedback: String,
    rate_limit_retries: usize,
) -> Result<()> {
    let config = manager.load_config(session)?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path);
    vars.set_prev_input(Some(feedback));
    run_fix_plan(&config, &mut vars, rate_limit_retries).await
}

/// Run the built-in fix-plan prompt.
async fn run_fix_plan(
    config: &WorkflowConfig,
    vars: &mut VariableStore,
    rate_limit_retries: usize,
) -> Result<()> {
    run_plan_prompt(
        config,
        vars,
        rate_limit_retries,
        FIX_PLAN_PROMPT_TEMPLATE,
        "[fix-plan] applying fixes...",
    )
    .await
}

/// Run the built-in ask-plan prompt.
async fn run_ask_plan(
    config: &WorkflowConfig,
    vars: &mut VariableStore,
    rate_limit_retries: usize,
) -> Result<()> {
    run_plan_prompt(
        config,
        vars,
        rate_limit_retries,
        ASK_PLAN_PROMPT_TEMPLATE,
        "[ask-plan] answering question...",
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
) -> Result<()> {
    let prompt = vars.resolve(template)?;
    let step = PromptStep {
        model: config.plan_model.clone().or_else(|| config.model.clone()),
        prompt,
        instruction: None,
    };
    let env = std::collections::HashMap::new();
    eprintln!("\n{} {}", style("▶").cyan().bold(), style(label).bold());
    let compiled = crate::workflow::compile(config.clone())?;
    run_prompt_step(vars, &compiled, &step, rate_limit_retries, &env).await?;
    Ok(())
}

/// Read `plan.md` and return its content, or error if the file is missing or
/// contains only whitespace.  This is the canonical validation point that
/// prevents empty plans from reaching the approve menu.
fn read_plan_non_empty(plan_path: &std::path::Path) -> Result<String> {
    let content = std::fs::read_to_string(plan_path).map_err(|e| {
        CruiseError::Other(format!(
            "failed to read generated plan {}: {}",
            plan_path.display(),
            e
        ))
    })?;

    if content.trim().is_empty() {
        return Err(CruiseError::Other(format!(
            "generated plan {} is empty",
            plan_path.display()
        )));
    }

    Ok(content)
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
    use tempfile::TempDir;

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

    // -----------------------------------------------------------------------
    // read_plan_non_empty() unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_read_plan_non_empty_returns_err_when_file_missing() {
        // Given: a path that does not exist
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = tmp.path().join("plan.md");

        // When
        let result = read_plan_non_empty(&plan_path);

        // Then: Err is returned
        assert!(result.is_err(), "expected Err for missing file, got Ok");
    }

    #[test]
    fn test_read_plan_non_empty_returns_err_when_file_is_empty() {
        // Given: plan file exists but is completely empty
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "").unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = read_plan_non_empty(&plan_path);

        // Then: Err is returned
        assert!(result.is_err(), "expected Err for empty file, got Ok");
    }

    #[test]
    fn test_read_plan_non_empty_returns_err_when_file_is_whitespace_only() {
        // Given: plan file contains only whitespace characters
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "   \n\t\n  ").unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = read_plan_non_empty(&plan_path);

        // Then: Err is returned (whitespace-only is treated as empty)
        assert!(
            result.is_err(),
            "expected Err for whitespace-only file, got Ok"
        );
    }

    #[test]
    fn test_read_plan_non_empty_returns_content_when_file_has_real_content() {
        // Given: plan file has meaningful content
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = tmp.path().join("plan.md");
        let content = "# Implementation Plan\n\nStep 1: do something\n";
        std::fs::write(&plan_path, content).unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = read_plan_non_empty(&plan_path);

        // Then: Ok with the original content is returned
        assert_eq!(result.unwrap_or_else(|e| panic!("{e:?}")), content);
    }

    // ── resolve_input with multiline stdin ───────────────────────────────────

    #[test]
    fn test_resolve_input_multiline_from_stdin_preserves_internal_newlines() {
        // Given: 複数行を含む stdin 入力（pipe 等）
        let stdin = "line1\nline2\nline3\n".to_string();
        let result = resolve_input(None, Some(stdin), || {
            panic!("interactive prompt should not run")
        });
        // Then: 先頭・末尾の空白のみ trim され、内部改行は保持される
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")),
            "line1\nline2\nline3"
        );
    }

    #[test]
    fn test_resolve_input_multiline_trims_only_leading_trailing_whitespace() {
        // Given: 先頭と末尾に余分な空白を持つ複数行 stdin 入力
        let stdin = "  line1\nline2  \n".to_string();
        let result = resolve_input(None, Some(stdin), || {
            panic!("interactive prompt should not run")
        });
        // Then: 先頭・末尾の空白のみ除去され、中間の改行は保持される
        assert_eq!(result.unwrap_or_else(|e| panic!("{e:?}")), "line1\nline2");
    }
}
