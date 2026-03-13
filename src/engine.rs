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
use crate::workflow::CompiledWorkflow;

/// Result of a completed `execute_steps` run.
#[derive(Debug)]
pub struct ExecutionResult {
    pub run: usize,
    pub skipped: usize,
    pub failed: usize,
}

/// Mutable counters threaded through the execution loop.
struct LoopCounters {
    run: usize,
    skipped: usize,
    failed: usize,
    step_index: usize,
    total_steps: usize,
}

/// Outcome of processing one step iteration.
enum StepOutcome {
    /// Advance to the named step.
    Next(String),
    /// The workflow is complete.
    Done,
}

/// Build a map from each step name to its invocation call-site.

/// Check whether the group containing `current_step` has exhausted its retry budget.
///
/// Returns `Some(StepOutcome)` when the group should be skipped entirely.
fn check_group_retry_skip(
    compiled: &CompiledWorkflow,
    current_step: &str,
    step_call_site: Option<&str>,
    group_retry_counts: &HashMap<String, usize>,
    counters: &mut LoopCounters,
) -> Option<StepOutcome> {
    let call_site = step_call_site?;
    let meta = compiled.invocations.get(call_site)?;
    let is_first = meta.first_step == current_step;
    if !is_first {
        return None;
    }
    let max = meta.max_retries?;
    if group_retry_counts.get(call_site).copied().unwrap_or(0) < max {
        return None;
    }
    eprintln!(
        "  {} group '{}' max retries ({}) reached, skipping",
        style("→").yellow(),
        call_site,
        max
    );
    let count = meta.step_count;
    counters.step_index += count;
    counters.skipped += count;
    let next = get_next_step(&compiled.steps, &meta.last_step, None);
    Some(next.map_or(StepOutcome::Done, StepOutcome::Next))
}

/// Take pre-execution snapshots (per-step and per-group) as needed.
fn take_pre_snapshots(
    compiled: &CompiledWorkflow,
    tracker: &mut FileTracker,
    current_step: &str,
    step_call_site: Option<&str>,
    has_if_file_changed: bool,
    fail_if_no_file_changes: bool,
) -> Result<Option<String>> {
    if has_if_file_changed {
        tracker.take_snapshot(current_step)?;
    }
    let nochange_key = if fail_if_no_file_changes {
        let key = nochange_snapshot_key(current_step);
        if !tracker.has_snapshot(&key) {
            tracker.take_snapshot(&key)?;
        }
        Some(key)
    } else {
        None
    };
    if let Some(call_site) = step_call_site {
        let meta = compiled.invocations.get(call_site);
        let is_first = meta.is_some_and(|m| m.first_step == current_step);
        if is_first
            && let Some(invoc) = meta
            && invoc
                .if_condition
                .as_ref()
                .is_some_and(|c| c.file_changed.is_some())
        {
            tracker.take_snapshot(&group_snapshot_key(call_site))?;
        }
    }
    Ok(nochange_key)
}

/// Determine the next-step override from file-change conditions after step execution.
fn resolve_if_next(
    compiled: &CompiledWorkflow,
    tracker: &mut FileTracker,
    current_step: &str,
    step_call_site: Option<&str>,
    step_if_file_changed: Option<&str>,
    group_retry_counts: &mut HashMap<String, usize>,
) -> Result<Option<String>> {
    // Per-step file-changed check.
    if let Some(target) = step_if_file_changed
        && tracker.has_files_changed(current_step)?
    {
        eprintln!(
            "  {} files changed, jumping to: {}",
            style("↻").cyan(),
            target
        );
        return Ok(Some(target.to_string()));
    }
    // Group file-changed check.
    let Some(call_site) = step_call_site else {
        return Ok(None);
    };
    let meta = compiled.invocations.get(call_site);
    let is_last = meta.is_some_and(|m| m.last_step == current_step);
    if !is_last {
        return Ok(None);
    }
    let Some(invoc) = meta else { return Ok(None) };
    let Some(ref if_cond) = invoc.if_condition else {
        return Ok(None);
    };
    let Some(ref target) = if_cond.file_changed else {
        return Ok(None);
    };
    if tracker.has_files_changed(&group_snapshot_key(call_site))? {
        *group_retry_counts.entry(call_site.to_string()).or_insert(0) += 1;
        eprintln!(
            "  {} files changed in group '{}', jumping to: {}",
            style("↻").cyan(),
            call_site,
            target
        );
        Ok(Some(target.clone()))
    } else {
        Ok(None)
    }
}

