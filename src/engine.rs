use std::collections::HashMap;
use std::time::Instant;

use console::style;

use crate::condition::should_skip;
use crate::config::{SkipCondition, WorkflowConfig};
use crate::error::{CruiseError, Result};
use crate::file_tracker::FileTracker;
use crate::step::command::run_commands;
use crate::step::option::run_option;
use crate::step::prompt::run_prompt;
use crate::step::{CommandStep, OptionStep, PromptStep, StepKind};
use crate::variable::VariableStore;

/// Result of a completed `execute_steps` run.
#[derive(Debug)]
#[allow(dead_code)]
pub struct ExecutionResult {
    pub steps_run: usize,
    pub steps_skipped: usize,
    pub steps_failed: usize,
}

/// Execute workflow steps starting from `start_step`.
///
/// `on_step_start` is called with the step name before each step executes,
/// allowing the caller to persist the current step for resume support.
pub async fn execute_steps(
    config: &WorkflowConfig,
    vars: &mut VariableStore,
    tracker: &mut FileTracker,
    start_step: &str,
    max_retries: usize,
    rate_limit_retries: usize,
    on_step_start: &dyn Fn(&str) -> Result<()>,
) -> Result<ExecutionResult> {
    // Edge counters for loop protection: (from, to) → visit count.
    let mut edge_counts: HashMap<(String, String), usize> = HashMap::new();
    let mut current_step = start_step.to_string();
    let mut total_steps = config.steps.len();
    let mut step_index = 0usize;
    let workflow_start = Instant::now();
    let mut steps_run = 0usize;
    let mut steps_skipped = 0usize;
    let mut steps_failed = 0usize;

    // (A) Pre-calculate group info.
    let mut group_first: HashMap<String, String> = HashMap::new();
    let mut group_last: HashMap<String, String> = HashMap::new();
    let mut group_size: HashMap<String, usize> = HashMap::new();
    let mut step_to_group: HashMap<String, String> = HashMap::new();
    let mut group_retry_counts: HashMap<String, usize> = HashMap::new();
    for (name, step) in &config.steps {
        if let Some(group_name) = &step.group {
            step_to_group.insert(name.clone(), group_name.clone());
            group_first
                .entry(group_name.clone())
                .or_insert_with(|| name.clone());
            group_last.insert(group_name.clone(), name.clone());
            *group_size.entry(group_name.clone()).or_insert(0) += 1;
        }
    }

    loop {
        let step_config = config
            .steps
            .get(&current_step)
            .ok_or_else(|| CruiseError::StepNotFound(current_step.clone()))?;

        let step_group_name = step_to_group.get(&current_step).map(|s| s.as_str());

        // (B) Group max_retries skip check (before individual should_skip).
        if let Some(group_name) = step_group_name {
            let is_first = group_first.get(group_name) == Some(&current_step);
            if is_first
                && let Some(group_cfg) = config.groups.get(group_name)
                && let Some(max) = group_cfg.max_retries
                && group_retry_counts.get(group_name).copied().unwrap_or(0) >= max
            {
                eprintln!(
                    "  {} group '{}' max retries ({}) reached, skipping",
                    style("→").yellow(),
                    group_name,
                    max
                );
                let last_step = group_last
                    .get(group_name)
                    .cloned()
                    .unwrap_or_else(|| current_step.clone());
                let count = group_size.get(group_name).copied().unwrap_or(1);
                step_index += count;
                steps_skipped += count;
                match get_next_step(config, &last_step, None) {
                    Some(next) => {
                        current_step = next;
                        continue;
                    }
                    None => break,
                }
            }
        }

        let skip_msg = if should_skip(&step_config.skip, vars)? {
            Some(format!("skipping: {}", current_step))
        } else {
            None
        };

        if let Some(msg) = skip_msg {
            step_index += 1;
            steps_skipped += 1;
            eprintln!("{} {}", style("→").yellow(), msg);
            match get_next_step(config, &current_step, None) {
                Some(next) => {
                    current_step = next;
                    continue;
                }
                None => break,
            }
        }

        step_index += 1;
        total_steps = total_steps.max(step_index);
        eprintln!(
            "\n{} {}",
            style("▶").cyan().bold(),
            style(format!(
                "[{}/{}] {}",
                step_index, total_steps, &current_step
            ))
            .bold()
        );

        on_step_start(&current_step)?;

        let step_start = Instant::now();
        let step_next = step_config.next.clone();
        let merged_env = resolve_env(&config.env, &step_config.env, vars)?;
        let kind = StepKind::try_from(step_config.clone())?;

        // Pre-execution snapshot so we can detect file changes after this step.
        if step_config
            .if_condition
            .as_ref()
            .is_some_and(|c| c.file_changed.is_some())
        {
            tracker.take_snapshot(&current_step)?;
        }

        // (C) Group snapshot at start of group.
        if let Some(group_name) = step_group_name {
            let is_first = group_first.get(group_name) == Some(&current_step);
            if is_first
                && let Some(group_cfg) = config.groups.get(group_name)
                && group_cfg
                    .if_condition
                    .as_ref()
                    .is_some_and(|c| c.file_changed.is_some())
            {
                tracker.take_snapshot(&group_snapshot_key(group_name))?;
            }
        }

        let option_next = match &kind {
            StepKind::Prompt(step) => {
                let output =
                    run_prompt_step(vars, config, step, rate_limit_retries, &merged_env).await?;
                let elapsed = step_start.elapsed();
                if !output.is_empty() {
                    eprint!("{}", output);
                }
                log_step_result(elapsed, true);
                None
            }
            StepKind::Command(step) => {
                let success = run_command_step(vars, step, rate_limit_retries, &merged_env).await?;
                let elapsed = step_start.elapsed();
                if !success {
                    steps_failed += 1;
                }
                log_step_result(elapsed, success);
                None
            }
            StepKind::Option(step) => {
                let result = run_option_step(vars, step)?;
                let elapsed = step_start.elapsed();
                log_step_result(elapsed, true);
                result
            }
        };
        steps_run += 1;

        // Post-execution: if file-changed condition → jump to target step.
        let if_next = if let Some(ref if_cond) = step_config.if_condition {
            if let Some(ref target) = if_cond.file_changed {
                if tracker.has_files_changed(&current_step)? {
                    eprintln!(
                        "  {} files changed, jumping to: {}",
                        style("↻").cyan(),
                        target
                    );
                    Some(target.clone())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // (D) Group file-change check at end of group.
        let group_if_next = if let Some(group_name) = step_group_name {
            let is_last = group_last.get(group_name) == Some(&current_step);
            if is_last
                && let Some(group_cfg) = config.groups.get(group_name)
                && let Some(ref if_cond) = group_cfg.if_condition
                && let Some(ref target) = if_cond.file_changed
            {
                if tracker.has_files_changed(&group_snapshot_key(group_name))? {
                    *group_retry_counts
                        .entry(group_name.to_string())
                        .or_insert(0) += 1;
                    eprintln!(
                        "  {} files changed in group '{}', jumping to: {}",
                        style("↻").cyan(),
                        group_name,
                        target
                    );
                    Some(target.clone())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let effective_next = if_next.or(group_if_next).or(option_next).or(step_next);
        let next_step = get_next_step(config, &current_step, effective_next.as_deref());

        // Loop protection.
        if let Some(ref next) = next_step {
            let edge = (current_step.clone(), next.clone());
            let count = edge_counts.entry(edge).or_insert(0);
            *count += 1;
            if *count > max_retries {
                return Err(CruiseError::LoopProtection(
                    current_step,
                    next.clone(),
                    max_retries,
                ));
            }
        }

        match next_step {
            Some(next) => current_step = next,
            None => break,
        }
    }

    let total_elapsed = workflow_start.elapsed();
    eprintln!(
        "\n{} ({} run, {} skipped, {} failed) [{}]",
        style("✓ workflow complete").green().bold(),
        steps_run,
        steps_skipped,
        steps_failed,
        format_duration(total_elapsed)
    );

    Ok(ExecutionResult {
        steps_run,
        steps_skipped,
        steps_failed,
    })
}

/// Merge top-level and step-level env maps, resolving template variables in values.
/// Step-level values override top-level values.
pub(crate) fn resolve_env(
    top: &HashMap<String, String>,
    step: &HashMap<String, String>,
    vars: &VariableStore,
) -> Result<HashMap<String, String>> {
    let mut merged = HashMap::new();
    for (k, v) in top {
        merged.insert(k.clone(), vars.resolve(v)?);
    }
    for (k, v) in step {
        merged.insert(k.clone(), vars.resolve(v)?);
    }
    Ok(merged)
}

/// Print the step completion line (✓ success or ✗ failure) with elapsed time.
pub(crate) fn log_step_result(elapsed: std::time::Duration, success: bool) {
    if success {
        eprintln!(
            "  {}",
            style(format!("✓ {}", format_duration(elapsed))).green()
        );
    } else {
        eprintln!(
            "  {}",
            style(format!("✗ {}", format_duration(elapsed))).red()
        );
    }
}

/// Build the FileTracker snapshot key for a group.
fn group_snapshot_key(group_name: &str) -> String {
    format!("__group__{}", group_name)
}

/// Format a duration as a human-readable string.
pub(crate) fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs >= 60.0 {
        let mins = (secs / 60.0) as u64;
        let remaining = secs % 60.0;
        format!("{}m {:.1}s", mins, remaining)
    } else {
        format!("{:.1}s", secs)
    }
}

/// Resolve the `{model}` placeholder in a command, or strip `--model {model}` if no model.
pub(crate) fn resolve_command_with_model(
    command: &[String],
    effective_model: Option<&str>,
) -> Vec<String> {
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
                if command
                    .get(i + 1)
                    .is_some_and(|next| next.contains("{model}"))
                {
                    i += 2;
                } else {
                    result.push(arg.clone());
                    i += 1;
                }
            } else if arg.starts_with("--model=") || arg.contains("{model}") {
                i += 1;
            } else {
                result.push(arg.clone());
                i += 1;
            }
        }
        result
    }
}

/// Execute a prompt step, updating variable state and returning the LLM output.
pub(crate) async fn run_prompt_step(
    vars: &mut VariableStore,
    config: &WorkflowConfig,
    step: &PromptStep,
    rate_limit_retries: usize,
    env: &HashMap<String, String>,
) -> Result<String> {
    if let Some(inst) = &step.instruction {
        let resolved = vars.resolve(inst)?;
        if vars.input_is_empty() {
            let prompt_text = format!("  {}", &resolved);
            let text = match crate::multiline_input::prompt_multiline(&prompt_text)? {
                crate::multiline_input::InputResult::Submitted(t) => t,
                crate::multiline_input::InputResult::Cancelled => {
                    return Err(CruiseError::Other("input cancelled".to_string()));
                }
            };
            vars.set_input(text);
        } else {
            eprintln!("  {}", style(resolved).dim());
        }
    }
    let prompt = vars.resolve(&step.prompt)?;

    let effective_model = step.model.as_deref().or(config.model.as_deref());

    let has_placeholder = config.command.iter().any(|s| s.contains("{model}"));

    let (resolved_command, model_arg) = if has_placeholder {
        (
            resolve_command_with_model(&config.command, effective_model),
            None,
        )
    } else {
        (config.command.clone(), effective_model.map(str::to_string))
    };

    let spinner = crate::spinner::Spinner::start("Cruising...");
    let result = {
        let on_retry = |msg: &str| spinner.suspend(|| eprintln!("{}", msg));
        run_prompt(
            &resolved_command,
            model_arg.as_deref(),
            &prompt,
            rate_limit_retries,
            env,
            Some(&on_retry),
        )
        .await
    };
    drop(spinner);
    let result = result?;

    let output = result.output;
    vars.set_prev_output(Some(output.clone()));
    vars.set_prev_stderr(Some(result.stderr));
    vars.set_prev_input(None);

    Ok(output)
}

/// Execute a command step, updating variable state and returning whether it succeeded.
pub(crate) async fn run_command_step(
    vars: &mut VariableStore,
    step: &CommandStep,
    rate_limit_retries: usize,
    env: &HashMap<String, String>,
) -> Result<bool> {
    let cmds: Vec<String> = step
        .command
        .iter()
        .map(|c| vars.resolve(c))
        .collect::<Result<Vec<_>>>()?;

    for cmd in &cmds {
        eprintln!("  {} {}", style("$").dim(), style(cmd).dim());
    }

    let result = run_commands(&cmds, rate_limit_retries, env).await?;

    let success = result.success;
    vars.set_prev_success(Some(success));
    vars.set_prev_stderr(Some(result.stderr));
    vars.set_prev_output(None);
    vars.set_prev_input(None);

    Ok(success)
}

/// Execute an option step, updating variable state and returning the chosen next step.
pub(crate) fn run_option_step(
    vars: &mut VariableStore,
    step: &OptionStep,
) -> Result<Option<String>> {
    let desc = step
        .plan
        .as_ref()
        .map(|tmpl| -> Result<String> {
            let path = vars.resolve(tmpl)?;
            std::fs::read_to_string(&path)
                .map_err(|e| CruiseError::Other(format!("failed to read plan file {path}: {e}")))
        })
        .transpose()?;

    let result = run_option(&step.choices, desc.as_deref())?;

    if let Some(ref text) = result.text_input {
        vars.set_prev_input(Some(text.clone()));
    }
    vars.set_prev_output(None);

    Ok(result.next_step)
}

/// Determine the next step: explicit next > IndexMap order > None (end).
pub(crate) fn get_next_step(
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

fn print_env_vars(env: &HashMap<String, String>, indent: &str) {
    let mut keys: Vec<&String> = env.keys().collect();
    keys.sort();
    for k in keys {
        println!("{}{}={}", indent, k, env[k]);
    }
}

/// Print a dry-run summary of the workflow flow.
pub(crate) fn print_dry_run(config: &WorkflowConfig, from: Option<&str>) -> Result<()> {
    println!("{}", style("=== Dry Run: Workflow Flow ===").bold());
    println!("command: {}", config.command.join(" "));

    if let Some(model) = &config.model {
        println!("model: {}", model);
    }

    if !config.env.is_empty() {
        println!("env:");
        print_env_vars(&config.env, "  ");
    }

    if !config.groups.is_empty() {
        println!("\ngroups:");
        let mut group_names: Vec<&str> = config.groups.keys().map(|s| s.as_str()).collect();
        group_names.sort();
        for name in group_names {
            let g = &config.groups[name];
            print!("  {}", style(name).bold());
            if let Some(max) = g.max_retries {
                print!(" (max_retries: {})", max);
            }
            if let Some(ref if_cond) = g.if_condition
                && let Some(ref target) = if_cond.file_changed
            {
                print!(" → retry from: {}", style(target).green());
            }
            println!();
        }
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

        if let Some(ref group_name) = step.group {
            print!(" {}", style(format!("(group: {})", group_name)).magenta());
        }

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

        if !step.env.is_empty() {
            print_env_vars(&step.env, "    env: ");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;
    use crate::file_tracker::FileTracker;
    use crate::variable::VariableStore;

    fn make_config(yaml: &str) -> WorkflowConfig {
        WorkflowConfig::from_yaml(yaml).unwrap()
    }

    async fn run_config(
        yaml: &str,
        input: &str,
        start_step: Option<&str>,
    ) -> Result<ExecutionResult> {
        run_config_with_retries(yaml, input, start_step, 10, 0).await
    }

    async fn run_config_with_retries(
        yaml: &str,
        input: &str,
        start_step: Option<&str>,
        max_retries: usize,
        rate_limit_retries: usize,
    ) -> Result<ExecutionResult> {
        let config = make_config(yaml);
        let mut vars = VariableStore::new(input.to_string());
        let mut tracker = FileTracker::with_root(std::env::current_dir().unwrap());
        let first_step = config.steps.keys().next().unwrap().clone();
        let step = start_step.unwrap_or(&first_step).to_string();
        execute_steps(
            &config,
            &mut vars,
            &mut tracker,
            &step,
            max_retries,
            rate_limit_retries,
            &|_| Ok(()),
        )
        .await
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
        let config = make_config(yaml);
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
        let config = make_config(yaml);
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
        let config = make_config(yaml);
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
        let result = run_config(yaml, "test", None).await;
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
        let result = run_config(yaml, "test", None).await;
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
        let result = run_config(yaml, "", None).await;
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
        let result = run_config(yaml, "", None).await;
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
        let result = run_config(yaml, "", Some("step2")).await;
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
        let result = run_config_with_retries(yaml, "", None, 2, 0).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_dry_run_prints_steps() {
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
        let config = make_config(yaml);
        let result = print_dry_run(&config, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_dry_run_with_from() {
        let yaml = r#"
command: [claude, -p]
steps:
  step1:
    command: echo skip
  step2:
    command: echo show
"#;
        let config = make_config(yaml);
        let result = print_dry_run(&config, Some("step2"));
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_top_level_env_passed_to_command() {
        let yaml = r#"
command: [echo]
env:
  CRUISE_TOP_ENV: top_value
steps:
  step1:
    command: 'test "$CRUISE_TOP_ENV" = top_value'
"#;
        let result = run_config(yaml, "", None).await;
        assert!(result.is_ok(), "top-level env was not passed: {:?}", result);
    }

    #[tokio::test]
    async fn test_step_env_overrides_top_level() {
        let yaml = r#"
command: [echo]
env:
  CRUISE_OVERRIDE_ENV: top_value
steps:
  step1:
    command: 'test "$CRUISE_OVERRIDE_ENV" = step_value'
    env:
      CRUISE_OVERRIDE_ENV: step_value
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "step env override did not work: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_env_variable_resolution() {
        let yaml = r#"
command: [echo]
env:
  CRUISE_INPUT_ENV: "{input}"
steps:
  step1:
    command: 'test "$CRUISE_INPUT_ENV" = myinput'
"#;
        let result = run_config(yaml, "myinput", None).await;
        assert!(
            result.is_ok(),
            "env variable resolution failed: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_variable_resolution_in_command() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo {input}"
"#;
        let result = run_config(yaml, "hello", None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_prev_success_true_propagation() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: echo hello
  step2:
    command: 'test "{prev.success}" = true'
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "prev.success should be true after success: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_prev_success_false_after_failure() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: exit 1
  step2:
    command: 'test "{prev.success}" = false'
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "prev.success should be false after failure: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_command_failure_does_not_stop_workflow() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: exit 1
  step2:
    command: echo done
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "workflow should continue after command failure: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_prev_stderr_propagation() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "printf 'hello_err' >&2; true"
  step2:
    command: test -n "$PREV_STDERR"
    env:
      PREV_STDERR: "{prev.stderr}"
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "prev.stderr should be propagated to env: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_next_field_skips_steps() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: echo hello
    next: step3
  step2:
    command: exit 1
  step3:
    command: echo done
"#;
        let result = run_config(yaml, "", None).await;
        assert!(result.is_ok(), "next field should skip step2: {:?}", result);
    }

    #[tokio::test]
    async fn test_env_prev_success_variable() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: echo hello
  step2:
    command: test "$RESULT" = true
    env:
      RESULT: "{prev.success}"
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "prev.success template in env should work: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_prompt_output_as_prev_output() {
        let yaml = r#"
command: [cat]
steps:
  step1:
    prompt: "hello_output"
  step2:
    command: 'test "{prev.output}" = hello_output'
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "prompt output should be accessible as prev.output: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_prev_output_accessible_in_subsequent_steps() {
        let yaml = r#"
command: [cat]
steps:
  step1:
    prompt: "stored_value"
  step2:
    command: 'test "{prev.output}" = stored_value'
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "prev.output should be accessible in subsequent steps: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_command_list_partial_failure() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command:
      - echo success
      - exit 1
  step2:
    command: 'test "{prev.success}" = false'
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "partial command list failure should set prev.success=false: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_skip_true_with_if_condition() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: exit 1
    skip: true
    if:
      file-changed: step1
  step2:
    command: echo done
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "skip:true should take priority over if condition: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_skipped_step_preserves_prev_vars() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: echo hello
  step2:
    command: exit 1
    skip: true
  step3:
    command: 'test "{prev.success}" = true'
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "skipped step should not update prev vars: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_prompt_to_command_chain() {
        let yaml = r#"
command: [cat]
steps:
  prompt_step:
    prompt: "chain_data"
  command_step:
    command: test "$OUTPUT" = chain_data
    env:
      OUTPUT: "{prev.output}"
"#;
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "prompt output should be usable in command env via prev.output: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_on_step_start_callback_called() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo hello"
  step2:
    command: "echo world"
"#;
        let config = make_config(yaml);
        let mut vars = VariableStore::new(String::new());
        let mut tracker = FileTracker::with_root(std::env::current_dir().unwrap());
        let mut called_steps: Vec<String> = Vec::new();
        let called_ref = std::cell::RefCell::new(&mut called_steps);

        let result = execute_steps(&config, &mut vars, &mut tracker, "step1", 10, 0, &|step| {
            called_ref.borrow_mut().push(step.to_string());
            Ok(())
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(called_steps, vec!["step1", "step2"]);
    }

    #[tokio::test]
    async fn test_run_prompt_step_stdout_still_captured_when_stderr_present() {
        // Given: an LLM command that writes specific content to stdout and noise to stderr
        // When: the prompt step runs
        // Then: stdout is captured to prev.output and stderr is captured to prev.stderr
        // NOTE: Both variables are checked in the same step2 via env vars, before any subsequent
        // command step could overwrite prev.stderr.
        let yaml = r#"
command: [sh, -c, "cat; printf noise >&2"]
steps:
  step1:
    prompt: "chain_value"
  step2:
    command: 'sh -c "test \"$PREV_OUT\" = chain_value && test \"$PREV_ERR\" = noise"'
    env:
      PREV_OUT: "{prev.output}"
      PREV_ERR: "{prev.stderr}"
"#;
        let result = run_config(yaml, "", None).await;
        let result = result.expect("workflow run failed");
        assert_eq!(
            result.steps_failed, 0,
            "stdout and stderr should both be captured correctly"
        );
    }

    // ── format_duration ───────────────────────────────────────────────────────

    #[test]
    fn test_format_duration_zero() {
        assert_eq!(format_duration(std::time::Duration::from_secs(0)), "0.0s");
    }

    #[test]
    fn test_format_duration_sub_minute() {
        assert_eq!(format_duration(std::time::Duration::from_secs(45)), "45.0s");
    }

    #[test]
    fn test_format_duration_exactly_one_minute() {
        assert_eq!(
            format_duration(std::time::Duration::from_secs(60)),
            "1m 0.0s"
        );
    }

    #[test]
    fn test_format_duration_over_one_minute() {
        assert_eq!(
            format_duration(std::time::Duration::from_secs(90)),
            "1m 30.0s"
        );
    }

    #[test]
    fn test_format_duration_multiple_minutes() {
        assert_eq!(
            format_duration(std::time::Duration::from_secs(125)),
            "2m 5.0s"
        );
    }

    #[test]
    fn test_format_duration_fractional_seconds() {
        assert_eq!(
            format_duration(std::time::Duration::from_millis(5500)),
            "5.5s"
        );
    }
}
