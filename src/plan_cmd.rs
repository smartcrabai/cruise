use std::io::IsTerminal;

use console::style;
use inquire::InquireError;

use crate::cli::PlanArgs;
use crate::config::{WorkflowConfig, validate_groups};
use crate::engine::{resolve_command_with_model, run_prompt_step};
use crate::error::{CruiseError, Result};
use crate::session::{SessionManager, SessionPhase, SessionState, get_cruise_home};
use crate::step::PromptStep;
use crate::variable::VariableStore;

/// Name of the variable that holds the plan file path.
const PLAN_VAR: &str = "plan";

pub async fn run(args: PlanArgs) -> Result<()> {
    // Resolve input: CLI arg, or read from stdin if piped.
    let input = resolve_input(args.input)?;

    if args.dry_run {
        eprintln!(
            "{}",
            style(format!("Would plan: \"{}\"", input.trim())).dim()
        );
        return Ok(());
    }

    // Resolve config.
    let (yaml, source) = crate::resolver::resolve_config(args.config.as_deref())?;
    eprintln!("{}", style(source.display_string()).dim());
    let config = WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
    validate_groups(&config)?;

    // Set up session.
    let manager = SessionManager::new(get_cruise_home()?);

    // Auto-cleanup old sessions.
    if let Err(e) = manager.cleanup_old(3) {
        eprintln!("warning: cleanup failed: {}", e);
    }

    let session_id = SessionManager::new_session_id();
    let base_dir = std::env::current_dir()?;
    let mut session = SessionState::new(
        session_id.clone(),
        base_dir,
        source.display_string(),
        input.trim().to_string(),
    );
    manager.create(&session)?;

    // Save config.yaml copy to session dir.
    let session_dir = manager.sessions_dir().join(&session_id);
    std::fs::write(session_dir.join("config.yaml"), &yaml)?;

    // Set up variables with the session plan path.
    let plan_path = session.plan_path(&manager.sessions_dir());

    let mut vars = VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path.clone());

    // Run the built-in plan step (LLM writes plan.md).
    let plan_model = config.plan_model.clone().or_else(|| config.model.clone());
    let plan_prompt = format!(
        "I am trying to implement the following features. Create an implementation plan and write it to {}.\n---\n{}",
        plan_path.display(),
        session.input
    );

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
        let on_retry = |msg: &str| spinner.suspend(|| eprintln!("{}", msg));
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
    )
    .await
}

/// Interactive approve-plan loop: show plan, let user approve/fix/ask/execute.
async fn run_approve_loop(
    config: &WorkflowConfig,
    manager: &SessionManager,
    session: &mut SessionState,
    plan_path: &std::path::Path,
    vars: &mut VariableStore,
    rate_limit_retries: usize,
) -> Result<()> {
    loop {
        // Read and display the current plan.
        let plan_content = match std::fs::read_to_string(plan_path) {
            Ok(c) if !c.trim().is_empty() => c,
            _ => "(plan file is empty or not found)".to_string(),
        };
        crate::display::print_bordered(&plan_content, Some("plan.md"));

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
                session.phase = SessionPhase::Planned;
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
                let text = match inquire::Text::new("Describe the changes needed:").prompt() {
                    Ok(t) => t,
                    Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                        continue;
                    }
                    Err(e) => return Err(CruiseError::Other(format!("input error: {e}"))),
                };
                vars.set_prev_input(Some(text));
                run_fix_plan(config, vars, plan_path, rate_limit_retries).await?;
            }
            "Ask" => {
                let text = match inquire::Text::new("Your question:").prompt() {
                    Ok(t) => t,
                    Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                        continue;
                    }
                    Err(e) => return Err(CruiseError::Other(format!("input error: {e}"))),
                };
                vars.set_prev_input(Some(text));
                run_ask_plan(config, vars, plan_path, rate_limit_retries).await?;
            }
            "Execute now" => {
                session.phase = SessionPhase::Planned;
                manager.save(session)?;
                eprintln!(
                    "\n{} Executing session {}...",
                    style("→").cyan(),
                    session.id
                );
                let run_args = crate::cli::RunArgs {
                    session: Some(session.id.clone()),
                    max_retries: 10,
                    rate_limit_retries,
                    keep_worktree: false,
                    dry_run: false,
                };
                return crate::run_cmd::run(run_args).await;
            }
            _ => {}
        }
    }
}

/// Run the built-in fix-plan prompt.
async fn run_fix_plan(
    config: &WorkflowConfig,
    vars: &mut VariableStore,
    plan_path: &std::path::Path,
    rate_limit_retries: usize,
) -> Result<()> {
    let prompt_template = format!(
        "The user has requested the following changes to the {} implementation plan. Make the modifications:\n{{prev.input}}",
        plan_path.display()
    );
    let prompt = vars.resolve(&prompt_template)?;
    let fix_model = config.plan_model.clone().or_else(|| config.model.clone());
    let step = PromptStep {
        model: fix_model,
        prompt,
        instruction: None,
    };
    let env = std::collections::HashMap::new();
    eprintln!(
        "\n{} {}",
        style("▶").cyan().bold(),
        style("[fix-plan] applying fixes...").bold()
    );
    run_prompt_step(vars, config, &step, rate_limit_retries, &env).await?;
    Ok(())
}

/// Run the built-in ask-plan prompt.
async fn run_ask_plan(
    config: &WorkflowConfig,
    vars: &mut VariableStore,
    plan_path: &std::path::Path,
    rate_limit_retries: usize,
) -> Result<()> {
    let prompt_template = format!(
        "The user has the following questions about the implementation plan for {}. Provide answers:\n{{prev.input}}",
        plan_path.display()
    );
    let prompt = vars.resolve(&prompt_template)?;
    let step = PromptStep {
        model: config.plan_model.clone().or_else(|| config.model.clone()),
        prompt,
        instruction: None,
    };
    let env = std::collections::HashMap::new();
    eprintln!(
        "\n{} {}",
        style("▶").cyan().bold(),
        style("[ask-plan] answering question...").bold()
    );
    run_prompt_step(vars, config, &step, rate_limit_retries, &env).await?;
    Ok(())
}

/// Resolve user input from CLI arg or stdin pipe.
fn resolve_input(arg: Option<String>) -> Result<String> {
    if let Some(input) = arg {
        return Ok(input);
    }
    // Read from stdin if piped.
    if !std::io::stdin().is_terminal() {
        use std::io::Read;
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).ok();
        let trimmed = input.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }
    // Prompt interactively.
    inquire::Text::new("What would you like to implement?")
        .prompt()
        .map_err(|e| CruiseError::Other(format!("input error: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_input_from_arg() {
        let result = resolve_input(Some("add feature X".to_string()));
        assert_eq!(result.unwrap(), "add feature X");
    }
}