/// Execute workflow steps starting from `start_step`.
///
/// `on_step_start` is called with the step name before each step executes,
/// allowing the caller to persist the current step for resume support.
pub async fn execute_steps(
    compiled: &CompiledWorkflow,
    vars: &mut VariableStore,
    tracker: &mut FileTracker,
    start_step: &str,
    max_retries: usize,
    rate_limit_retries: usize,
    on_step_start: &dyn Fn(&str) -> Result<()>,
) -> Result<ExecutionResult> {
    let mut current_step = start_step.to_string();
    let workflow_start = Instant::now();
    let step_to_invocation = &compiled.step_to_invocation;
    let mut state = LoopState {
        step_to_invocation,
        group_retry_counts: HashMap::new(),
        counters: LoopCounters {
            run: 0,
            skipped: 0,
            failed: 0,
            step_index: 0,
            total_steps: compiled.steps.len(),
        },
        max_retries,
        rate_limit_retries,
        edge_counts: HashMap::new(),
    };

    loop {
        let outcome = step_loop_iteration(
            compiled,
            vars,
            tracker,
            &current_step,
            &mut state,
            on_step_start,
        )
        .await?;
        match outcome {
            StepOutcome::Next(next) => current_step = next,
            StepOutcome::Done => break,
        }
    }

    let total_elapsed = workflow_start.elapsed();
    let c = &state.counters;
    eprintln!(
        "\n{} ({} run, {} skipped, {} failed) [{}]",
        style("✓ workflow complete").green().bold(),
        c.run,
        c.skipped,
        c.failed,
        format_duration(total_elapsed)
    );
    Ok(ExecutionResult {
        run: c.run,
        skipped: c.skipped,
        failed: c.failed,
    })
}

/// Shared mutable state for the execution loop.
struct LoopState<'a> {
    step_to_invocation: &'a HashMap<String, String>,
    group_retry_counts: HashMap<String, usize>,
    counters: LoopCounters,
    max_retries: usize,
    rate_limit_retries: usize,
    edge_counts: HashMap<(String, String), usize>,
}

