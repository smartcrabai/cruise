use std::collections::HashMap;
use std::time::Instant;

use console::style;

use crate::cancellation::CancellationToken;
use crate::condition::should_skip;
use crate::config::{SkipCondition, WorkflowConfig};
use crate::error::{CruiseError, Result};
use crate::file_tracker::FileTracker;
use crate::option_handler::OptionHandler;
use crate::step::command::run_commands;
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

/// Immutable context shared across the execution loop.
///
/// Groups the "configuration" parameters that are threaded unchanged through
/// `execute_steps` -> `step_loop_iteration` -> `execute_step_kind`, keeping each
/// function's signature short enough that Clippy is happy.
pub struct ExecutionContext<'a> {
    pub compiled: &'a CompiledWorkflow,
    pub max_retries: usize,
    pub rate_limit_retries: usize,
    pub on_step_start: &'a dyn Fn(&str) -> Result<()>,
    pub cancel_token: Option<&'a CancellationToken>,
    pub option_handler: &'a dyn OptionHandler,
    pub config_reloader: Option<&'a dyn Fn() -> Result<Option<CompiledWorkflow>>>,
    /// Working directory for child processes spawned by prompt and command steps.
    /// When set, both the LLM subprocess and shell commands run with this as their `cwd`,
    /// ensuring that relative-path file writes land inside the `FileTracker` root.
    pub working_dir: Option<&'a std::path::Path>,
    /// Step names that the user selected to skip before execution.
    /// Applied in addition to YAML-configured `skip` conditions.
    pub skipped_steps: &'a [String],
}

/// Mutable counters threaded through the execution loop.
struct LoopCounters {
    run: usize,
    skipped: usize,
    failed: usize,
}

/// Outcome of processing one step iteration.
enum StepOutcome {
    /// Advance to the named step.
    Next(String),
    /// The workflow is complete.
    Done,
}

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
        style("->").yellow(),
        call_site,
        max
    );
    let count = meta.step_count;
    counters.skipped += count;
    let next = get_next_step(&compiled.steps, &meta.last_step, None);
    Some(next.map_or(StepOutcome::Done, StepOutcome::Next))
}

/// Take pre-execution snapshots (per-step, per-group, and per-attempt NFC) as needed.
fn take_pre_snapshots(
    compiled: &CompiledWorkflow,
    tracker: &mut FileTracker,
    current_step: &str,
    step_call_site: Option<&str>,
    has_if_file_changed: bool,
    fail_if_no_file_changes: bool,
    has_no_file_changes_condition: bool,
) -> Result<(Option<String>, Option<String>)> {
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
    let nfc_key = if has_no_file_changes_condition {
        let key = nfc_snapshot_key(current_step);
        tracker.take_snapshot(&key)?;
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
    Ok((nochange_key, nfc_key))
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
    if let Some(target) = step_if_file_changed {
        if tracker.has_files_changed(current_step)? {
            eprintln!(
                "  {} files changed, jumping to: {}",
                style("R").cyan(),
                target
            );
            return Ok(Some(target.to_string()));
        }
        eprintln!(
            "  {} no file changes (if.file-changed check)",
            style(".").dim()
        );
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
            style("R").cyan(),
            call_site,
            target
        );
        Ok(Some(target.clone()))
    } else {
        eprintln!(
            "  {} no file changes in group '{}'",
            style(".").dim(),
            call_site
        );
        Ok(None)
    }
}

