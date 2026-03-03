use std::collections::HashMap;

use console::style;

use crate::cli::Args;
use crate::condition::{evaluate_if_condition, should_skip};
use crate::config::{SkipCondition, WorkflowConfig};

/// Variable name that maps to the plan file.
const PLAN_VAR_NAME: &str = "plan";
use crate::error::{CruiseError, Result};
use crate::file_tracker::FileTracker;
use crate::step::command::run_commands;
use crate::step::option::run_option;
use crate::step::prompt::run_prompt;
use crate::step::{CommandStep, OptionStep, PromptStep, StepKind};
use crate::variable::VariableStore;

/// Load the config and run the workflow state machine.
pub async fn run(args: Args) -> Result<()> {
    let config_path = &args.config;
    let yaml = std::fs::read_to_string(config_path)
        .map_err(|_| CruiseError::ConfigNotFound(config_path.clone()))?;

    let config = WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;

    if args.dry_run {
        return print_dry_run(&config, args.from.as_deref());
    }

    let input = args.input.unwrap_or_default();
    let mut vars = VariableStore::new(input.clone());

    if let Some(plan_path) = &config.plan {
        vars.set_named_file(PLAN_VAR_NAME, plan_path.clone());
    }

    let mut tracker = FileTracker::with_root(std::env::current_dir()?);

    // Edge counters for loop protection: (from, to) → visit count.
    let mut edge_counts: HashMap<(String, String), usize> = HashMap::new();

    let start_step = if let Some(from) = args.from {
        from
    } else {
        config
            .steps
            .keys()
            .next()
            .ok_or_else(|| CruiseError::Other("no steps defined".to_string()))?
            .clone()
    };

    let mut current_step = start_step;

    loop {
        let step_config = config
            .steps
            .get(&current_step)
            .ok_or_else(|| CruiseError::StepNotFound(current_step.clone()))?;

        // Determine if this step should be skipped and why.
        let skip_msg = if should_skip(&step_config.skip, &vars)? {
            Some(format!("skipping: {}", current_step))
        } else if let Some(ref if_cond) = step_config.if_condition {
            if !evaluate_if_condition(if_cond, &tracker)? {
                Some(format!("condition not met, skipping: {}", current_step))
            } else {
                None
            }
        } else {
            None
        };

        if let Some(msg) = skip_msg {
            eprintln!("{} {}", style("→").yellow(), msg);
            match get_next_step(&config, &current_step, None) {
                Some(next) => {
                    current_step = next;
                    continue;
                }
                None => break,
            }
        }

        eprintln!(
            "\n{} {}",
            style("▶").cyan().bold(),
            style(&current_step).bold()
        );

        let step_next = step_config.next.clone();
        let kind = StepKind::try_from(step_config.clone())?;

        let option_next = match &kind {
            StepKind::Prompt(step) => {
                run_prompt_step(&mut vars, &config, step, args.rate_limit_retries).await?;
                None
            }
            StepKind::Command(step) => {
                run_command_step(&mut vars, step, args.rate_limit_retries).await?;
                // Snapshot after the command so `if: file-changed` can detect diffs.
                tracker.take_snapshot(&current_step)?;
                None
            }
            StepKind::Option(step) => run_option_step(&mut vars, step)?,
        };

        let effective_next = option_next.or(step_next);
        let next_step = get_next_step(&config, &current_step, effective_next.as_deref());

        // Loop protection.
        if let Some(ref next) = next_step {
            let edge = (current_step.clone(), next.clone());
            let count = edge_counts.entry(edge).or_insert(0);
            *count += 1;
            if *count > args.max_retries {
                return Err(CruiseError::LoopProtection(
                    current_step,
                    next.clone(),
                    args.max_retries,
                ));
            }
        }

        match next_step {
            Some(next) => current_step = next,
            None => break,
        }
    }

    eprintln!("\n{}", style("✓ workflow complete").green().bold());
    Ok(())
}