/// Execute one iteration of the step loop, returning the next step or Done.
async fn step_loop_iteration(
    compiled: &CompiledWorkflow,
    vars: &mut VariableStore,
    tracker: &mut FileTracker,
    current_step: &str,
    state: &mut LoopState<'_>,
    on_step_start: &dyn Fn(&str) -> Result<()>,
) -> Result<StepOutcome> {
    let step_config = compiled
        .steps
        .get(current_step)
        .ok_or_else(|| CruiseError::StepNotFound(current_step.to_string()))?;
    let step_call_site = state
        .step_to_invocation
        .get(current_step)
        .map(std::string::String::as_str);

    if let Some(outcome) = check_group_retry_skip(
        compiled,
        current_step,
        step_call_site,
        &state.group_retry_counts,
        &mut state.counters,
    ) {
        return Ok(outcome);
    }

    if should_skip(step_config.skip.as_ref(), vars)? {
        state.counters.step_index += 1;
        state.counters.skipped += 1;
        eprintln!("{} skipping: {}", style("→").yellow(), current_step);
        return Ok(get_next_step(&compiled.steps, current_step, None)
            .map_or(StepOutcome::Done, StepOutcome::Next));
    }

    state.counters.step_index += 1;
    state.counters.total_steps = state.counters.total_steps.max(state.counters.step_index);
    eprintln!(
        "\n{} {}",
        style("▶").cyan().bold(),
        style(format!(
            "[{}/{}] {}",
            state.counters.step_index, state.counters.total_steps, current_step
        ))
        .bold()
    );
    on_step_start(current_step)?;

    let step_start = Instant::now();
    let step_next = step_config.next.clone();
    let merged_env = resolve_env(&compiled.env, &step_config.env, vars)?;
    let kind = StepKind::try_from(step_config.clone())?;
    let step_if_file_changed = step_config
        .if_condition
        .as_ref()
        .and_then(|c| c.file_changed.as_deref());

    let nochange_key = take_pre_snapshots(
        compiled,
        tracker,
        current_step,
        step_call_site,
        step_if_file_changed.is_some(),
        step_config.fail_if_no_file_changes,
    )?;
    let option_next = execute_step_kind(
        &kind,
        vars,
        compiled,
        state.rate_limit_retries,
        &merged_env,
        step_start,
        &mut state.counters.failed,
    )
    .await?;
    state.counters.run += 1;

    if let Some(ref key) = nochange_key
        && !tracker.has_files_changed(key)?
    {
        return Err(CruiseError::StepMadeNoFileChanges(current_step.to_string()));
    }

    let if_next = resolve_if_next(
        compiled,
        tracker,
        current_step,
        step_call_site,
        step_if_file_changed,
        &mut state.group_retry_counts,
    )?;
    let effective_next = if_next.or(option_next).or(step_next);
    let next_step = get_next_step(&compiled.steps, current_step, effective_next.as_deref());

    if let Some(ref next) = next_step {
        let edge = (current_step.to_string(), next.clone());
        let count = state.edge_counts.entry(edge).or_insert(0);
        *count += 1;
        if *count > state.max_retries {
            return Err(CruiseError::LoopProtection(
                current_step.to_string(),
                next.clone(),
                state.max_retries,
            ));
        }
    }

    Ok(next_step.map_or(StepOutcome::Done, StepOutcome::Next))
}