/// Execute workflow steps starting from `start_step`.
///
/// # Errors
///
/// Returns an error if a step fails fatally or an I/O operation fails.
pub async fn execute_steps(
    ctx: &ExecutionContext<'_>,
    vars: &mut VariableStore,
    tracker: &mut FileTracker,
    start_step: &str,
) -> Result<ExecutionResult> {
    let mut current_step = start_step.to_string();
    let workflow_start = Instant::now();
    let mut reloaded: Option<CompiledWorkflow> = None;
    let mut state = LoopState {
        group_retry_counts: HashMap::new(),
        counters: LoopCounters {
            run: 0,
            skipped: 0,
            failed: 0,
        },
        edge_counts: HashMap::new(),
    };

    loop {
        // Reload config between steps if a reloader is provided.
        if let Some(reloader) = ctx.config_reloader
            && let Some(new_compiled) = reloader()?
            && new_compiled.steps.contains_key(current_step.as_str())
        {
            state.group_retry_counts.clear();
            state.edge_counts.clear();
            reloaded = Some(new_compiled);
        }
        let active_compiled = reloaded.as_ref().unwrap_or(ctx.compiled);
        let active_ctx = ExecutionContext {
            compiled: active_compiled,
            max_retries: ctx.max_retries,
            rate_limit_retries: ctx.rate_limit_retries,
            on_step_start: ctx.on_step_start,
            cancel_token: ctx.cancel_token,
            option_handler: ctx.option_handler,
            config_reloader: None,
            working_dir: ctx.working_dir,
            skipped_steps: ctx.skipped_steps,
        };
        let outcome =
            step_loop_iteration(&active_ctx, vars, tracker, &current_step, &mut state).await?;
        match outcome {
            StepOutcome::Next(next) => current_step = next,
            StepOutcome::Done => break,
        }
    }

    let total_elapsed = workflow_start.elapsed();
    let c = &state.counters;
    eprintln!(
        "\n{} ({} run, {} skipped, {} failed) [{}]",
        style("v workflow complete").green().bold(),
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
struct LoopState {
    group_retry_counts: HashMap<String, usize>,
    counters: LoopCounters,
    edge_counts: HashMap<(String, String), usize>,
}

/// Execute one iteration of the step loop, returning the next step or Done.
#[expect(
    clippy::too_many_lines,
    reason = "step loop logic is inherently complex"
)]
async fn step_loop_iteration(
    ctx: &ExecutionContext<'_>,
    vars: &mut VariableStore,
    tracker: &mut FileTracker,
    current_step: &str,
    state: &mut LoopState,
) -> Result<StepOutcome> {
    let step_config = ctx
        .compiled
        .steps
        .get(current_step)
        .ok_or_else(|| CruiseError::StepNotFound(current_step.to_string()))?;
    let step_call_site = ctx
        .compiled
        .step_to_invocation
        .get(current_step)
        .map(std::string::String::as_str);

    if let Some(outcome) = check_group_retry_skip(
        ctx.compiled,
        current_step,
        step_call_site,
        &state.group_retry_counts,
        &mut state.counters,
    ) {
        return Ok(outcome);
    }

    if should_skip(step_config.skip.as_ref(), vars)?
        || ctx.skipped_steps.iter().any(|s| s == current_step)
    {
        state.counters.skipped += 1;
        // sakoku-ignore-next-line
        eprintln!("{} skipping: {}", style("→").yellow(), current_step);
        // When the first substep of a group with file-changed is user-skipped,
        // the normal "before" snapshot is never taken (it happens in the
        // non-skip path below). Take it here so the group-level file-changed
        // comparison still works for the remaining substeps.
        if let Some(call_site) = step_call_site
            && let Some(meta) = ctx.compiled.invocations.get(call_site)
            && meta.first_step == current_step
            && meta
                .if_condition
                .as_ref()
                .is_some_and(|c| c.file_changed.is_some())
        {
            tracker.take_snapshot(&group_snapshot_key(call_site))?;
        }
        return Ok(get_next_step(&ctx.compiled.steps, current_step, None)
            .map_or(StepOutcome::Done, StepOutcome::Next));
    }

    eprintln!(
        "\n{} {}",
        style(">").cyan().bold(),
        style(current_step).bold()
    );
    (ctx.on_step_start)(current_step)?;

    // Check for cancellation after saving state (allows resume from this step later).
    // This must happen after `on_step_start` so the caller can persist which step to resume at.
    if let Some(token) = ctx.cancel_token
        && token.is_cancelled()
    {
        return Ok(StepOutcome::Done);
    }

    let step_start = Instant::now();
    let step_next = step_config.next.clone();
    let merged_env = resolve_env(&ctx.compiled.env, &step_config.env, vars)?;
    let kind = StepKind::try_from(step_config.clone())?;
    let if_cond = step_config.if_condition.as_ref();
    let step_if_file_changed = if_cond.and_then(|c| c.file_changed.as_deref());
    let nfc_cond = if_cond.and_then(|c| c.no_file_changes.as_ref());

    let (nochange_key, nfc_key) = take_pre_snapshots(
        ctx.compiled,
        tracker,
        current_step,
        step_call_site,
        step_if_file_changed.is_some() && nfc_cond.is_none(),
        step_config.fail_if_no_file_changes,
        nfc_cond.is_some(),
    )?;
    let option_next = execute_step_kind(
        ctx,
        &kind,
        vars,
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

    let nfc_retry = if let Some(ref key) = nfc_key
        && let Some(nfc) = nfc_cond
        && !tracker.has_files_changed(key)?
    {
        if nfc.fail {
            return Err(CruiseError::StepMadeNoFileChanges(current_step.to_string()));
        }
        if nfc.retry {
            eprintln!(
                "  {} no file changes, will retry (if.no-file-changes.retry)",
                style("R").cyan()
            );
        }
        nfc.retry
    } else {
        false
    };

    let if_next = resolve_if_next(
        ctx.compiled,
        tracker,
        current_step,
        step_call_site,
        if nfc_cond.is_none() {
            step_if_file_changed
        } else {
            None
        },
        &mut state.group_retry_counts,
    )?;
    let transition_reason = if if_next.is_some() {
        "if.file-changed"
    } else if nfc_retry {
        "if.no-file-changes.retry"
    } else if option_next.is_some() {
        "option"
    } else if step_next.is_some() {
        "next"
    } else {
        "sequential"
    };
    let effective_next = if_next
        .or(nfc_retry.then(|| current_step.to_string()))
        .or(option_next)
        .or(step_next);
    let next_step = get_next_step(&ctx.compiled.steps, current_step, effective_next.as_deref());

    if let Some(ref next) = next_step {
        let edge = (current_step.to_string(), next.clone());
        let count = state.edge_counts.entry(edge).or_insert(0);
        *count += 1;
        eprintln!(
            "  {} {} -> {} [{}] (edge {}/{})",
            style("->").dim(),
            current_step,
            next,
            transition_reason,
            count,
            ctx.max_retries
        );
        if *count > ctx.max_retries {
            let mut all_edges: Vec<(String, String, usize)> = state
                .edge_counts
                .iter()
                .map(|((f, t), &c)| (f.clone(), t.clone(), c))
                .collect();
            all_edges.sort_by(|a, b| b.2.cmp(&a.2));
            return Err(CruiseError::LoopProtection {
                from: current_step.to_string(),
                to: next.clone(),
                max_retries: ctx.max_retries,
                edge_counts: all_edges,
            });
        }
    }

    Ok(next_step.map_or(StepOutcome::Done, StepOutcome::Next))
}

/// Execute a single step kind and return the option-selected next step (if any).
async fn execute_step_kind(
    ctx: &ExecutionContext<'_>,
    kind: &StepKind,
    vars: &mut VariableStore,
    merged_env: &HashMap<String, String>,
    step_start: Instant,
    failed: &mut usize,
) -> Result<Option<String>> {
    match kind {
        StepKind::Prompt(step) => {
            let output = run_prompt_step(
                vars,
                ctx.compiled,
                step,
                ctx.rate_limit_retries,
                merged_env,
                ctx.cancel_token,
                ctx.working_dir,
            )
            .await?;
            let elapsed = step_start.elapsed();
            if !output.is_empty() {
                eprint!("{output}");
            }
            log_step_result(elapsed, true);
            Ok(None)
        }
        StepKind::Command(step) => {
            let success = run_command_step(
                vars,
                step,
                ctx.rate_limit_retries,
                merged_env,
                ctx.working_dir,
            )
            .await?;
            let elapsed = step_start.elapsed();
            if !success {
                *failed += 1;
            }
            log_step_result(elapsed, success);
            Ok(None)
        }
        StepKind::Option(step) => {
            let result = run_option_step(vars, step, ctx.option_handler)?;
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

/// Print the step completion line (v success or x failure) with elapsed time.
pub(crate) fn log_step_result(elapsed: std::time::Duration, success: bool) {
    if success {
        eprintln!(
            "  {}",
            style(format!("v {}", format_duration(elapsed))).green()
        );
    } else {
        eprintln!(
            "  {}",
            style(format!("x {}", format_duration(elapsed))).red()
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

/// Build the `FileTracker` snapshot key for an if.no-file-changes check.
fn nfc_snapshot_key(step_name: &str) -> String {
    format!("__nfc__{step_name}")
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
#[must_use]
pub fn resolve_command_with_model(
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
    cancel_token: Option<&CancellationToken>,
    working_dir: Option<&std::path::Path>,
) -> Result<String> {
    if let Some(inst) = &step.instruction {
        let resolved = vars.resolve(inst)?;
        if vars.input_is_empty() {
            let prompt_text = format!("  {}", &resolved);
            let text = crate::multiline_input::prompt_multiline(&prompt_text)?.into_result()?;
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
            cancel_token,
            working_dir,
        )
        .await
    };
    drop(spinner);
    let result = result?;

    if !result.stderr.is_empty() {
        for line in result.stderr.trim_end().lines() {
            eprintln!("  {} {}", style("stderr:").dim(), line);
        }
    }

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
    working_dir: Option<&std::path::Path>,
) -> Result<bool> {
    let cmds: Vec<String> = step
        .command
        .iter()
        .map(|c| vars.resolve(c))
        .collect::<Result<Vec<_>>>()?;

    for cmd in &cmds {
        eprintln!("  {} {}", style("$").dim(), style(cmd).dim());
    }

    let result = run_commands(&cmds, rate_limit_retries, env, working_dir).await?;

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
    option_handler: &dyn OptionHandler,
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

    let result = option_handler.select_option(&step.choices, desc.as_deref())?;

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
pub fn print_dry_run(config: &WorkflowConfig, from: Option<&str>) {
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
                print!(" -> retry from: {}", style(target).green());
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
            print!(" -> {}", style(next).green());
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
    use crate::cancellation::CancellationToken;
    use crate::config::WorkflowConfig;
    use crate::file_tracker::FileTracker;
    use crate::option_handler::{NoOpOptionHandler, OptionHandler};
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

    // Core test helper: compile `yaml`, build an `ExecutionContext`, and run.
    // `on_step_start` is always a no-op in this helper; tests that need a custom
    // callback build `ExecutionContext` directly (see `test_on_step_start_callback_called`).
    #[expect(clippy::too_many_arguments)]
    async fn run_config_inner(
        yaml: &str,
        input: &str,
        start_step: Option<&str>,
        tracker_root: std::path::PathBuf,
        max_retries: usize,
        rate_limit_retries: usize,
        config_reloader: Option<&dyn Fn() -> Result<Option<crate::workflow::CompiledWorkflow>>>,
        cancel_token: Option<&CancellationToken>,
        option_handler: &dyn OptionHandler,
        skipped_steps: &[String],
    ) -> Result<ExecutionResult> {
        let _guard = crate::test_support::lock_process();
        let config = make_config(yaml);
        let compiled = crate::workflow::compile(config).unwrap_or_else(|e| panic!("{e:?}"));
        let mut vars = VariableStore::new(input.to_string());
        let mut tracker = FileTracker::with_root(tracker_root.clone());
        let first_step = compiled
            .steps
            .keys()
            .next()
            .unwrap_or_else(|| panic!("unexpected None"))
            .clone();
        let step = start_step.unwrap_or(&first_step).to_string();
        let ctx = ExecutionContext {
            compiled: &compiled,
            max_retries,
            rate_limit_retries,
            on_step_start: &|_| Ok(()),
            cancel_token,
            option_handler,
            config_reloader,
            working_dir: Some(tracker_root.as_path()),
            skipped_steps,
        };
        execute_steps(&ctx, &mut vars, &mut tracker, &step).await
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
            None,
            None,
            &NoOpOptionHandler,
            &[],
        )
        .await
    }

    /// Run config with user-selected skipped steps.
    /// Useful for testing the user-skip feature independently from the YAML `skip` condition.
    async fn run_config_with_skipped(
        yaml: &str,
        input: &str,
        start_step: Option<&str>,
        skipped_steps: &[&str],
    ) -> Result<ExecutionResult> {
        let skipped: Vec<String> = skipped_steps
            .iter()
            .map(std::string::ToString::to_string)
            .collect();
        run_config_inner(
            yaml,
            input,
            start_step,
            std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")),
            10,
            0,
            None,
            None,
            &NoOpOptionHandler,
            &skipped,
        )
        .await
    }

    // Run config with a custom `FileTracker` rooted at `tracker_root`.
    // Use this for tests that need to control file-change detection.
    // max_retries=10 (loop guard), rate_limit_retries=0 (no live API calls in tests).
    async fn run_config_with_tracker(
        yaml: &str,
        input: &str,
        start_step: Option<&str>,
        tracker_root: std::path::PathBuf,
    ) -> Result<ExecutionResult> {
        run_config_inner(
            yaml,
            input,
            start_step,
            tracker_root,
            10,
            0,
            None,
            None,
            &NoOpOptionHandler,
            &[],
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
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));
        assert_eq!(result.skipped, 1, "one step should be counted as skipped");
        assert_eq!(result.run, 1, "one step should be counted as run");
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
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));
        assert_eq!(result.skipped, 1, "one step should be counted as skipped");
        assert_eq!(result.run, 2, "two steps should be counted as run");
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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

        let ctx = ExecutionContext {
            compiled: &compiled,
            max_retries: 10,
            rate_limit_retries: 0,
            on_step_start: &|step| {
                called_ref.borrow_mut().push(step.to_string());
                Ok(())
            },
            cancel_token: None,
            option_handler: &NoOpOptionHandler,
            config_reloader: None,
            working_dir: None,
            skipped_steps: &[],
        };
        let result = execute_steps(&ctx, &mut vars, &mut tracker, "step1").await;

        assert!(result.is_ok());
        assert_eq!(called_steps, vec!["step1", "step2"]);
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[tokio::test]
    async fn test_fail_if_no_file_changes_with_if_file_changed_jumps_on_change() {
        // Given: a step with BOTH fail-if-no-file-changes: true AND if.file-changed,
        // where the command DOES change a file -> file-changed jump should win, no failure
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
        // (implement writes a file -> if.file-changed triggers jump back to implement)
        let result = run_config_inner(
            &yaml,
            "",
            None,
            dir.path().to_path_buf(),
            10,
            0,
            None,
            None,
            &NoOpOptionHandler,
            &[],
        )
        .await;
        // Then: workflow does NOT return StepMadeNoFileChanges (files changed, so no-change failure is skipped)
        assert!(
            !matches!(&result, Err(CruiseError::StepMadeNoFileChanges(_))),
            "should not fail with StepMadeNoFileChanges when files changed, got: {result:?}"
        );
    }

    // --- if.no-file-changes tests (new syntax) ---

    #[tokio::test]
    async fn test_if_no_file_changes_fail_fails_when_no_changes() {
        // Given: a step with if.no-file-changes.fail: true whose command does NOT create any files
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let yaml = r#"
command: [echo]
steps:
  implement:
    command: "echo no file changes"
    if:
      no-file-changes:
        fail: true
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

    #[cfg(unix)]
    #[tokio::test]
    async fn test_if_no_file_changes_fail_ok_when_files_changed() {
        // Given: a step with if.no-file-changes.fail: true whose command DOES create a file
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let output_file = dir.path().join("output.txt");
        let yaml = format!(
            r#"
command: [echo]
steps:
  implement:
    command: "touch {}"
    if:
      no-file-changes:
        fail: true
"#,
            output_file.display()
        );
        // When: executed in the temp dir (tracker detects the new file)
        let result = run_config_with_tracker(&yaml, "", None, dir.path().to_path_buf()).await;
        // Then: workflow succeeds
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_if_no_file_changes_retry_reruns_step_when_no_changes() {
        // Given: a step with if.no-file-changes.retry: true and a counter file
        // The step runs N times before creating a file (simulated with a counter).
        // Counter is stored OUTSIDE the tracked dir so it doesn't cause spurious change detection.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let counter_dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}")); // not tracked
        let output_file = dir.path().join("output.txt");
        let counter_file = counter_dir.path().join("counter.txt");
        // Write initial counter value
        std::fs::write(&counter_file, "0").unwrap_or_else(|e| panic!("{e:?}"));
        // The command increments counter and creates output.txt only when counter reaches 2.
        // On attempt 1: counter 0->1, no output.txt -> no tracked file change -> nfc retry fires.
        // On attempt 2: counter 1->2, output.txt created -> tracked file change -> proceed to done.
        let yaml = format!(
            r#"
command: [sh, -c]
steps:
  implement:
    command: >-
      COUNT=$(cat {counter}) &&
      NEW=$((COUNT+1)) &&
      echo $NEW > {counter} &&
      if [ $NEW -ge 2 ]; then touch {output}; fi
    if:
      no-file-changes:
        retry: true
  done:
    command: "echo done"
"#,
            counter = counter_file.display(),
            output = output_file.display()
        );
        // When: executed
        let result = run_config_with_tracker(&yaml, "", None, dir.path().to_path_buf()).await;
        // Then: workflow succeeds after retry (implement ran twice, then done)
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")).run,
            3,
            "implement (*2 attempts) + done = 3 executions"
        );
    }

    #[tokio::test]
    async fn test_if_no_file_changes_retry_triggers_loop_protection() {
        // Given: a step with if.no-file-changes.retry: true that NEVER creates any files
        let yaml = r#"
command: [echo]
steps:
  implement:
    command: "echo no changes ever"
    if:
      no-file-changes:
        retry: true
"#;
        // When: executed with max_retries=3 (loop protection kicks in)
        let result = run_config_with_retries(yaml, "", None, 3, 0).await;
        // Then: workflow fails with LoopProtection
        assert!(result.is_err(), "expected Err but got Ok");
        let err = result.map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"));
        assert!(
            matches!(err, CruiseError::LoopProtection { .. }),
            "expected LoopProtection, got: {err:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_if_no_file_changes_retry_not_triggered_when_files_changed() {
        // Given: a step with if.no-file-changes.retry: true that DOES create a file
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let output_file = dir.path().join("output.txt");
        let yaml = format!(
            r#"
command: [echo]
steps:
  implement:
    command: "touch {}"
    if:
      no-file-changes:
        retry: true
  done:
    command: "echo done"
"#,
            output_file.display()
        );
        // When: executed
        let result = run_config_with_tracker(&yaml, "", None, dir.path().to_path_buf()).await;
        // Then: workflow runs both steps (no retry loop)
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        let result = result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result.run, 2, "both steps should run (no extra retry)");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_if_file_changed_and_no_file_changes_retry_combo() {
        // Given: a step with BOTH if.file-changed (jump) and if.no-file-changes.retry,
        // where the command DOES change a file.
        // When no-file-changes is set, the file-changed snapshot is suppressed (no-file-changes
        // takes precedence for change detection). Files changed -> no-file-changes does NOT trigger,
        // workflow proceeds to the sequential next step.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let output_file = dir.path().join("output.txt");
        let yaml = format!(
            r#"
command: [echo]
steps:
  implement:
    command: "touch {}"
    if:
      file-changed: implement
      no-file-changes:
        retry: true
  loop_back:
    command: "echo jumped here"
  done:
    command: "echo done"
"#,
            output_file.display()
        );
        // When: executed
        let result = run_config_with_tracker(&yaml, "", None, dir.path().to_path_buf()).await;
        // Then: workflow completes without retry (files changed, nfc does not trigger)
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        // implement -> loop_back -> done = 3 steps
        let r = result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(r.run, 3, "implement + loop_back + done should all run");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_if_no_file_changes_snapshot_per_attempt() {
        // Given: a step with if.no-file-changes.retry: true
        // First attempt: no tracked changes -> retry (nfc snapshot taken fresh, fires retry)
        // Second attempt: tracked file created -> proceed
        // This verifies that snapshot is taken fresh each attempt (not reused from first visit).
        // Counter is stored OUTSIDE the tracked dir so it doesn't cause spurious change detection.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let counter_dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}")); // not tracked
        let output_file = dir.path().join("output.txt");
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap_or_else(|e| panic!("{e:?}"));
        // Step creates output.txt only on second call (N >= 1).
        // Attempt 1: N=0 -> no output.txt, counter changes (untracked) -> nfc retry fires.
        // Attempt 2: N=1 -> output.txt created (tracked) -> nfc doesn't fire -> proceed to done.
        let yaml = format!(
            r#"
command: [sh, -c]
steps:
  implement:
    command: >-
      N=$(cat {counter}) &&
      echo $((N+1)) > {counter} &&
      if [ $N -ge 1 ]; then touch {output}; fi
    if:
      no-file-changes:
        retry: true
  done:
    command: "echo done"
"#,
            counter = counter_file.display(),
            output = output_file.display()
        );
        // When: executed with sufficient retries
        let result = run_config_with_tracker(&yaml, "", None, dir.path().to_path_buf()).await;
        // Then: workflow proceeds after retry (snapshot was per-attempt, not global)
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        let r = result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(r.run, 3, "implement (*2 attempts) + done = 3 executions");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_if_file_changed_and_no_file_changes_retry_combo_unchanged() {
        // Given: a step with BOTH if.file-changed (jump) and if.no-file-changes.retry,
        // where the first attempt does NOT change any tracked files -> no-file-changes.retry fires.
        // Counter is stored OUTSIDE the tracked dir so it doesn't cause spurious change detection.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let counter_dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}")); // not tracked
        let output_file = dir.path().join("output.txt");
        let counter_file = counter_dir.path().join("count.txt");
        std::fs::write(&counter_file, "0").unwrap_or_else(|e| panic!("{e:?}"));
        // Attempt 1: N=0 -> counter increments (untracked), no output.txt -> nfc retry fires.
        // Attempt 2: N=1 -> counter increments (untracked), output.txt created (tracked) -> proceed.
        // (file-changed snapshot is suppressed when no-file-changes is set; step exits to done.)
        let yaml = format!(
            r#"
command: [sh, -c]
steps:
  implement:
    command: >-
      N=$(cat {counter}) &&
      echo $((N+1)) > {counter} &&
      if [ $N -ge 1 ]; then touch {output}; fi
    if:
      file-changed: implement
      no-file-changes:
        retry: true
  done:
    command: "echo done"
"#,
            counter = counter_file.display(),
            output = output_file.display()
        );
        // When: executed (nfc retry fires on first attempt, then files change, then done)
        let result = run_config_with_tracker(&yaml, "", None, dir.path().to_path_buf()).await;
        // Then: workflow succeeds
        assert!(
            result.is_ok(),
            "expected Ok after retry on unchanged: {result:?}"
        );
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")).run,
            3,
            "implement (*2 attempts) + done = 3 executions"
        );
    }

    // -- regression: prompt/command step relative-path and nfc ----------------

    #[cfg(unix)]
    #[tokio::test]
    async fn test_if_no_file_changes_retry_not_triggered_for_prompt_step_relative_path() {
        // Given: a prompt step whose LLM subprocess creates a file via a relative path.
        // The subprocess must run with cwd = tracker_root so the file lands inside the
        // tracked directory; without working_dir the file lands in the test-runner cwd
        // and no-file-changes.retry fires.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        // sh -c "touch output.txt && cat":
        //   - creates output.txt in the subprocess cwd (= tracker_root after fix)
        //   - echoes the prompt text via cat so run_prompt receives a non-empty output
        let yaml = r#"
command: [sh, -c, "touch output.txt && cat"]
steps:
  implement:
    prompt: "work"
    if:
      no-file-changes:
        retry: true
  done:
    command: "echo done"
"#;
        // When: executed with tracker_root = dir
        let result = run_config_with_tracker(yaml, "", None, dir.path().to_path_buf()).await;
        // Then: workflow completes without retry (file created in tracker_root -> nfc does not fire)
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        let r = result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(r.run, 2, "implement + done = 2 executions (no nfc retry)");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_if_no_file_changes_retry_not_triggered_for_command_step_relative_path() {
        // Given: a command step that creates a file via a relative path.
        // The subprocess must run with cwd = tracker_root so the file lands inside the
        // tracked directory; without working_dir the file lands in the test-runner cwd
        // and no-file-changes.retry fires.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let yaml = r#"
command: [echo]
steps:
  implement:
    command: "touch output.txt"
    if:
      no-file-changes:
        retry: true
  done:
    command: "echo done"
"#;
        // When: executed with tracker_root = dir
        let result = run_config_with_tracker(yaml, "", None, dir.path().to_path_buf()).await;
        // Then: workflow completes without retry (file created in tracker_root -> nfc does not fire)
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        let r = result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(r.run, 2, "implement + done = 2 executions (no nfc retry)");
    }

    // -- format_duration -------------------------------------------------------

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

    // --- config_reloader tests ---

    #[tokio::test]
    async fn test_execute_steps_config_reloader_not_triggered_when_no_change() {
        // Given: reloader returns None (no change)
        let yaml = "command: [echo]\nsteps:\n  s1:\n    command: echo hi\n";
        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_clone = call_count.clone();
        let reloader = move || -> Result<Option<crate::workflow::CompiledWorkflow>> {
            count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None) // no change
        };
        // When: running with reloader that always returns None
        let result = run_config_inner(
            yaml,
            "",
            None,
            std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")),
            10,
            0,
            Some(&reloader),
            None,
            &NoOpOptionHandler,
            &[],
        )
        .await;
        // Then: completes successfully and reloader has been called
        assert!(result.is_ok());
        assert!(
            call_count.load(std::sync::atomic::Ordering::SeqCst) >= 1,
            "reloader should have been called at least once"
        );
    }

    #[tokio::test]
    async fn test_execute_steps_config_reloader_updates_compiled_when_changed() {
        // Given: initial config with only step1, reloader returns step1+step2
        let original_yaml = "command: [echo]\nsteps:\n  step1:\n    command: echo original\n";
        let updated_yaml = "command: [echo]\nsteps:\n  step1:\n    command: echo updated\n  step2:\n    command: echo extra\n";
        let updated_config = make_config(updated_yaml);
        let updated_compiled =
            crate::workflow::compile(updated_config).unwrap_or_else(|e| panic!("{e:?}"));
        let updated = std::sync::Arc::new(std::sync::Mutex::new(Some(updated_compiled)));
        let reloader = {
            let updated = updated.clone();
            move || -> Result<Option<crate::workflow::CompiledWorkflow>> {
                // return the updated config only on the first call
                Ok(updated.lock().unwrap_or_else(|e| panic!("{e:?}")).take())
            }
        };
        // When: reloader returns the updated config
        let result = run_config_inner(
            original_yaml,
            "",
            None,
            std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")),
            10,
            0,
            Some(&reloader),
            None,
            &NoOpOptionHandler,
            &[],
        )
        .await;
        // Then: completes successfully (steps executed with the updated config)
        assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_execute_steps_config_reloader_keeps_old_config_when_step_missing() {
        // Given: the currently executing step does not exist in the new config
        let original_yaml = "command: [echo]\nsteps:\n  step1:\n    command: echo original\n";
        // the new config does not contain step1
        let new_yaml = "command: [echo]\nsteps:\n  completely_different:\n    command: echo new\n";
        let new_config = make_config(new_yaml);
        let new_compiled = crate::workflow::compile(new_config).unwrap_or_else(|e| panic!("{e:?}"));
        let new = std::sync::Arc::new(std::sync::Mutex::new(Some(new_compiled)));
        let reloader = {
            let new = new.clone();
            move || -> Result<Option<crate::workflow::CompiledWorkflow>> {
                Ok(new.lock().unwrap_or_else(|e| panic!("{e:?}")).take())
            }
        };
        // When: reloader returns a config that does not contain the current step
        let result = run_config_inner(
            original_yaml,
            "",
            None,
            std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")),
            10,
            0,
            Some(&reloader),
            None,
            &NoOpOptionHandler,
            &[],
        )
        .await;
        // Then: retains the old config and completes successfully (step1 is executed)
        assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")).run,
            1,
            "step1 should have run with old config"
        );
    }

    // -- Helpers for cancel/option tests ---------------------------------------

    // Run a workflow with an explicit cancel token and option handler.
    async fn run_with_options(
        yaml: &str,
        start: &str,
        cancel_token: Option<&CancellationToken>,
        handler: &dyn OptionHandler,
    ) -> Result<ExecutionResult> {
        run_config_inner(
            yaml,
            "",
            Some(start),
            std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")),
            10,
            0,
            None,
            cancel_token,
            handler,
            &[],
        )
        .await
    }

    // -- CancellationToken integration tests -----------------------------------

    #[tokio::test]
    async fn test_execute_steps_none_cancel_token_runs_all_steps() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo hello"
  step2:
    command: "echo world"
"#;
        let result = run_with_options(yaml, "step1", None, &NoOpOptionHandler).await;
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        let result = result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(result.run, 2, "both steps should run with no cancel token");
    }

    #[tokio::test]
    async fn test_execute_steps_active_cancel_token_runs_normally() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo hello"
  step2:
    command: "echo world"
"#;
        let token = CancellationToken::new(); // not cancelled
        let result = run_with_options(yaml, "step1", Some(&token), &NoOpOptionHandler).await;
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        let result = result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            result.run, 2,
            "both steps should run with uncancelled token"
        );
    }

    #[tokio::test]
    async fn test_execute_steps_pre_cancelled_token_skips_all_steps() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo should not run"
  step2:
    command: "echo also not run"
"#;
        let token = CancellationToken::new();
        token.cancel();
        let result = run_with_options(yaml, "step1", Some(&token), &NoOpOptionHandler).await;
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        let result = result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            result.run, 0,
            "no steps should run when token is pre-cancelled"
        );
    }

    #[tokio::test]
    async fn test_execute_steps_cancel_between_steps_stops_at_boundary() {
        // Given: a three-step workflow where the token is cancelled during on_step_start of step2
        // (i.e. after step1 runs, before step2 executes)
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo first"
  step2:
    command: "echo second"
  step3:
    command: "echo third"
"#;
        let _guard = crate::test_support::lock_process();
        let config = make_config(yaml);
        let compiled = crate::workflow::compile(config).unwrap_or_else(|e| panic!("{e:?}"));
        let mut vars = VariableStore::new(String::new());
        let mut tracker =
            FileTracker::with_root(std::env::current_dir().unwrap_or_else(|e| panic!("{e:?}")));
        let token = CancellationToken::new();
        let token_clone = token.clone();
        // on_step_start is called before the cancel check: cancel on the 2nd call (step2)
        let call_count = std::cell::Cell::new(0usize);
        let ctx = ExecutionContext {
            compiled: &compiled,
            max_retries: 10,
            rate_limit_retries: 0,
            on_step_start: &|_step| {
                let n = call_count.get();
                if n >= 1 {
                    // step2 (second call): cancel so the token check fires after on_step_start
                    token_clone.cancel();
                }
                call_count.set(n + 1);
                Ok(())
            },
            cancel_token: Some(&token),
            option_handler: &NoOpOptionHandler,
            config_reloader: None,
            working_dir: None,
            skipped_steps: &[],
        };
        // When: execute_steps runs; step2's on_step_start triggers cancellation
        let result = execute_steps(&ctx, &mut vars, &mut tracker, "step1").await;
        // Then: step1 ran (run=1); step2's on_step_start was called but step2 did not execute
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        let result = result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            result.run, 1,
            "only step1 should run; step2 cancelled before execution, step3 never reached"
        );
    }

    // -- OptionHandler integration tests ---------------------------------------

    #[tokio::test]
    async fn test_execute_steps_option_handler_not_called_for_command_steps() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo hello"
  step2:
    command: "echo world"
"#;
        let handler = crate::option_handler::FirstChoiceOptionHandler::new();
        let result = run_with_options(yaml, "step1", None, &handler).await;
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        assert_eq!(
            handler.call_count(),
            0,
            "option handler should not be called for command steps"
        );
    }

    #[tokio::test]
    async fn test_execute_steps_option_step_calls_handler() {
        let yaml = r#"
command: [echo]
steps:
  choose:
    option:
      - selector: "Continue"
        next: done
      - selector: "Cancel"
  done:
    command: "echo done"
"#;
        let handler = crate::option_handler::FirstChoiceOptionHandler::new();
        let result = run_with_options(yaml, "choose", None, &handler).await;
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        assert_eq!(
            handler.call_count(),
            1,
            "option handler should be called exactly once for the option step"
        );
    }

    #[tokio::test]
    async fn test_execute_steps_option_step_handler_next_step_followed() {
        let yaml = r#"
command: [echo]
steps:
  choose:
    option:
      - selector: "Skip to final"
        next: final
      - selector: "Go middle"
        next: middle
  middle:
    command: "exit 1"
  final:
    command: "echo reached_final"
"#;
        let handler = crate::option_handler::FirstChoiceOptionHandler::new();
        let result = run_with_options(yaml, "choose", None, &handler).await;
        assert!(result.is_ok(), "expected Ok but got: {result:?}");
        let result = result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            result.run, 2,
            "choose (option) + final should run; middle should be skipped via next=final"
        );
    }

    #[tokio::test]
    async fn test_execution_result_run_counter_counts_all_executed_steps() {
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo a"
  step2:
    command: "echo b"
  step3:
    command: "echo c"
"#;
        let result = run_config(yaml, "", None).await;
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));
        assert_eq!(result.run, 3, "all three steps should be counted as run");
        assert_eq!(result.skipped, 0, "no steps should be skipped");
    }

    #[tokio::test]
    async fn test_execution_result_skipped_counter_group_max_retries_zero() {
        let yaml = r#"
command: [echo]
groups:
  my_group:
    max_retries: 0
    steps:
      substep1:
        command: "exit 1"
      substep2:
        command: "exit 1"
steps:
  before:
    command: "echo before"
  group_call:
    group: my_group
  after:
    command: "echo after"
"#;
        let result = run_config(yaml, "", None).await;
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));
        assert_eq!(
            result.skipped, 2,
            "both group steps should be counted as skipped"
        );
        assert_eq!(
            result.run, 2,
            "before and after steps should be counted as run"
        );
    }

    // --- user-selected skipped_steps tests ---

    #[tokio::test]
    async fn test_user_skipped_step_increments_skipped_counter() {
        // Given: a workflow with 3 command steps where step2 would fail if executed
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo step1"
  step2:
    command: "exit 1"
  step3:
    command: "echo step3"
"#;
        // When: step2 is in user-selected skipped_steps
        let result = run_config_with_skipped(yaml, "", None, &["step2"]).await;
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));

        // Then: step2 is counted as skipped, step1 and step3 run
        assert_eq!(result.skipped, 1, "one step should be counted as skipped");
        assert_eq!(result.run, 2, "two steps should be counted as run");
    }

    #[tokio::test]
    async fn test_user_skipped_step_is_not_executed() {
        // Given: a workflow where a step would fail if executed
        let yaml = r#"
command: [echo]
steps:
  before:
    command: "echo before"
  should_not_run:
    command: "exit 1"
  after:
    command: "echo after"
"#;
        // When: the failing step is in user-selected skipped_steps
        let result = run_config_with_skipped(yaml, "", None, &["should_not_run"]).await;

        // Then: the workflow succeeds because the failing step was not executed
        assert!(
            result.is_ok(),
            "workflow should succeed when failing step is user-skipped: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_user_skip_nonexistent_step_name_is_silently_ignored() {
        // Given: a workflow with 2 steps
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo step1"
  step2:
    command: "echo step2"
"#;
        // When: a step name that does not exist is in skipped_steps
        let result = run_config_with_skipped(yaml, "", None, &["does_not_exist"]).await;
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));

        // Then: all steps run normally, skipped counter remains 0
        assert_eq!(result.run, 2, "all steps should run");
        assert_eq!(
            result.skipped, 0,
            "nonexistent step name should not affect skip count"
        );
    }

    #[tokio::test]
    async fn test_yaml_skip_and_user_skip_are_both_respected() {
        // Given: a workflow with one YAML-based skip and one user-selected skip
        let yaml = r#"
command: [echo]
steps:
  yaml_skipped:
    command: "exit 1"
    skip: true
  user_skipped:
    command: "exit 1"
  step3:
    command: "echo done"
"#;
        // When: user_skipped is also in skipped_steps (yaml_skipped is handled by YAML)
        let result = run_config_with_skipped(yaml, "", None, &["user_skipped"]).await;
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));

        // Then: both mechanisms each skip one step; step3 runs
        assert_eq!(result.skipped, 2, "both steps should be skipped");
        assert_eq!(result.run, 1, "only step3 should run");
    }

    #[tokio::test]
    async fn test_user_skip_empty_list_runs_all_steps() {
        // Given: a workflow with 2 steps
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo step1"
  step2:
    command: "echo step2"
"#;
        // When: skipped_steps is empty (no user selection)
        let result = run_config_with_skipped(yaml, "", None, &[]).await;
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));

        // Then: all steps run, nothing is skipped
        assert_eq!(
            result.run, 2,
            "all steps should run when skipped_steps is empty"
        );
        assert_eq!(result.skipped, 0, "no steps should be skipped");
    }

    #[tokio::test]
    async fn test_user_skip_multiple_steps_all_skipped() {
        // Given: a workflow with 4 steps
        let yaml = r#"
command: [echo]
steps:
  step1:
    command: "echo step1"
  step2:
    command: "exit 1"
  step3:
    command: "exit 1"
  step4:
    command: "echo step4"
"#;
        // When: step2 and step3 are both in user-selected skipped_steps
        let result = run_config_with_skipped(yaml, "", None, &["step2", "step3"]).await;
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));

        // Then: step2 and step3 are skipped, step1 and step4 run
        assert_eq!(result.skipped, 2, "two steps should be skipped");
        assert_eq!(result.run, 2, "two steps should run");
    }

    // --- user-selected skipped_steps for grouped steps ---

    #[tokio::test]
    async fn test_user_skipped_grouped_step_is_not_executed() {
        // Given: a workflow with a group call where one substep would fail if executed
        let yaml = r#"
command: [echo]
groups:
  review:
    steps:
      simplify:
        command: "exit 1"
      coderabbit:
        command: "echo coderabbit"
steps:
  before:
    command: "echo before"
  review-pass:
    group: review
  after:
    command: "echo after"
"#;
        // When: the failing substep is in user-selected skipped_steps (expanded ID)
        let result = run_config_with_skipped(yaml, "", None, &["review-pass/simplify"]).await;

        // Then: the workflow succeeds because the failing substep was not executed
        assert!(
            result.is_ok(),
            "workflow should succeed when grouped substep is user-skipped: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_user_skipped_grouped_step_increments_skipped_counter() {
        // Given: a workflow with a group call
        let yaml = r#"
command: [echo]
groups:
  review:
    steps:
      simplify:
        command: "echo simplify"
      coderabbit:
        command: "echo coderabbit"
steps:
  before:
    command: "echo before"
  review-pass:
    group: review
  after:
    command: "echo after"
"#;
        // When: one grouped substep is in user-selected skipped_steps
        let result = run_config_with_skipped(yaml, "", None, &["review-pass/simplify"]).await;
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));

        // Then: the grouped substep is counted as skipped
        assert_eq!(
            result.skipped, 1,
            "one grouped step should be counted as skipped"
        );
        assert_eq!(
            result.run, 3,
            "before, other group step, and after should run"
        );
    }

    #[tokio::test]
    async fn test_user_skipped_multiple_grouped_steps() {
        // Given: a workflow with a group call
        let yaml = r#"
command: [echo]
groups:
  review:
    steps:
      simplify:
        command: "exit 1"
      coderabbit:
        command: "exit 1"
steps:
  before:
    command: "echo before"
  review-pass:
    group: review
  after:
    command: "echo after"
"#;
        // When: both substeps are in user-selected skipped_steps
        let result = run_config_with_skipped(
            yaml,
            "",
            None,
            &["review-pass/simplify", "review-pass/coderabbit"],
        )
        .await;
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));

        // Then: both grouped substeps are skipped, before and after run
        assert_eq!(result.skipped, 2, "both grouped steps should be skipped");
        assert_eq!(result.run, 2, "before and after should run");
    }

    #[tokio::test]
    async fn test_user_skipped_grouped_step_with_non_group_step() {
        // Given: a workflow with both regular and grouped steps
        let yaml = r#"
command: [echo]
groups:
  review:
    steps:
      simplify:
        command: "exit 1"
steps:
  step1:
    command: "echo step1"
  review-call:
    group: review
  step2:
    command: "exit 1"
"#;
        // When: both a regular step and a grouped substep are in skipped_steps
        let result =
            run_config_with_skipped(yaml, "", None, &["step2", "review-call/simplify"]).await;
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));

        // Then: both are skipped, step1 runs
        assert_eq!(result.skipped, 2, "two steps should be skipped");
        assert_eq!(result.run, 1, "one step should run");
    }

    // --- snapshot regression tests for group-level if.file-changed with skipped first step ---

    #[cfg(unix)]
    #[tokio::test]
    async fn test_group_first_step_skipped_snapshot_still_taken() {
        // Regression test: when the first substep of a group is user-skipped,
        // the group-level if.file-changed should still work because the group
        // snapshot needs to be taken even when the first step is skipped.
        let dir = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let marker_file = dir.path().join("changed.txt");
        let yaml = format!(
            r#"
command: [echo]
groups:
  review:
    max_retries: 1
    if:
      file-changed: bootstrap
    steps:
      first:
        command: "echo first"
      second:
        command: "touch {}"
steps:
  bootstrap:
    command: "echo bootstrap"
  review-pass:
    group: review
  done:
    command: "echo done"
"#,
            marker_file.display()
        );
        // When: first substep is skipped but second substep modifies a file
        // and group-level if.file-changed jumps back to an earlier step
        let skipped: Vec<String> = vec!["review-pass/first".to_string()];
        let result = run_config_inner(
            &yaml,
            "",
            None,
            dir.path().to_path_buf(),
            10,
            0,
            None,
            None,
            &NoOpOptionHandler,
            &skipped,
        )
        .await;
        let result = result.unwrap_or_else(|e| panic!("workflow failed: {e:?}"));
        // Then: the group-level if.file-changed should trigger, causing bootstrap
        // to run a second time before the group's retry budget skips the group.
        assert_eq!(
            result.run, 4,
            "expected bootstrap -> second -> bootstrap -> done"
        );
        assert_eq!(
            result.skipped, 3,
            "expected the initial skipped first step plus one skipped group retry"
        );
    }
}