/// Resolve the `{model}` placeholder in a command, or strip `--model {model}` if no model.
///
/// - `Some(model)`: replaces every `{model}` occurrence with the model string.
/// - `None`: removes arguments containing `{model}` and any immediately-preceding `--model` flag.
fn resolve_command_with_model(command: &[String], effective_model: Option<&str>) -> Vec<String> {
    if let Some(model) = effective_model {
        command
            .iter()
            .map(|arg| arg.replace("{model}", model))
            .collect()
    } else {
        let mut result = Vec::new();
        let mut i = 0;
        while i < command.len() {
            let arg = &command[i];
            if arg == "--model" {
                // Only remove the pair if the next arg is a {model} template placeholder.
                if command
                    .get(i + 1)
                    .is_some_and(|next| next.contains("{model}"))
                {
                    i += 2;
                } else {
                    result.push(arg.clone());
                    i += 1;
                }
            } else if arg.starts_with("--model=") {
                // Always drop --model=VALUE when effective_model is None.
                i += 1;
            } else if arg.contains("{model}") {
                i += 1;
            } else {
                result.push(arg.clone());
                i += 1;
            }
        }
        result
    }
}

/// Execute a prompt step, updating variable state.
async fn run_prompt_step(
    vars: &mut VariableStore,
    config: &WorkflowConfig,
    step: &PromptStep,
    rate_limit_retries: usize,
) -> Result<()> {
    // Display instruction and description.
    if let Some(inst) = &step.instruction {
        let resolved = vars.resolve(inst)?;
        eprintln!("  {}", style(resolved).dim());
    }
    if let Some(desc) = &step.description {
        let resolved = vars.resolve(desc)?;
        eprintln!("  {}", style(resolved).dim());
    }

    let prompt = vars.resolve(&step.prompt)?;

    // Effective model: step-level overrides config-level default.
    let effective_model = step.model.as_deref().or(config.model.as_deref());

    // If the command contains a {model} placeholder, resolve it there and pass
    // model=None to run_prompt (backward-compat path). Otherwise pass model
    // directly so execute_prompt appends --model as before.
    let has_placeholder = config.command.iter().any(|s| s.contains("{model}"));

    let (resolved_command, model_arg) = if has_placeholder {
        (
            resolve_command_with_model(&config.command, effective_model),
            None,
        )
    } else {
        (config.command.clone(), effective_model.map(str::to_string))
    };

    let result = run_prompt(
        &resolved_command,
        model_arg.as_deref(),
        &prompt,
        rate_limit_retries,
    )
    .await?;

    if let Some(output_var) = &step.output {
        // Write to the plan file if this output is bound to it.
        if let Some(plan_path) = &config.plan
            && output_var == PLAN_VAR_NAME
        {
            std::fs::write(plan_path, &result.output)?;
        }
        vars.set_named_value(output_var, result.output.clone());
    }

    vars.set_prev_output(Some(result.output));
    vars.set_prev_input(None);

    Ok(())
}

/// Execute a command step, updating variable state.
async fn run_command_step(
    vars: &mut VariableStore,
    step: &CommandStep,
    rate_limit_retries: usize,
) -> Result<()> {
    if let Some(desc) = &step.description {
        let resolved = vars.resolve(desc)?;
        eprintln!("  {}", style(resolved).dim());
    }

    // Resolve variables in each command, then display and run.
    let cmds: Vec<String> = step
        .command
        .iter()
        .map(|c| vars.resolve(c))
        .collect::<Result<Vec<_>>>()?;

    for cmd in &cmds {
        eprintln!("  {} {}", style("$").dim(), style(cmd).dim());
    }

    let result = run_commands(&cmds, rate_limit_retries).await?;

    vars.set_prev_success(Some(result.success));
    vars.set_prev_stderr(Some(result.stderr));
    vars.set_prev_output(None);
    vars.set_prev_input(None);

    Ok(())
}

/// Execute an option step, updating variable state and returning the chosen next step.
fn run_option_step(vars: &mut VariableStore, step: &OptionStep) -> Result<Option<String>> {
    let desc = step
        .description
        .as_ref()
        .map(|d| vars.resolve(d))
        .transpose()?;

    let result = run_option(&step.choices, desc.as_deref())?;

    if let Some(ref text) = result.text_input {
        vars.set_prev_input(Some(text.clone()));
    }
    vars.set_prev_output(None);

    Ok(result.next_step)
}