/// Execute a single step kind and return the option-selected next step (if any).
async fn execute_step_kind(
    kind: &StepKind,
    vars: &mut VariableStore,
    compiled: &CompiledWorkflow,
    rate_limit_retries: usize,
    merged_env: &HashMap<String, String>,
    step_start: Instant,
    failed: &mut usize,
) -> Result<Option<String>> {
    match kind {
        StepKind::Prompt(step) => {
            let output =
                run_prompt_step(vars, compiled, step, rate_limit_retries, merged_env).await?;
            let elapsed = step_start.elapsed();
            if !output.is_empty() {
                eprint!("{output}");
            }
            log_step_result(elapsed, true);
            Ok(None)
        }
        StepKind::Command(step) => {
            let success = run_command_step(vars, step, rate_limit_retries, merged_env).await?;
            let elapsed = step_start.elapsed();
            if !success {
                *failed += 1;
            }
            log_step_result(elapsed, success);
            Ok(None)
        }
        StepKind::Option(step) => {
            let result = run_option_step(vars, step)?;
            let elapsed = step_start.elapsed();
            log_step_result(elapsed, true);
            Ok(result)
        }
    }
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

/// Build the `FileTracker` snapshot key for a group.
fn group_snapshot_key(group_name: &str) -> String {
    format!("__group__{group_name}")
}

/// Build the `FileTracker` snapshot key for a fail-if-no-file-changes check.
fn nochange_snapshot_key(step_name: &str) -> String {
    format!("__nochange__{step_name}")
}

/// Format a duration as a human-readable string.
pub(crate) fn format_duration(d: std::time::Duration) -> String {
    let total_secs = d.as_secs();
    if total_secs >= 60 {
        let mins = total_secs / 60;
        let remaining = d.as_secs_f64() % 60.0;
        format!("{mins}m {remaining:.1}s")
    } else {
        let secs = d.as_secs_f64();
        format!("{secs:.1}s")
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
    compiled: &CompiledWorkflow,
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

    let effective_model = step.model.as_deref().or(compiled.model.as_deref());

    let has_placeholder = compiled.command.iter().any(|s| s.contains("{model}"));

    let (resolved_command, model_arg) = if has_placeholder {
        (
            resolve_command_with_model(&compiled.command, effective_model),
            None,
        )
    } else {
        (
            compiled.command.clone(),
            effective_model.map(str::to_string),
        )
    };

    let spinner = crate::spinner::Spinner::start("Cruising...");
    let result = {
        let on_retry = |msg: &str| spinner.suspend(|| eprintln!("{msg}"));
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

/// Determine the next step: explicit next > `IndexMap` order > None (end).
pub(crate) fn get_next_step(
    steps: &indexmap::IndexMap<String, crate::config::StepConfig>,
    current: &str,
    explicit_next: Option<&str>,
) -> Option<String> {
    if let Some(next) = explicit_next {
        return Some(next.to_string());
    }

    let mut found = false;
    for key in steps.keys() {
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
pub(crate) fn print_dry_run(config: &WorkflowConfig, from: Option<&str>) {
    println!("{}", style("=== Dry Run: Workflow Flow ===").bold());
    println!("command: {}", config.command.join(" "));

    if let Some(model) = &config.model {
        println!("model: {model}");
    }

    if !config.env.is_empty() {
        println!("env:");
        print_env_vars(&config.env, "  ");
    }

    if !config.groups.is_empty() {
        println!("\ngroups:");
        let mut group_names: Vec<&str> = config
            .groups
            .keys()
            .map(std::string::String::as_str)
            .collect();
        group_names.sort_unstable();
        for name in group_names {
            let g = &config.groups[name];
            print!("  {}", style(name).bold());
            if let Some(max) = g.max_retries {
                print!(" (max_retries: {max})");
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
            print!(" {}", style(format!("(group: {group_name})")).magenta());
        }

        match &step.skip {
            Some(SkipCondition::Static(true)) => print!(" {}", style("(skip)").yellow()),
            Some(SkipCondition::Variable(v)) => {
                print!(" {}", style(format!("(skip if {v})")).yellow());
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowConfig;
    use crate::file_tracker::FileTracker;
    use crate::variable::VariableStore;
    use tempfile::TempDir;

    fn make_config(yaml: &str) -> WorkflowConfig {
        WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"))
    }

    async fn run_config(
        yaml: &str,
        input: &str,
        start_step: Option<&str>,
    ) -> Result<ExecutionResult> {
        run_config_with_retries(yaml, input, start_step, 10, 0).await
    }

    async fn run_config_inner(
        yaml: &str,
        input: &str,
        start_step: Option<&str>,
        tracker_root: std::path::PathBuf,
        max_retries: usize,
        rate_limit_retries: usize,
    ) -> Result<ExecutionResult> {
        let _guard = crate::test_support::lock_process();
        let config = make_config(yaml);
        let compiled = crate::workflow::compile(config).unwrap_or_else(|e| panic!("{e:?}"));
        let mut vars = VariableStore::new(input.to_string());
        let mut tracker = FileTracker::with_root(tracker_root);
        let first_step = compiled
            .steps
            .keys()
            .next()
            .unwrap_or_else(|| panic!("unexpected None"))
            .clone();
        let step = start_step.unwrap_or(&first_step).to_string();
        execute_steps(
            &compiled,
            &mut vars,
            &mut tracker,
            &step,
            max_retries,
            rate_limit_retries,
            &|_| Ok(()),
        )
        .await
    }

    async fn run_config_with_retries(
        yaml: &str,
        input: &str,
        start_step: Option<&str>,
        max_retries: usize,
        rate_limit_retries: usize,
    ) -> Result<ExecutionResult> {
        run_config_inner(
            yaml,
            input,
            start_step,
            std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")),
            max_retries,
            rate_limit_retries,
        )
        .await
    }

    /// Run config with a custom `FileTracker` rooted at `tracker_root`.
    /// Use this for tests that need to control file-change detection.
    /// `max_retries=10` (loop guard), `rate_limit_retries=0` (no live API calls in tests).
    async fn run_config_with_tracker(
        yaml: &str,
        input: &str,
        start_step: Option<&str>,
        tracker_root: std::path::PathBuf,
    ) -> Result<ExecutionResult> {
        run_config_inner(yaml, input, start_step, tracker_root, 10, 0).await
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
        let yaml = r"
command: [echo]
steps:
  step_a:
    command: echo a
  step_b:
    command: echo b
  step_c:
    command: echo c
";
        let config = make_config(yaml);
        assert_eq!(
            get_next_step(&config.steps, "step_a", None),
            Some("step_b".to_string())
        );
        assert_eq!(
            get_next_step(&config.steps, "step_b", None),
            Some("step_c".to_string())
        );
        assert_eq!(get_next_step(&config.steps, "step_c", None), None);
    }

    #[test]
    fn test_get_next_step_explicit() {
        let yaml = r"
command: [echo]
steps:
  step_a:
    command: echo a
  step_b:
    command: echo b
  step_c:
    command: echo c
";
        let config = make_config(yaml);
        assert_eq!(
            get_next_step(&config.steps, "step_a", Some("step_c")),
            Some("step_c".to_string())
        );
    }

    #[test]
    fn test_get_next_step_not_found() {
        let yaml = r"
command: [echo]
steps:
  only_step:
    command: echo hello
";
        let config = make_config(yaml);
        assert_eq!(get_next_step(&config.steps, "only_step", None), None);
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
        assert!(result.is_ok(), "workflow run failed: {result:?}");
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
        assert!(result.is_ok(), "workflow run failed: {result:?}");
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
        print_dry_run(&config, None);
    }

    #[test]
    fn test_dry_run_with_from() {
        let yaml = r"
command: [claude, -p]
steps:
  step1:
    command: echo skip
  step2:
    command: echo show
";
        let config = make_config(yaml);
        print_dry_run(&config, Some("step2"));
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
        assert!(result.is_ok(), "top-level env was not passed: {result:?}");
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
        assert!(result.is_ok(), "step env override did not work: {result:?}");
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
        assert!(result.is_ok(), "env variable resolution failed: {result:?}");
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
            "prev.success should be true after success: {result:?}"
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
            "prev.success should be false after failure: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_command_failure_does_not_stop_workflow() {
        let yaml = r"
command: [echo]
steps:
  step1:
    command: exit 1
  step2:
    command: echo done
";
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "workflow should continue after command failure: {result:?}"
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
            "prev.stderr should be propagated to env: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_next_field_skips_steps() {
        let yaml = r"
command: [echo]
steps:
  step1:
    command: echo hello
    next: step3
  step2:
    command: exit 1
  step3:
    command: echo done
";
        let result = run_config(yaml, "", None).await;
        assert!(result.is_ok(), "next field should skip step2: {result:?}");
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
            "prev.success template in env should work: {result:?}"
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
            "prompt output should be accessible as prev.output: {result:?}"
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
            "prev.output should be accessible in subsequent steps: {result:?}"
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
            "partial command list failure should set prev.success=false: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_skip_true_with_if_condition() {
        let yaml = r"
command: [echo]
steps:
  step1:
    command: exit 1
    skip: true
    if:
      file-changed: step1
  step2:
    command: echo done
";
        let result = run_config(yaml, "", None).await;
        assert!(
            result.is_ok(),
            "skip:true should take priority over if condition: {result:?}"
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
            "skipped step should not update prev vars: {result:?}"
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
            "prompt output should be usable in command env via prev.output: {result:?}"
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
        let compiled = crate::workflow::compile(config).unwrap_or_else(|e| panic!("{e:?}"));
        let mut vars = VariableStore::new(String::new());
        let mut tracker =
            FileTracker::with_root(std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")));
        let mut called_steps: Vec<String> = Vec::new();
        let called_ref = std::cell::RefCell::new(&mut called_steps);

        let result = execute_steps(
            &compiled,
            &mut vars,
            &mut tracker,
            "step1",
            10,
            0,
            &|step| {
                called_ref.borrow_mut().push(step.to_string());
                Ok(())
            },
        )
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
        let result = result.unwrap_or_else(|e| panic!("workflow run failed: {e:?}"));
        assert_eq!(
            result.failed, 0,
            "stdout and stderr should both be captured correctly"
        );
    }

    // --- fail-if-no-file-changes tests ---

    #[tokio::test]
    async fn test_fail_if_no_file_changes_fails_when_no_changes() {
        // Given: a step with fail-if-no-file-changes: true whose command does NOT create any files
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let yaml = r#"
command: [echo]
steps:
  implement:
    command: "echo no file changes"
    fail-if-no-file-changes: true
  next_step:
    command: "echo should not run"
"#;
        // When: executed in a temp dir where no files are written
        let result = run_config_with_tracker(yaml, "", None, dir.path().to_path_buf()).await;
        // Then: workflow fails with StepMadeNoFileChanges
        assert!(result.is_err(), "expected Err but got Ok");
        let err = result.map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        assert!(
            matches!(err, CruiseError::StepMadeNoFileChanges(_)),
            "expected StepMadeNoFileChanges, got: {err:?}"
        );
        assert!(
            err.to_string().contains("implement"),
            "error should mention step name, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_fail_if_no_file_changes_succeeds_when_files_changed() {
        // Given: a step with fail-if-no-file-changes: true whose command DOES create a file
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let output_file = dir.path().join("output.txt");
        let yaml = format!(
            r#"
command: [echo]
steps:
  implement:
    command: "touch {}"
    fail-if-no-file-changes: true
"#,
            output_file.display()
        );
        // When: executed in the temp dir (tracker detects the new file)
        let result = run_config_with_tracker(&yaml, "", None, dir.path().to_path_buf()).await;
        // Then: workflow succeeds
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
    }

    #[tokio::test]
    async fn test_fail_if_no_file_changes_not_set_continues_when_no_changes() {
        // Given: a step WITHOUT fail-if-no-file-changes (default false) that does not change files
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let yaml = r#"
command: [echo]
steps:
  implement:
    command: "echo no changes"
  next_step:
    command: "echo second step"
"#;
        // When: executed
        let result = run_config_with_tracker(yaml, "", None, dir.path().to_path_buf()).await;
        // Then: workflow continues and completes successfully (regression: default behavior unchanged)
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        let result = result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result.run, 2, "both steps should run");
    }

    #[tokio::test]
    async fn test_fail_if_no_file_changes_with_if_file_changed_jumps_on_change() {
        // Given: a step with BOTH fail-if-no-file-changes: true AND if.file-changed,
        // where the command DOES change a file → file-changed jump should win, no failure
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let output_file = dir.path().join("output.txt");
        let yaml = format!(
            r#"
command: [echo]
steps:
  implement:
    command: "touch {}"
    fail-if-no-file-changes: true
    if:
      file-changed: implement
  loop_back:
    command: "echo retry"
  done:
    command: "echo done"
"#,
            output_file.display()
        );
        // When: executed with max_retries=1 to prevent infinite loop
        // (implement writes a file → if.file-changed triggers jump back to implement)
        let result = run_config_inner(&yaml, "", None, dir.path().to_path_buf(), 1, 0).await;
        // Then: workflow does NOT return StepMadeNoFileChanges (files changed, so no-change failure is skipped)
        assert!(
            !matches!(&result, Err(CruiseError::StepMadeNoFileChanges(_))),
            "should not fail with StepMadeNoFileChanges when files changed, got: {result:?}"
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

    #[tokio::test]
    async fn test_next_pointing_to_nonexistent_step() {
        // Given: a step whose `next` points to a step that doesn't exist
        let yaml = r"
command: [echo]
steps:
  step1:
    command: echo hello
    next: nonexistent
";
        // When: the workflow runs
        let result = run_config(yaml, "test", None).await;
        // Then: StepNotFound error is returned
        assert!(result.is_err(), "expected an error but got Ok");
        let err = result.map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        assert!(
            matches!(err, CruiseError::StepNotFound(ref s) if s == "nonexistent"),
            "expected StepNotFound(\"nonexistent\"), got: {err:?}"
        );
    }
}
