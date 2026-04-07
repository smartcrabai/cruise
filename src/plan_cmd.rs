use std::collections::HashSet;
use std::fmt;
use std::io::IsTerminal;

use console::style;
use inquire::InquireError;

use crate::cli::{DEFAULT_MAX_RETRIES, PlanArgs};
use crate::config::{WorkflowConfig, validate_config};
use crate::engine::{resolve_command_with_model, run_prompt_step};
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};
use crate::session::{PLAN_VAR, SessionManager, SessionState, get_cruise_home};
use crate::step::PromptStep;
use crate::variable::VariableStore;
use crate::workflow::{SkippableStepNode, list_skippable_steps};

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
            None,
            None,
        )
        .await
    };
    drop(spinner);
    let prompt_result = result?;
    if let Err(e) = crate::metadata::resolve_plan_content(
        &plan_path,
        &prompt_result.output,
        &prompt_result.stderr,
    ) {
        eprintln!(
            "\n{} Plan generation failed. Session {} discarded.",
            style("✗").red().bold(),
            session_id
        );
        if let Err(del_err) = manager.delete(&session_id) {
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

/// Read task input from CLI arg, piped stdin, or interactive prompt.
fn read_plan_input(input: Option<String>, noninteractive: bool) -> Result<String> {
    let stdin_input = if input.is_none() && noninteractive {
        use std::io::Read;
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

#[derive(Clone)]
struct FlatNode {
    label: String,
    expanded_step_ids: Vec<String>,
}

impl fmt::Display for FlatNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

fn flatten_nodes(nodes: &[SkippableStepNode]) -> Vec<FlatNode> {
    let mut flat = Vec::new();
    flatten_nodes_into(nodes, 0, &mut flat);
    flat
}

fn flatten_nodes_into(nodes: &[SkippableStepNode], depth: usize, flat: &mut Vec<FlatNode>) {
    for node in nodes {
        let label = if depth == 0 {
            node.id.clone()
        } else {
            node.id
                .rsplit('/')
                .next()
                .unwrap_or(node.id.as_str())
                .to_string()
        };
        flat.push(FlatNode {
            label: format!("{}{}", "  ".repeat(depth), label),
            expanded_step_ids: node.expanded_step_ids.clone(),
        });
        flatten_nodes_into(&node.children, depth + 1, flat);
    }
}

fn collect_expanded_ids(selected_nodes: Vec<FlatNode>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut expanded_ids = Vec::new();

    for expanded_id in selected_nodes
        .into_iter()
        .flat_map(|node| node.expanded_step_ids)
    {
        if seen.insert(expanded_id.clone()) {
            expanded_ids.push(expanded_id);
        }
    }

    expanded_ids
}

/// Present a `MultiSelect` prompt so the user can choose which steps to skip.
/// Returns an empty `Vec` when there are no steps, the user cancels, or
/// an interruption is received (so the approve flow can continue unblocked).
fn select_steps_to_skip(config: &WorkflowConfig) -> Result<Vec<String>> {
    let nodes = list_skippable_steps(config)?;
    if nodes.is_empty() {
        return Ok(vec![]);
    }

    let flat = flatten_nodes(&nodes);

    match inquire::MultiSelect::new("Steps to skip (Space to toggle, Enter to confirm):", flat)
        .with_help_message("No selection = run all steps")
        .prompt()
    {
        Ok(selected_nodes) => Ok(collect_expanded_ids(selected_nodes)),
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
                session.skipped_steps = select_steps_to_skip(config)?;
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
                run_fix_plan(config, vars, rate_limit_retries).await?;
                plan_content = crate::metadata::read_plan_markdown(plan_path)?;
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
                session.skipped_steps = select_steps_to_skip(config)?;
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
    run_plan_prompt(
        config,
        vars,
        rate_limit_retries,
        PLAN_PROMPT_TEMPLATE,
        "[plan] creating plan...",
    )
    .await
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
    run_fix_plan(&config, &mut vars, rate_limit_retries).await?;

    let plan_markdown = crate::metadata::read_plan_markdown(&plan_path)?;
    crate::metadata::refresh_session_title_from_plan(session, &plan_markdown);
    manager.save(session)?;
    Ok(())
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
    run_prompt_step(vars, &compiled, &step, rate_limit_retries, &env, None, None).await?;
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

    #[test]
    fn test_collect_expanded_ids_deduplicates_parent_and_child_selection() {
        let selected = vec![
            FlatNode {
                label: "review-pass".to_string(),
                expanded_step_ids: vec![
                    "review-pass/simplify".to_string(),
                    "review-pass/coderabbit".to_string(),
                ],
            },
            FlatNode {
                label: "  simplify".to_string(),
                expanded_step_ids: vec!["review-pass/simplify".to_string()],
            },
        ];

        assert_eq!(
            collect_expanded_ids(selected),
            vec!["review-pass/simplify", "review-pass/coderabbit"]
        );
    }
}