/// Determine the next step: explicit next > IndexMap order > None (end).
fn get_next_step(
    config: &WorkflowConfig,
    current: &str,
    explicit_next: Option<&str>,
) -> Option<String> {
    if let Some(next) = explicit_next {
        return Some(next.to_string());
    }

    let mut found = false;
    for key in config.steps.keys() {
        if found {
            return Some(key.clone());
        }
        if key == current {
            found = true;
        }
    }

    None
}

/// Print a dry-run summary of the workflow flow.
fn print_dry_run(config: &WorkflowConfig, from: Option<&str>) -> Result<()> {
    println!("{}", style("=== Dry Run: Workflow Flow ===").bold());
    println!("command: {}", config.command.join(" "));

    if let Some(model) = &config.model {
        println!("model: {}", model);
    }

    if let Some(plan) = &config.plan {
        println!("plan: {}", plan.display());
    }

    println!("\nsteps:");

    let mut started = from.is_none();

    for (name, step) in &config.steps {
        if !started {
            if Some(name.as_str()) == from {
                started = true;
            } else {
                continue;
            }
        }

        let kind_label = if step.prompt.is_some() {
            "prompt"
        } else if step.command.is_some() && step.option.is_none() {
            "command"
        } else if step.option.is_some() {
            "option"
        } else {
            "unknown"
        };

        print!("  {} [{}]", style(name).bold(), style(kind_label).cyan());

        match &step.skip {
            Some(SkipCondition::Static(true)) => print!(" {}", style("(skip)").yellow()),
            Some(SkipCondition::Variable(v)) => {
                print!(" {}", style(format!("(skip if {v})")).yellow())
            }
            _ => {}
        }
        if step.if_condition.is_some() {
            print!(" {}", style("(conditional)").yellow());
        }
        if let Some(next) = &step.next {
            print!(" → {}", style(next).green());
        }

        println!();

        if let Some(desc) = &step.description {
            println!("    {}", style(desc).dim());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;

    fn make_args(config: &str, input: Option<&str>, from: Option<&str>, dry_run: bool) -> Args {
        crate::cli::Args {
            input: input.map(|s| s.to_string()),
            config: config.to_string(),
            from: from.map(|s| s.to_string()),
            max_retries: 10,
            rate_limit_retries: 0,
            dry_run,
        }
    }

    #[test]
    fn test_resolve_command_with_model_some() {
        let command = vec![
            "claude".to_string(),
            "--model".to_string(),
            "{model}".to_string(),
            "-p".to_string(),
        ];
        let resolved = resolve_command_with_model(&command, Some("sonnet"));
        assert_eq!(resolved, vec!["claude", "--model", "sonnet", "-p"]);
    }

    #[test]
    fn test_resolve_command_with_model_none() {
        let command = vec![
            "claude".to_string(),
            "--model".to_string(),
            "{model}".to_string(),
            "-p".to_string(),
        ];
        let resolved = resolve_command_with_model(&command, None);
        assert_eq!(resolved, vec!["claude", "-p"]);
    }

    #[test]
    fn test_resolve_command_no_placeholder() {
        let command = vec!["claude".to_string(), "-p".to_string()];
        let resolved = resolve_command_with_model(&command, Some("opus"));
        assert_eq!(resolved, vec!["claude", "-p"]);
    }

    #[test]
    fn test_resolve_command_model_equals_form_none() {
        // --model=value form is also removed when None
        let command = vec![
            "claude".to_string(),
            "--model=claude-opus-4-6".to_string(),
            "-p".to_string(),
        ];
        let resolved = resolve_command_with_model(&command, None);
        assert_eq!(resolved, vec!["claude", "-p"]);
    }

    #[test]
    fn test_resolve_command_model_equals_form_some() {
        // --model=value form does not contain {model}, so it is preserved when Some
        let command = vec![
            "claude".to_string(),
            "--model={model}".to_string(),
            "-p".to_string(),
        ];
        let resolved = resolve_command_with_model(&command, Some("claude-opus-4-6"));
        assert_eq!(resolved, vec!["claude", "--model=claude-opus-4-6", "-p"]);
    }

    #[test]
    fn test_get_next_step_sequential() {
        let yaml = r#"
command: [echo]
steps:
  step_a:
    command: echo a
  step_b:
    command: echo b
  step_c:
    command: echo c
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        assert_eq!(
            get_next_step(&config, "step_a", None),
            Some("step_b".to_string())
        );
        assert_eq!(
            get_next_step(&config, "step_b", None),
            Some("step_c".to_string())
        );
        assert_eq!(get_next_step(&config, "step_c", None), None);
    }

    #[test]
    fn test_get_next_step_explicit() {
        let yaml = r#"
command: [echo]
steps:
  step_a:
    command: echo a
  step_b:
    command: echo b
  step_c:
    command: echo c
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        // Explicit next takes priority over sequential order.
        assert_eq!(
            get_next_step(&config, "step_a", Some("step_c")),
            Some("step_c".to_string())
        );
    }

    #[test]
    fn test_get_next_step_not_found() {
        let yaml = r#"
command: [echo]
steps:
  only_step:
    command: echo hello
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap();
        assert_eq!(get_next_step(&config, "only_step", None), None);
    }

    #[tokio::test]
    async fn test_run_command_workflow() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo hello"
  step2:
    command: "echo world"
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), yaml).unwrap();

        let args = make_args(tmp.path().to_str().unwrap(), Some("test"), None, false);
        let result = run(args).await;
        assert!(result.is_ok(), "workflow run failed: {:?}", result);
    }

    #[tokio::test]
    async fn test_run_command_list_workflow() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command:
      - "echo hello"
      - "echo world"
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), yaml).unwrap();

        let args = make_args(tmp.path().to_str().unwrap(), Some("test"), None, false);
        let result = run(args).await;
        assert!(result.is_ok(), "workflow run failed: {:?}", result);
    }

    #[tokio::test]
    async fn test_run_skip_step() {
        let yaml = r#"
command: [echo]
steps:
  skipped:
    command: "exit 1"
    skip: true
  normal:
    command: "echo done"
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), yaml).unwrap();

        let args = make_args(tmp.path().to_str().unwrap(), None, None, false);
        // The skipped step has `exit 1` but should not be executed.
        let result = run(args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_dynamic_skip_step() {
        let yaml = r#"
command: [echo]
steps:
  first:
    command: "echo success"
  skipped:
    command: "exit 1"
    skip: prev.success
  normal:
    command: "echo done"
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), yaml).unwrap();

        let args = make_args(tmp.path().to_str().unwrap(), None, None, false);
        // "first" succeeds → prev.success = true → "skipped" step is skipped.
        let result = run(args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_run_from_step() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "exit 1"
  step2:
    command: "echo hello"
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), yaml).unwrap();

        // Starting from step2 skips the failing step1.
        let args = make_args(tmp.path().to_str().unwrap(), None, Some("step2"), false);
        let result = run(args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_loop_protection() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo loop"
    next: step1
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), yaml).unwrap();

        let mut args = make_args(tmp.path().to_str().unwrap(), None, None, false);
        args.max_retries = 2;
        let result = run(args).await;
        // Loop protection should trigger an error.
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_dry_run() {
        let yaml = r#"
command: [claude, -p]
steps:
  plan:
    prompt: "Plan: {input}"
  implement:
    command: "cargo build"
    if:
      file-changed: plan
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), yaml).unwrap();

        let args = make_args(tmp.path().to_str().unwrap(), Some("feature"), None, true);
        let result = run(args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_dry_run_with_skip_variable() {
        let yaml = r#"
command: [claude, -p]
steps:
  step1:
    command: "echo hi"
  step2:
    command: "echo skip me"
    skip: prev.success
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), yaml).unwrap();

        let args = make_args(tmp.path().to_str().unwrap(), None, None, true);
        let result = run(args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_config_not_found() {
        let args = make_args("nonexistent.yaml", None, None, false);
        let result = run(args).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_variable_resolution_in_command() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo {input}"
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), yaml).unwrap();

        let args = make_args(tmp.path().to_str().unwrap(), Some("hello"), None, false);
        let result = run(args).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_variable_resolution_in_description() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo hello"
    description: "Input is: {input}"
"#;
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), yaml).unwrap();

        let args = make_args(tmp.path().to_str().unwrap(), Some("world"), None, false);
        let result = run(args).await;
        assert!(result.is_ok());
    }
}
