use indexmap::IndexMap;
use serde::Serialize;

use super::{
    CancellationToken, CommandStepOptions, CruiseError, Duration, ExecutionContext, HashMap,
    Result, StepExecOutcome, StepKind, VariableStore, run_command_step, run_prompt_step,
    should_skip, should_skip_due_to_when,
};
use crate::config::StepConfig;

#[cfg(test)]
mod tests;

#[derive(Serialize)]
struct ChildResult {
    output: Option<String>,
    stderr: String,
    success: bool,
    skipped: bool,
}

/// A parallel block owns one DAG checkpoint. Branches only mutate private
/// variable stores; the parent publishes their results in declaration order.
/// All futures are drained, including during timeout/cancellation, so no child
/// process can keep modifying the workspace after the next step starts.
pub(super) async fn run_parallel_step(
    ctx: &ExecutionContext<'_>,
    children: &IndexMap<String, StepConfig>,
    vars: &mut VariableStore,
    inherited_env: &HashMap<String, String>,
    timeout: Option<Duration>,
    has_if_fail: bool,
    parent_name: &str,
) -> Result<StepExecOutcome> {
    let cancel = ctx
        .cancel_token
        .map_or_else(CancellationToken::new, CancellationToken::child_token);
    let child_ctx = ExecutionContext {
        cancel_token: Some(&cancel),
        ..*ctx
    };
    let futures = children.iter().map(|(name, child)| {
        run_child(
            &child_ctx,
            child,
            vars.clone(),
            inherited_env,
            format!("{parent_name}/{name}"),
        )
    });
    let joined = futures::future::join_all(futures);
    tokio::pin!(joined);
    let (results, timed_out) = if let Some(duration) = timeout {
        if let Ok(results) = tokio::time::timeout(duration, &mut joined).await {
            (results, false)
        } else {
            let message = format!(
                "[{parent_name}] parallel step timed out after {}s",
                duration.as_secs()
            );
            if let Some(log) = ctx.on_step_log {
                log("stderr", &message);
            } else {
                crate::status_eprintln!("{message}");
            }
            cancel.cancel();
            (joined.await, true)
        }
    } else {
        (joined.await, false)
    };

    if ctx
        .cancel_token
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(CruiseError::Interrupted);
    }
    let mut outputs = IndexMap::new();
    let mut fatal_error = None;
    let mut stderr = Vec::new();
    for ((name, _), result) in children.iter().zip(results) {
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                let message = error.to_string();
                // A block timeout cancels unfinished children, but the block
                // itself follows the same failure path as an ordinary timeout.
                let fatal = match &error {
                    CruiseError::CommitGuardViolation(_) => true,
                    CruiseError::Interrupted => !timed_out,
                    CruiseError::StepTimeout { .. } => false,
                    _ => !has_if_fail,
                };
                if fatal && fatal_error.is_none() {
                    fatal_error = Some(error);
                }
                ChildResult {
                    output: None,
                    stderr: message,
                    success: false,
                    skipped: false,
                }
            }
        };
        if !output.stderr.is_empty() {
            stderr.push(format!("[{parent_name}/{name}] {}", output.stderr));
        }
        outputs.insert(name, output);
    }
    if timed_out {
        stderr.push(format!("[{parent_name}] parallel step timed out"));
    }
    let failed = timed_out || outputs.values().any(|output| !output.success);
    vars.set_prev_output(Some(serde_json::to_string(&outputs)?));
    vars.set_prev_stderr(Some(stderr.join("\n")));
    vars.set_prev_success(Some(!failed));
    vars.set_prev_input(None);
    if let Some(error) = fatal_error {
        return Err(error);
    }
    Ok(StepExecOutcome {
        option_next: None,
        failed,
        skip_step_reason: None,
    })
}

async fn run_child(
    ctx: &ExecutionContext<'_>,
    child: &StepConfig,
    mut vars: VariableStore,
    inherited_env: &HashMap<String, String>,
    name: String,
) -> Result<ChildResult> {
    // Only the block calls on_step_start: persisting a child as current_step
    // would create a resume point that does not exist in the execution DAG.
    let log = |stream: &str, line: &str| {
        if let Some(log) = ctx.on_step_log {
            log(stream, &format!("[{name}] {line}"));
        }
        crate::status_eprintln!("[{name}] {line}");
    };
    if ctx
        .cancel_token
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(CruiseError::Interrupted);
    }
    if should_skip(child.skip.as_ref(), &vars)?
        || should_skip_due_to_when(child.when.as_ref(), &vars, ctx.working_dir)?
    {
        log("info", "skipped");
        return Ok(ChildResult {
            output: None,
            stderr: String::new(),
            success: true,
            skipped: true,
        });
    }
    let mut env = inherited_env.clone();
    for (key, value) in &child.env {
        // Inherited values were already resolved by the parent. Resolving them
        // again would reinterpret literal braces in input/environment values.
        env.insert(key.clone(), vars.resolve(value)?);
    }
    let timeout = child
        .timeout
        .as_deref()
        .map(crate::timeout::parse_timeout)
        .transpose()?;
    log("info", "started");
    let result = async {
        let success = match StepKind::try_from(child.clone())? {
            StepKind::Prompt(step) => {
                run_prompt_step(
                    &mut vars,
                    ctx.compiled,
                    &step,
                    ctx.rate_limit_retries,
                    &env,
                    ctx.cancel_token,
                    ctx.working_dir,
                    super::PromptStepOutput::Redirect(&log),
                    timeout,
                    &name,
                    false,
                    false,
                )
                .await?;
                true
            }
            StepKind::Command(step) => {
                run_command_step(
                    &mut vars,
                    &step,
                    CommandStepOptions {
                        rate_limit_retries: ctx.rate_limit_retries,
                        env: &env,
                        working_dir: ctx.working_dir,
                        timeout,
                        on_step_log: Some(&log),
                        cancel_token: ctx.cancel_token,
                    },
                )
                .await?
            }
            StepKind::Option(_) | StepKind::Parallel(_) => {
                return Err(CruiseError::InvalidStepConfig(format!(
                    "parallel child '{name}' must be a prompt or command step"
                )));
            }
        };
        Ok(ChildResult {
            output: vars.prev_output().map(str::to_string),
            stderr: vars.prev_stderr().unwrap_or_default().to_string(),
            success,
            skipped: false,
        })
    }
    .await;
    match &result {
        Ok(output) => log(
            "info",
            if output.success {
                "completed"
            } else {
                "failed"
            },
        ),
        Err(error) => log("stderr", &error.to_string()),
    }
    result
}
