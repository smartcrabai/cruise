use opentelemetry::trace::FutureExt as OtelFutureExt;
/// Shared worktree PR helper functions used by both the CLI and the `WebUI`.
///
/// Functions in this module are extracted from `run_cmd.rs` so that
/// `src/webui` can call the same PR post-processing logic
/// without duplicating it.
use std::path::Path;

use console::style;

use crate::cancellation::CancellationToken;
use crate::engine::{ExecutionContext, execute_steps_with_graph};
use crate::error::{CruiseError, Result};
use crate::file_tracker::FileTracker;
use crate::option_handler::OptionHandler;
use crate::session::SessionState;
use crate::variable::VariableStore;
use crate::workflow::CompiledWorkflow;
use crate::worktree;

// --- Constants ----------------------------------------------------------------

const PR_NUMBER_VAR: &str = "pr.number";
pub(crate) const PR_URL_VAR: &str = "pr.url";
const PR_LANGUAGE_VAR: &str = "pr.language";
const COMMIT_COAUTHOR_NAME_ENV: &str = "CRUISE_COMMIT_COAUTHOR_NAME";
const COMMIT_COAUTHOR_EMAIL_ENV: &str = "CRUISE_COMMIT_COAUTHOR_EMAIL";
const CREATE_PR_PROMPT_TEMPLATE: &str = include_str!("../prompts/create-pr.md");

// --- Types --------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitOutcome {
    Created,
    NoChanges,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrAttemptOutcome {
    Created {
        url: String,
        commit_outcome: CommitOutcome,
    },
    SkippedNoCommits,
    CreateFailed {
        error: String,
        commit_outcome: CommitOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommitCoauthor {
    name: String,
    email: String,
}

impl PrAttemptOutcome {
    pub(crate) fn report(&self) {
        match self {
            Self::Created { commit_outcome, .. } | Self::CreateFailed { commit_outcome, .. } => {
                report_commit_outcome(*commit_outcome);
            }
            Self::SkippedNoCommits => {}
        }
    }
}

fn report_commit_outcome(commit_outcome: CommitOutcome) {
    match commit_outcome {
        CommitOutcome::Created => {
            crate::status_eprintln!("{} Changes committed", style("v").green().bold());
        }
        CommitOutcome::NoChanges => {
            // sakoku-ignore-next-line
            crate::status_eprintln!(
                "{} No new changes to commit; using existing branch commits",
                style("->").cyan()
            );
        }
    }
}

// --- Preflight ---------------------------------------------------------------

/// Verify that the `gh` CLI is available in `PATH`.
///
/// Repo-backed sessions check this before cloning, where the CLI may be needed
/// for repository access. Local worktrees check only after finding commits or
/// changes that make PR processing necessary.
///
/// # Errors
///
/// Returns [`CruiseError::Other`] if `gh` is not found in `PATH` or exits
/// with a non-zero status.
pub fn ensure_gh_available() -> Result<()> {
    let ok = std::process::Command::new("gh")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success());

    if ok {
        Ok(())
    } else {
        Err(CruiseError::Other(
            "gh CLI is not installed. Install it from https://cli.github.com/".to_string(),
        ))
    }
}

// --- PR post-processing ------------------------------------------------------

/// Callback used to persist a session after a pull request is created.
pub type PrCreatedCallback<'a> = dyn Fn(&SessionState) -> Result<()> + Send + Sync + 'a;

/// Callback invoked once a worktree has commits or changes warranting PR work.
pub type PrProcessingCallback<'a> = dyn Fn() -> Result<()> + Send + Sync + 'a;

pub(crate) fn log_after_pr_skipped(
    compiled: &CompiledWorkflow,
    flags: &crate::flags::FlagValues,
    skipped_steps: &[String],
    reason: &str,
) {
    if compiled.after_pr.iter().any(|(name, step)| {
        !skipped_steps.iter().any(|skipped| skipped == name)
            && !crate::flags::is_disabled(step.flag.as_ref(), flags)
    }) {
        let message = format!("after-pr actions skipped: {reason}");
        crate::status_eprintln!("{} {message}", style("!").yellow());
    }
}

fn squash_merge_enabled(compiled: &CompiledWorkflow, flags: &crate::flags::FlagValues) -> bool {
    flags.get("merge").copied().unwrap_or(false)
        && compiled
            .after_pr
            .values()
            .any(|step| step.squash_merge == Some(true))
}

/// Handle PR creation and after-PR steps, persisting the session immediately
/// after a PR is created when `on_pr_created` is provided.
///
/// # Errors
///
/// Returns an error when description generation, pull-request creation, post-PR
/// execution, persistence, or cancellation fails.
#[expect(
    clippy::too_many_arguments,
    reason = "all params are needed for PR flow"
)]
pub async fn handle_worktree_pr_with_persistence(
    ctx: &worktree::WorktreeContext,
    compiled: &CompiledWorkflow,
    vars: &mut VariableStore,
    tracker: &mut FileTracker,
    session: &mut SessionState,
    rate_limit_retries: usize,
    max_retries: usize,
    skipped_steps: &[String],
    flags: &crate::flags::FlagValues,
    cancel_token: Option<&CancellationToken>,
    option_handler: &dyn OptionHandler,
    on_step_log: Option<&crate::step::command::StepLogCallback<'_>>,
    on_pr_processing: Option<&PrProcessingCallback<'_>>,
    on_pr_created: Option<&PrCreatedCallback<'_>>,
) -> Result<()> {
    if cancel_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(CruiseError::Interrupted);
    }
    let skipped_reason = if compiled.create_pr {
        session.pr_skipped_reason = None;
        pr_creation_skip_reason(ctx, cancel_token).await?
    } else {
        Some(crate::session::PrSkippedReason::Disabled)
    };
    if let Some(reason) = skipped_reason {
        mark_pr_skipped(session, compiled, flags, skipped_steps, reason);
        return Ok(());
    }
    let operation = crate::telemetry::Operation::phase("pull_request");
    let mut pr_creation_failed = false;
    let result: Result<()> = async {
        // Repo-backed sessions need forge availability before execution to clone
        // private repositories; local worktrees defer this until PR processing is
        // actually needed.
        if session.repo.is_none() {
            crate::forge::ensure_forge_available(crate::forge::session_forge(session))?;
        }
        if let Some(on_pr_processing) = on_pr_processing {
            on_pr_processing()?;
        }

        let pr_attempt = create_pr_attempt(
            ctx,
            compiled,
            vars,
            session,
            rate_limit_retries,
            cancel_token,
            on_step_log,
        )
        .await?;
        pr_attempt.report();
        match pr_attempt {
            PrAttemptOutcome::Created { url, .. } => {
                crate::status_eprintln!("{} PR created: {}", style("v").green().bold(), url);
                if let Some(number) = extract_last_path_segment(&url) {
                    vars.set_named_value(PR_NUMBER_VAR, number);
                }
                vars.set_named_value(PR_URL_VAR, url.clone());
                session.pr_url = Some(url);
                session.pr_skipped_reason = None;
                if let Some(persist) = on_pr_created {
                    persist(session)?;
                }
                if cancel_token.is_some_and(CancellationToken::is_cancelled) {
                    return Err(CruiseError::Interrupted);
                }
                run_after_pr_steps(
                    compiled,
                    vars,
                    tracker,
                    max_retries,
                    rate_limit_retries,
                    ctx.path.as_path(),
                    skipped_steps,
                    flags,
                    cancel_token,
                    option_handler,
                    on_step_log,
                )
                .await?;
                Ok(())
            }
            PrAttemptOutcome::SkippedNoCommits => {
                session.pr_skipped_reason = Some(crate::session::PrSkippedReason::NoCommits);
                log_after_pr_skipped(
                    compiled,
                    flags,
                    skipped_steps,
                    "the worktree has no commits or changes to open a pull request",
                );
                Ok(())
            }
            PrAttemptOutcome::CreateFailed { error, .. } => {
                if session.repo.is_some() {
                    return Err(CruiseError::Other(format!("PR creation failed: {error}")));
                }
                crate::status_eprintln!("warning: PR creation failed: {error}");
                pr_creation_failed = true;
                Ok(())
            }
        }
    }
    .with_context(operation.context())
    .await;
    if pr_creation_failed && result.is_ok() {
        operation.set_error_type("pr_creation_failed");
        operation.finish("error");
    } else {
        operation.finish_result(&result);
    }
    result
}

async fn pr_creation_skip_reason(
    ctx: &worktree::WorktreeContext,
    cancel_token: Option<&CancellationToken>,
) -> Result<Option<crate::session::PrSkippedReason>> {
    if branch_commit_count(ctx, cancel_token).await? == 0
        && !worktree_has_changes(ctx, cancel_token).await?
    {
        Ok(Some(crate::session::PrSkippedReason::NoCommits))
    } else {
        Ok(None)
    }
}

fn mark_pr_skipped(
    session: &mut SessionState,
    compiled: &CompiledWorkflow,
    flags: &crate::flags::FlagValues,
    skipped_steps: &[String],
    reason: crate::session::PrSkippedReason,
) {
    let message = match &reason {
        crate::session::PrSkippedReason::Disabled => "pull-request creation is disabled",
        crate::session::PrSkippedReason::NoCommits => {
            "the worktree has no commits or changes to open a pull request"
        }
    };
    session.pr_skipped_reason = Some(reason);
    log_after_pr_skipped(compiled, flags, skipped_steps, message);
}

async fn create_pr_attempt(
    ctx: &worktree::WorktreeContext,
    compiled: &CompiledWorkflow,
    vars: &mut VariableStore,
    session: &SessionState,
    rate_limit_retries: usize,
    cancel_token: Option<&CancellationToken>,
    on_step_log: Option<&crate::step::command::StepLogCallback<'_>>,
) -> Result<PrAttemptOutcome> {
    let (pr_title, pr_body) = generate_pr_description(
        &ctx.path,
        compiled,
        vars,
        rate_limit_retries,
        cancel_token,
        on_step_log,
    )
    .await?;
    if cancel_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(CruiseError::Interrupted);
    }
    attempt_pr_creation(
        ctx,
        crate::forge::session_forge(session),
        session.input_or_title(),
        &pr_title,
        &pr_body,
        cancel_token,
    )
    .await
}

/// Generate a PR title and body. Returns `Err(Interrupted)` on cancellation and
/// propagates [`CruiseError::MissingJcodeModel`]; other failures return empty
/// strings (best-effort PR description).
async fn generate_pr_description(
    worktree_path: &Path,
    compiled: &CompiledWorkflow,
    vars: &mut VariableStore,
    rate_limit_retries: usize,
    cancel_token: Option<&CancellationToken>,
    on_step_log: Option<&crate::step::command::StepLogCallback<'_>>,
) -> Result<(String, String)> {
    let pr_prompt = match build_pr_prompt(vars, compiled) {
        Err(e) => {
            crate::status_eprintln!("warning: PR prompt resolution failed: {e}");
            return Ok((String::new(), String::new()));
        }
        Ok(p) => p,
    };
    let executor = crate::executor::Executor::new(compiled.sdk.as_deref(), &compiled.command);
    let model = compiled.model.as_ref();
    let spinner = crate::spinner::Spinner::start("Generating PR description...");
    let env = std::collections::HashMap::new();

    if executor.is_sdk() {
        let result = generate_pr_via_sdk_tool(
            &executor,
            worktree_path,
            &pr_prompt,
            model,
            rate_limit_retries,
            &env,
            &compiled.mcp_servers,
            &spinner,
            cancel_token,
            on_step_log,
        )
        .await;
        drop(spinner);
        return result;
    }

    let output = {
        let on_notice = |msg: &str| {
            spinner.suspend(|| crate::status_eprintln!("{msg}"));
            if let Some(cb) = on_step_log {
                cb("info", msg);
            }
        };
        match executor
            .run(crate::executor::PromptRun {
                prompt: &pr_prompt,
                model,
                subagent_model: None,
                max_retries: rate_limit_retries,
                env: &env,
                mcp_servers: &compiled.mcp_servers,
                on_notice: Some(&on_notice),
                cancel_token,
                working_dir: Some(worktree_path),
                stream: None,
                tools: Vec::new(),
                on_session_id: None,
                resume: None,
                computer_use: false,
                permission: crate::config::PermissionMode::Full,
            })
            .await
        {
            Ok(o) => o.result.output,
            Err(CruiseError::Interrupted) => {
                drop(spinner);
                return Err(CruiseError::Interrupted);
            }
            Err(e) => {
                crate::status_eprintln!("warning: PR description generation failed: {e}");
                String::new()
            }
        }
    };
    drop(spinner);
    let (pr_title, pr_body) = parse_pr_metadata(&output);
    if pr_title.is_empty() && !output.trim().is_empty() {
        let truncated: String = output.chars().take(500).collect();
        crate::status_eprintln!(
            "{} Failed to parse PR metadata from LLM output (first 500 chars):\n{}",
            style("!").yellow(),
            truncated
        );
    }
    Ok((pr_title, pr_body))
}

#[expect(
    clippy::too_many_arguments,
    reason = "PR metadata execution needs the executor, worktree, prompt, retry, environment, MCP servers, spinner, and cancellation state"
)]
async fn generate_pr_via_sdk_tool(
    executor: &crate::executor::Executor,
    worktree_path: &Path,
    pr_prompt: &str,
    model: Option<&crate::config::ModelSpec>,
    rate_limit_retries: usize,
    env: &std::collections::HashMap<String, String>,
    mcp_servers: &crate::config::McpServers,
    spinner: &crate::spinner::Spinner,
    cancel_token: Option<&CancellationToken>,
    on_step_log: Option<&crate::step::command::StepLogCallback<'_>>,
) -> Result<(String, String)> {
    use std::sync::{Arc, Mutex};

    let store = Arc::new(Mutex::new(None::<crate::sdk_tools::PrMetadata>));
    let tool = crate::sdk_tools::submit_pr_metadata_tool(Arc::clone(&store));
    let prompt = format!(
        "{pr_prompt}\n\n\
         Call the submit_pr_metadata tool with the title and body."
    );
    let on_notice = |msg: &str| {
        spinner.suspend(|| crate::status_eprintln!("{msg}"));
        if let Some(cb) = on_step_log {
            cb("info", msg);
        }
    };
    match executor
        .run(crate::executor::PromptRun {
            prompt: &prompt,
            model,
            subagent_model: None,
            max_retries: rate_limit_retries,
            env,
            mcp_servers,
            on_notice: Some(&on_notice),
            cancel_token,
            working_dir: Some(worktree_path),
            stream: None,
            tools: vec![tool],
            on_session_id: None,
            resume: None,
            computer_use: false,
            permission: crate::config::PermissionMode::Full,
        })
        .await
    {
        Ok(_) => {
            if let Ok(mut guard) = store.lock()
                && let Some(meta) = guard.take()
            {
                Ok((meta.title, meta.body))
            } else {
                crate::status_eprintln!("warning: SDK agent did not call submit_pr_metadata tool");
                Ok((String::new(), String::new()))
            }
        }
        Err(CruiseError::Interrupted) => Err(CruiseError::Interrupted),
        Err(e @ CruiseError::MissingJcodeModel(_)) => Err(e),
        Err(e) => {
            crate::status_eprintln!("warning: PR description generation failed: {e}");
            Ok((String::new(), String::new()))
        }
    }
}

/// Run the after-PR workflow steps. Cancellation, missing-model errors, and
/// GitHub review failures always propagate; other errors only warn unless
/// native squash merge is opted in.
#[expect(clippy::too_many_arguments, reason = "mirrors the PR flow parameters")]
async fn run_after_pr_steps(
    compiled: &CompiledWorkflow,
    vars: &mut VariableStore,
    tracker: &mut FileTracker,
    max_retries: usize,
    rate_limit_retries: usize,
    working_dir: &std::path::Path,
    skipped_steps: &[String],
    flags: &crate::flags::FlagValues,
    cancel_token: Option<&CancellationToken>,
    option_handler: &dyn OptionHandler,
    on_step_log: Option<&crate::step::command::StepLogCallback<'_>>,
) -> Result<()> {
    if compiled.after_pr.is_empty() {
        return Ok(());
    }
    let critical_merge_enabled = squash_merge_enabled(compiled, flags);
    let after_compiled = compiled.to_after_pr_compiled();
    let mut dag = crate::graph::build_graph(&after_compiled, max_retries)?;
    crate::graph::validation::validate_execution_graph(
        &dag,
        &after_compiled,
        &dag.start,
        &crate::flags::preflight_skips(compiled, flags, skipped_steps),
        "after-pr",
    )?;
    let ctx = ExecutionContext {
        compiled: &after_compiled,
        max_retries,
        rate_limit_retries,
        on_step_start: &|_| Ok(()),
        cancel_token,
        option_handler,
        config_reloader: None,
        working_dir: Some(working_dir),
        skipped_steps,
        flags,
        on_step_log,
    };
    match execute_steps_with_graph(&ctx, vars, tracker, &mut dag, &|_cp, _dag| Ok(())).await {
        Err(CruiseError::Interrupted) => Err(CruiseError::Interrupted),
        Err(e @ (CruiseError::MissingJcodeModel(_) | CruiseError::GitHubReviewFailed { .. })) => {
            Err(e)
        }
        Err(e) if critical_merge_enabled => Err(e),
        Ok(_) | Err(CruiseError::StepPaused) => Ok(()),
        Err(e) => {
            crate::status_eprintln!("warning: after-pr steps failed: {e}");
            Ok(())
        }
    }
}

/// Test entry point mirroring the after-pr phase of `run_pr_flow`.
#[cfg(test)]
pub(crate) async fn run_after_pr_steps_for_test(
    compiled: &CompiledWorkflow,
    vars: &mut VariableStore,
    tracker: &mut FileTracker,
    working_dir: &Path,
    option_handler: &dyn OptionHandler,
) -> Result<()> {
    run_after_pr_steps(
        compiled,
        vars,
        tracker,
        10,
        0,
        working_dir,
        &[],
        &crate::flags::NO_FLAGS,
        None,
        option_handler,
        None,
    )
    .await
}

pub(crate) fn build_pr_prompt(
    vars: &mut VariableStore,
    compiled: &CompiledWorkflow,
) -> Result<String> {
    vars.set_named_value(PR_LANGUAGE_VAR, compiled.pr_language.clone());
    vars.resolve(CREATE_PR_PROMPT_TEMPLATE)
}

pub(crate) async fn attempt_pr_creation(
    ctx: &worktree::WorktreeContext,
    forge: crate::forge::ForgeKind,
    message: &str,
    title: &str,
    body: &str,
    cancel_token: Option<&CancellationToken>,
) -> Result<PrAttemptOutcome> {
    let trimmed_title = title.trim();
    let commit_message = if trimmed_title.is_empty() {
        message
    } else {
        trimmed_title
    };
    let commit_message = commit_message_with_coauthor(commit_message);
    let commit_outcome = commit_changes(&ctx.path, &commit_message, cancel_token).await?;
    if branch_commit_count(ctx, cancel_token).await? == 0 {
        return Ok(PrAttemptOutcome::SkippedNoCommits);
    }
    push_branch(&ctx.path, &ctx.branch, cancel_token).await?;
    match create_pr(
        forge,
        &ctx.path,
        &ctx.branch,
        trimmed_title,
        body,
        cancel_token,
    )
    .await
    {
        Ok(url) => Ok(PrAttemptOutcome::Created {
            url,
            commit_outcome,
        }),
        Err(CruiseError::Interrupted) => Err(CruiseError::Interrupted),
        Err(e) => Ok(PrAttemptOutcome::CreateFailed {
            error: e.to_string(),
            commit_outcome,
        }),
    }
}

fn commit_message_with_coauthor(message: &str) -> String {
    let Some(coauthor) = commit_coauthor_from_env() else {
        return message.to_string();
    };

    format!(
        "{message}\n\nCo-authored-by: {} <{}>",
        coauthor.name, coauthor.email
    )
}

fn commit_coauthor_from_env() -> Option<CommitCoauthor> {
    let name = std::env::var(COMMIT_COAUTHOR_NAME_ENV).ok()?;
    let email = std::env::var(COMMIT_COAUTHOR_EMAIL_ENV).ok()?;
    let name = name.trim();
    let email = email.trim();

    if !is_valid_commit_coauthor_name(name) || !is_valid_commit_coauthor_email(email) {
        return None;
    }

    Some(CommitCoauthor {
        name: name.to_string(),
        email: email.to_string(),
    })
}

fn is_valid_commit_coauthor_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('\n')
        && !name.contains('\r')
        && !name.contains('<')
        && !name.contains('>')
}

fn is_valid_commit_coauthor_email(email: &str) -> bool {
    !email.is_empty()
        && email.contains('@')
        && !email.chars().any(char::is_whitespace)
        && !email.contains('<')
        && !email.contains('>')
}

async fn branch_commit_count(
    ctx: &worktree::WorktreeContext,
    cancel_token: Option<&CancellationToken>,
) -> Result<usize> {
    let base_head = git_stdout(
        &ctx.original_dir,
        &["rev-parse", "HEAD"],
        "git rev-parse HEAD failed",
        cancel_token,
    )
    .await?;
    let merge_base = git_stdout(
        &ctx.path,
        &["merge-base", "HEAD", &base_head],
        "git merge-base failed",
        cancel_token,
    )
    .await?;
    let range = format!("{merge_base}..HEAD");
    let count = git_stdout(
        &ctx.path,
        &["rev-list", "--count", &range],
        "git rev-list --count failed",
        cancel_token,
    )
    .await?;
    count.parse::<usize>().map_err(|e| {
        CruiseError::Other(format!(
            "failed to parse branch commit count from `{count}`: {e}"
        ))
    })
}
async fn worktree_has_changes(
    ctx: &worktree::WorktreeContext,
    cancel_token: Option<&CancellationToken>,
) -> Result<bool> {
    let output = crate::step::command::run_process_output_cancelled(
        "git",
        &["status", "--porcelain", "--untracked-files=all"],
        Some(&ctx.path),
        cancel_token,
    )
    .await?;
    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "git status failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(!output.stdout.is_empty())
}

async fn git_stdout(
    current_dir: &Path,
    args: &[&str],
    context: &str,
    cancel_token: Option<&CancellationToken>,
) -> Result<String> {
    let output = crate::step::command::run_process_output_cancelled(
        "git",
        args,
        Some(current_dir),
        cancel_token,
    )
    .await
    .map_err(|error| match error {
        CruiseError::Interrupted => error,
        _ => CruiseError::Other(format!("{context}: {error}")),
    })?;
    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "{context}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        Err(CruiseError::Other(format!(
            "{context}: command produced no stdout"
        )))
    } else {
        Ok(stdout)
    }
}

/// Stage all changes and commit them.
async fn commit_changes(
    worktree_path: &Path,
    message: &str,
    cancel_token: Option<&CancellationToken>,
) -> Result<CommitOutcome> {
    let add = crate::step::command::run_process_output_cancelled(
        "git",
        &["add", "-A"],
        Some(worktree_path),
        cancel_token,
    )
    .await?;
    if !add.status.success() {
        return Err(CruiseError::Other(format!(
            "git add -A failed: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        )));
    }
    let diff = crate::step::command::run_process_output_cancelled(
        "git",
        &["diff", "--cached", "--quiet"],
        Some(worktree_path),
        cancel_token,
    )
    .await?;
    if diff.status.success() {
        return Ok(CommitOutcome::NoChanges);
    }
    let commit = crate::step::command::run_process_output_cancelled(
        "git",
        &["commit", "-m", message],
        Some(worktree_path),
        cancel_token,
    )
    .await?;
    if !commit.status.success() {
        return Err(CruiseError::Other(format!(
            "git commit failed: {}",
            String::from_utf8_lossy(&commit.stderr).trim()
        )));
    }
    Ok(CommitOutcome::Created)
}

async fn push_branch(
    worktree_path: &Path,
    branch: &str,
    cancel_token: Option<&CancellationToken>,
) -> Result<()> {
    let output = crate::step::command::run_process_output_cancelled(
        "git",
        &["push", "-u", "origin", branch],
        Some(worktree_path),
        cancel_token,
    )
    .await?;
    if !output.status.success() {
        return Err(CruiseError::Other(format!(
            "git push failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Create a draft PR using `gh pr create --draft`. Uses `--title`/`--body` if provided, otherwise
/// `--fill`. Falls back to `gh pr view` if a PR already exists.
async fn create_pr(
    forge: crate::forge::ForgeKind,
    worktree_path: &Path,
    branch: &str,
    title: &str,
    body: &str,
    cancel_token: Option<&CancellationToken>,
) -> Result<String> {
    if forge == crate::forge::ForgeKind::GitLab {
        return create_mr(worktree_path, branch, title, body, cancel_token).await;
    }
    let mut gh_args = vec!["pr", "create", "--head", branch, "--draft"];
    if title.is_empty() {
        gh_args.push("--fill");
    } else {
        gh_args.extend(["--title", title, "--body", body]);
    }
    let output = crate::step::command::run_process_output_cancelled(
        "gh",
        &gh_args,
        Some(worktree_path),
        cancel_token,
    )
    .await?;
    if output.status.success()
        && let Some(url) = gh_output_line(&output.stdout)
    {
        return Ok(url);
    }
    let fallback = crate::step::command::run_process_output_cancelled(
        "gh",
        &["pr", "view", branch, "--json", "url", "--jq", ".url"],
        Some(worktree_path),
        cancel_token,
    )
    .await?;
    if fallback.status.success()
        && let Some(url) = gh_output_line(&fallback.stdout)
    {
        return Ok(url);
    }
    let create_stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let view_stderr = String::from_utf8_lossy(&fallback.stderr).trim().to_string();
    Err(CruiseError::Other(format!(
        "gh pr create failed: {create_stderr}; gh pr view also failed: {view_stderr}"
    )))
}

/// Create a draft GitLab MR using `glab mr create --draft`. Falls back to `glab mr view`
/// if a merge request already exists for the branch.
async fn create_mr(
    worktree_path: &Path,
    branch: &str,
    title: &str,
    body: &str,
    cancel_token: Option<&CancellationToken>,
) -> Result<String> {
    let mut args = vec![
        "mr",
        "create",
        "--source-branch",
        branch,
        "--draft",
        "--yes",
    ];
    if title.is_empty() {
        args.push("--fill");
    } else {
        args.extend(["--title", title, "--description", body]);
    }
    let output = crate::step::command::run_process_output_cancelled(
        "glab",
        &args,
        Some(worktree_path),
        cancel_token,
    )
    .await?;
    if output.status.success()
        && let Some(url) = extract_mr_url(&output.stdout)
    {
        return Ok(url);
    }
    let fallback = crate::step::command::run_process_output_cancelled(
        "glab",
        &["mr", "view", branch, "--output", "json", "--jq", ".web_url"],
        Some(worktree_path),
        cancel_token,
    )
    .await?;
    if fallback.status.success()
        && let Some(url) = gh_output_line(&fallback.stdout)
    {
        return Ok(url);
    }
    let create_stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let view_stderr = String::from_utf8_lossy(&fallback.stderr).trim().to_string();
    Err(CruiseError::Other(format!(
        "glab mr create failed: {create_stderr}; glab mr view also failed: {view_stderr}"
    )))
}

/// Find the merge request URL in `glab mr create` stdout (falls back to the last line).
fn extract_mr_url(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .map(str::trim)
        .find(|l| l.starts_with("http") && l.contains("/merge_requests/"))
        .map(ToString::to_string)
        .or_else(|| gh_output_line(bytes).and_then(|l| l.lines().last().map(str::to_string)))
}

/// Trim and return a non-empty line from `gh` stdout bytes, or `None`.
pub(crate) fn gh_output_line(bytes: &[u8]) -> Option<String> {
    let cow = String::from_utf8_lossy(bytes);
    let trimmed = cow.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Extracts the last path segment from a URL, stripping any query string or fragment.
/// Returns `None` if the URL has no non-empty trailing path segment.
pub(crate) fn extract_last_path_segment(url: &str) -> Option<String> {
    url.rsplit('/')
        .next()
        .map(|s| s.split_once(['?', '#']).map_or(s, |(prefix, _)| prefix))
        .filter(|s| !s.is_empty())
        .map(std::string::ToString::to_string)
}

pub(crate) fn strip_code_block(s: &str) -> &str {
    let trimmed = s.trim();

    // Fast path: starts directly with ```
    if let Some(after_backticks) = trimmed.strip_prefix("```") {
        if let Some(newline_pos) = after_backticks.find('\n') {
            let inner = &after_backticks[newline_pos + 1..];
            if let Some(close) = inner.rfind("```") {
                return inner[..close].trim_end_matches('\n');
            }
        }
        return trimmed;
    }

    // Slow path: look for a ``` line somewhere in the text (preamble case).
    for (line_start, line) in iter_line_offsets(trimmed) {
        if line.starts_with("```") {
            let rest = &trimmed[line_start + line.len()..];
            let rest = skip_newline(rest);
            if let Some(close) = rest.rfind("```") {
                return rest[..close].trim_end_matches('\n');
            }
            break;
        }
    }

    trimmed
}

/// Strip a leading newline (`\r\n` or `\n`) from `s`, if present.
fn skip_newline(s: &str) -> &str {
    s.strip_prefix("\r\n")
        .or_else(|| s.strip_prefix('\n'))
        .unwrap_or(s)
}

/// Iterate over (`byte_offset_of_line_start`, `line_content`) pairs in `s`.
fn iter_line_offsets(s: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    s.split('\n').map(move |raw| {
        let start = offset;
        offset += raw.len() + 1;
        (start, raw.trim_end_matches('\r'))
    })
}

fn try_parse_heading_format(content: &str) -> Option<(String, String)> {
    for (line_start, line) in iter_line_offsets(content) {
        if let Some(rest) = line.strip_prefix("# ") {
            let title = rest.trim().to_string();
            if title.is_empty() {
                continue;
            }
            let after = &content[line_start + line.len()..];
            let after = skip_newline(after);
            return Some((title, after.to_string()));
        }
    }
    None
}

pub(crate) fn parse_pr_metadata(output: &str) -> (String, String) {
    let content = strip_code_block(output);

    // 1. Try parsing the whole content as frontmatter
    if let Some(result) = crate::metadata::try_parse_frontmatter(content) {
        return result;
    }

    // 2. Search for \n---\n in the text and try from that position
    if let Some(pos) = content.find("\n---\n")
        && let Some(result) = crate::metadata::try_parse_frontmatter(&content[pos + 1..])
    {
        return result;
    }

    // 3. Fallback: Markdown h1 heading format
    if let Some(result) = try_parse_heading_format(content) {
        return result;
    }

    (String::new(), String::new())
}

/// Merge the base branch of PR `pr_number` into the current branch.
///
/// A `pr_url` containing `/-/merge_requests/` is a GitLab merge request and is
/// queried with `glab`; otherwise `gh` is used.
///
/// # Errors
///
/// Returns an error when the base ref cannot be resolved or is invalid, or
/// when `git fetch` / `git merge` fails (including merge conflicts).
pub fn sync_base(pr_number: &str, pr_url: Option<&str>) -> Result<()> {
    sync_base_with(pr_number, pr_url, |program, args| {
        std::process::Command::new(program).args(args).output()
    })
}

/// Same as [`sync_base`] with an injectable command runner `(program, args)`.
pub(crate) fn sync_base_with<F>(pr_number: &str, pr_url: Option<&str>, mut runner: F) -> Result<()>
where
    F: FnMut(&str, &[&str]) -> std::io::Result<std::process::Output>,
{
    let mut run = |program: &str, args: &[&str]| -> Result<std::process::Output> {
        let output = runner(program, args).map_err(|e| {
            CruiseError::Other(format!("failed to run `{program} {}`: {e}", args.join(" ")))
        })?;
        if output.status.success() {
            Ok(output)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = [stderr.trim(), stdout.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            Err(CruiseError::Other(format!(
                "`{program} {}` failed: {detail}",
                args.join(" ")
            )))
        }
    };

    let is_gitlab_mr = pr_url.is_some_and(|url| url.contains("/-/merge_requests/"));
    let base = if is_gitlab_mr {
        let view = run("glab", &["mr", "view", pr_number, "--output", "json"])?;
        let json: serde_json::Value = serde_json::from_slice(&view.stdout)
            .map_err(|e| CruiseError::Other(format!("invalid `glab mr view` JSON: {e}")))?;
        json.get("target_branch")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        let view = run(
            "gh",
            &[
                "pr",
                "view",
                pr_number,
                "--json",
                "baseRefName",
                "-q",
                ".baseRefName",
            ],
        )?;
        String::from_utf8(view.stdout)
            .map_err(|e| CruiseError::Other(format!("base ref is not valid UTF-8: {e}")))?
            .trim()
            .to_string()
    };
    if base.is_empty() || base.starts_with('-') {
        return Err(CruiseError::Other(format!("invalid base ref: `{base}`")));
    }
    run("git", &["check-ref-format", "--branch", &base])
        .map_err(|e| CruiseError::Other(format!("invalid base ref `{base}`: {e}")))?;
    run("git", &["fetch", "origin", &base])?;
    run("git", &["merge", &format!("origin/{base}"), "--no-edit"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[tokio::test]
    async fn mixed_sdk_pr_description_uses_first_claude_candidate_under_jcode()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let _lock = crate::test_support::lock_process();
        let tmp = TempDir::new()?;
        let _home = crate::test_support::set_fake_home(tmp.path());
        let _path = crate::test_support::install_exiting_jcode_stub(tmp.path());
        let _jcode_home =
            crate::test_support::EnvGuard::set("JCODE_HOME", tmp.path().join("jcode-home"));
        let _sdk = crate::test_support::EnvGuard::remove("CRUISE_SDK");
        let _model = crate::test_support::EnvGuard::remove("CRUISE_MODEL");
        let _plan_model = crate::test_support::EnvGuard::remove("CRUISE_PLAN_MODEL");
        let _subagent_model = crate::test_support::EnvGuard::remove("CLAUDE_CODE_SUBAGENT_MODEL");
        let _subagent_force =
            crate::test_support::EnvGuard::remove("CLAUDE_CODE_SUBAGENT_MODEL_FORCE");
        let script = format!(
            r#"#!/bin/sh
case "$1" in --version) echo '2.0.0'; exit 0;; --cruise-exec-probe) exit 0;; esac
d='{}'
printf '%s\n' "$@" > "$d/args"
printf 'model=%s force=%s\n' "${{CLAUDE_CODE_SUBAGENT_MODEL-unset}}" "${{CLAUDE_CODE_SUBAGENT_MODEL_FORCE-unset}}" > "$d/subagent-env"
pwd > "$d/cwd"
read -r input
printf '%s\n' '{{"type":"control_request","request_id":"pr-request","request":{{"subtype":"mcp_message","server_name":"cruise","message":{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"submit_pr_metadata","arguments":{{"title":"Mixed SDK PR","body":"Metadata from the selected Claude candidate."}}}}}}}}}}'
read -r response
printf '%s\n' "$response" > "$d/response"
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"session_id":"mixed-pr"}}'
"#,
            tmp.path().display()
        );
        crate::test_support::write_executable_script(&tmp.path().join("claude"), &script);
        // Exercise the same SDK tool fixture before parsing mixed candidates.
        let control = crate::config::WorkflowConfig::from_yaml(
            "sdk: claude\nmodel: mixed-pr-model\nsteps:\n  build:\n    command: echo hi\n",
        )?;
        let control_compiled = crate::workflow::compile(control)?;
        let control_plan = tmp.path().join("control-plan.md");
        std::fs::write(&control_plan, "# Legacy SDK control plan")?;
        let mut control_vars = VariableStore::new("legacy SDK control".to_string());
        control_vars.set_named_file(crate::session::PLAN_VAR, control_plan);
        let control_metadata = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            generate_pr_description(
                tmp.path(),
                &control_compiled,
                &mut control_vars,
                0,
                None,
                None,
            ),
        )
        .await??;
        assert_eq!(
            control_metadata,
            (
                "Mixed SDK PR".to_string(),
                "Metadata from the selected Claude candidate.".to_string()
            )
        );
        let config = crate::config::WorkflowConfig::from_yaml(
            "sdk: jcode\nsubagent_model: {sdk: claude, model: haiku}\nmodel:\n  - sdk: claude\n    model: mixed-pr-model\n  - unused/jcode\nsteps:\n  build:\n    command: echo hi\n",
        )?;
        let compiled = crate::workflow::compile(config)?;
        // The configured generation must create its own dispatch evidence.
        std::fs::remove_file(tmp.path().join("subagent-env"))?;
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "# Mixed SDK plan")?;
        let mut vars = VariableStore::new("implement mixed SDK feature".to_string());
        vars.set_named_file(crate::session::PLAN_VAR, plan_path);
        let metadata = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            generate_pr_description(tmp.path(), &compiled, &mut vars, 0, None, None),
        )
        .await??;
        assert_eq!(
            metadata,
            (
                "Mixed SDK PR".to_string(),
                "Metadata from the selected Claude candidate.".to_string()
            )
        );
        let args = std::fs::read_to_string(tmp.path().join("args"))?;
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("subagent-env"))?,
            "model=unset force=unset\n"
        );
        assert!(args.lines().any(|line| line == "--model"));
        assert!(args.lines().any(|line| line == "mixed-pr-model"));
        let cwd = std::fs::read_to_string(tmp.path().join("cwd"))?;
        assert_eq!(
            std::fs::canonicalize(cwd.trim())?,
            std::fs::canonicalize(tmp.path())?
        );
        Ok(())
    }

    // --- sync_base_with -----------------------------------------------------

    fn output(code: i32, stdout: &[u8]) -> std::process::Output {
        #[cfg(unix)]
        let status = std::os::unix::process::ExitStatusExt::from_raw(code << 8);
        #[cfg(windows)]
        let status = std::os::windows::process::ExitStatusExt::from_raw(code.unsigned_abs());
        std::process::Output {
            status,
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }
    }

    type Call = (String, Vec<String>);

    /// Runs `sync_base_with` with scripted responses (consumed in order),
    /// returning the result and the recorded calls.
    fn run_scripted(
        responses: Vec<std::io::Result<std::process::Output>>,
    ) -> (Result<()>, Vec<Call>) {
        run_scripted_url(None, responses)
    }

    /// [`run_scripted`] with an explicit PR URL.
    fn run_scripted_url(
        pr_url: Option<&str>,
        responses: Vec<std::io::Result<std::process::Output>>,
    ) -> (Result<()>, Vec<Call>) {
        let mut calls: Vec<Call> = Vec::new();
        let mut queue = responses.into_iter();
        let result = sync_base_with("42", pr_url, |program, args| {
            calls.push((
                program.to_string(),
                args.iter().map(|a| (*a).to_string()).collect(),
            ));
            queue
                .next()
                .unwrap_or_else(|| panic!("unexpected extra command: {program} {args:?}"))
        });
        (result, calls)
    }

    fn call(program: &str, args: &[&str]) -> Call {
        (
            program.to_string(),
            args.iter().map(|a| (*a).to_string()).collect(),
        )
    }

    fn assert_other(result: &Result<()>) {
        assert!(
            matches!(result, Err(CruiseError::Other(_))),
            "expected CruiseError::Other, got {result:?}"
        );
    }

    #[test]
    fn sync_base_runs_gh_check_ref_format_fetch_merge_in_order() {
        let (result, calls) = run_scripted(vec![
            Ok(output(0, b"main\n")),
            Ok(output(0, b"")),
            Ok(output(0, b"")),
            Ok(output(0, b"")),
        ]);
        result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            calls,
            vec![
                call(
                    "gh",
                    &[
                        "pr",
                        "view",
                        "42",
                        "--json",
                        "baseRefName",
                        "-q",
                        ".baseRefName"
                    ]
                ),
                call("git", &["check-ref-format", "--branch", "main"]),
                call("git", &["fetch", "origin", "main"]),
                call("git", &["merge", "origin/main", "--no-edit"]),
            ]
        );
    }

    #[test]
    fn sync_base_uses_glab_target_branch_for_gitlab_merge_request_url() {
        let (result, calls) = run_scripted_url(
            Some("https://gitlab.com/g/p/-/merge_requests/42"),
            vec![
                Ok(output(0, br#"{"iid":42,"target_branch":"develop"}"#)),
                Ok(output(0, b"")),
                Ok(output(0, b"")),
                Ok(output(0, b"")),
            ],
        );
        result.unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            calls,
            vec![
                call("glab", &["mr", "view", "42", "--output", "json"]),
                call("git", &["check-ref-format", "--branch", "develop"]),
                call("git", &["fetch", "origin", "develop"]),
                call("git", &["merge", "origin/develop", "--no-edit"]),
            ]
        );
    }

    #[test]
    fn sync_base_rejects_glab_json_without_target_branch() {
        let (result, calls) = run_scripted_url(
            Some("https://gitlab.com/g/p/-/merge_requests/42"),
            vec![Ok(output(0, br#"{"iid":42}"#))],
        );
        assert_other(&result);
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn sync_base_merge_failure_includes_stdout_conflict_output() {
        let (result, _calls) = run_scripted(vec![
            Ok(output(0, b"main\n")),
            Ok(output(0, b"")),
            Ok(output(0, b"")),
            Ok(output(1, b"CONFLICT (content): Merge conflict in a.txt\n")),
        ]);
        match result {
            Err(CruiseError::Other(message)) => assert!(message.contains("CONFLICT"), "{message}"),
            other => panic!("expected CruiseError::Other, got {other:?}"),
        }
    }

    #[test]
    fn sync_base_rejects_empty_base_ref_without_running_git() {
        let (result, calls) = run_scripted(vec![Ok(output(0, b"\n"))]);
        assert_other(&result);
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn sync_base_rejects_option_like_base_ref_without_fetch() {
        let (result, calls) = run_scripted(vec![
            Ok(output(0, b"--upload-pack=evil\n")),
            Ok(output(1, b"")),
        ]);
        assert_other(&result);
        assert!(
            calls
                .iter()
                .all(|(_, args)| !args.contains(&"fetch".to_string())),
            "must not fetch an option-like ref: {calls:?}"
        );
    }

    #[test]
    fn sync_base_rejects_ref_failing_check_ref_format() {
        let (result, calls) = run_scripted(vec![Ok(output(0, b"bad..ref\n")), Ok(output(1, b""))]);
        assert_other(&result);
        assert_eq!(calls.len(), 2);
    }

    #[test]
    fn sync_base_gh_spawn_failure_is_other_and_stops() {
        let (result, calls) = run_scripted(vec![Err(std::io::Error::other("no gh"))]);
        assert_other(&result);
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn sync_base_gh_nonzero_is_other_and_stops() {
        let (result, calls) = run_scripted(vec![Ok(output(1, b""))]);
        assert_other(&result);
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn sync_base_gh_invalid_utf8_is_other_and_stops() {
        let (result, calls) = run_scripted(vec![Ok(output(0, &[0xff, 0xfe, b'\n']))]);
        assert_other(&result);
        assert_eq!(calls.len(), 1);
    }

    #[test]
    fn sync_base_fetch_failure_skips_merge() {
        let (result, calls) = run_scripted(vec![
            Ok(output(0, b"main\n")),
            Ok(output(0, b"")),
            Ok(output(1, b"")),
        ]);
        assert_other(&result);
        assert_eq!(calls.len(), 3);
    }

    #[test]
    fn sync_base_merge_failure_is_other() {
        let (result, calls) = run_scripted(vec![
            Ok(output(0, b"main\n")),
            Ok(output(0, b"")),
            Ok(output(0, b"")),
            Ok(output(1, b"")),
        ]);
        assert_other(&result);
        assert_eq!(calls.len(), 4);
    }

    #[test]
    fn test_extract_last_path_segment_gitlab_mr_url_variants() {
        assert_eq!(
            extract_last_path_segment("https://host/group/subgroup/project/-/merge_requests/42"),
            Some("42".to_string())
        );
        assert_eq!(
            extract_last_path_segment(
                "https://host/group/subgroup/project/-/merge_requests/42?x=1#note_3"
            ),
            Some("42".to_string())
        );
    }

    #[test]
    fn test_extract_mr_url_finds_url_among_output_lines() {
        let out = b"Creating draft merge request\n\nhttps://gitlab.com/g/p/-/merge_requests/5\n";
        assert_eq!(
            extract_mr_url(out),
            Some("https://gitlab.com/g/p/-/merge_requests/5".to_string())
        );
        assert_eq!(extract_mr_url(b"  \n"), None);
    }

    // --- ensure_gh_available ------------------------------------------------

    /// Given: fake `gh` that responds to --version with exit 0
    /// When: `ensure_gh_available` is called
    /// Then: returns Ok
    #[cfg(unix)]
    #[test]
    fn test_ensure_gh_available_succeeds_when_gh_responds_to_version() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap_or_else(|e| panic!("{e:?}"));
        crate::test_support::install_version_only_gh(&bin_dir);

        let _lock = crate::test_support::lock_process();
        let _path_guard = crate::test_support::prepend_to_path(&bin_dir);

        // When
        let result = ensure_gh_available();

        // Then
        assert!(
            result.is_ok(),
            "expected Ok when gh responds to --version: {result:?}"
        );
    }

    /// Given: no `gh` binary in PATH (empty directory only)
    /// When: `ensure_gh_available` is called
    /// Then: returns Err with a message that mentions "gh"
    #[cfg(unix)]
    #[test]
    fn test_ensure_gh_available_fails_when_gh_not_in_path() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let empty_bin = tmp.path().join("empty_bin");
        std::fs::create_dir_all(&empty_bin).unwrap_or_else(|e| panic!("{e:?}"));
        crate::test_support::run_isolated_test_with_path(
            "worktree_pr::tests::test_ensure_gh_available_missing_path_child_probe",
            &empty_bin,
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "child process probe"]
    fn test_ensure_gh_available_missing_path_child_probe() {
        let result = ensure_gh_available();
        let Err(result_err) = result else {
            panic!("expected Err when gh is absent");
        };
        let err = result_err.to_string();
        assert!(
            err.to_lowercase().contains("gh"),
            "error should mention gh: {err}"
        );
        crate::test_support::report_isolated_test_probe();
    }

    /// Given: a `gh` binary that exits non-zero for --version (broken install)
    /// When: `ensure_gh_available` is called
    /// Then: returns Err with a message that mentions "gh"
    #[cfg(unix)]
    #[test]
    fn test_ensure_gh_available_fails_when_gh_exits_nonzero() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap_or_else(|e| panic!("{e:?}"));

        // Install a gh that always exits 1 (broken install simulation)
        {
            use std::fs;

            let script_path = bin_dir.join("gh");
            fs::write(&script_path, "#!/bin/sh\nexit 1\n").unwrap_or_else(|e| panic!("{e:?}"));
            let mut perms = fs::metadata(&script_path)
                .unwrap_or_else(|e| panic!("{e:?}"))
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script_path, perms).unwrap_or_else(|e| panic!("{e:?}"));
        }
        crate::test_support::run_isolated_test_with_path(
            "worktree_pr::tests::test_ensure_gh_available_nonzero_child_probe",
            &bin_dir,
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "child process probe"]
    fn test_ensure_gh_available_nonzero_child_probe() {
        let result = ensure_gh_available();
        let Err(result_err) = result else {
            panic!("expected Err when gh exits non-zero");
        };
        let err = result_err.to_string();
        assert!(
            err.to_lowercase().contains("gh"),
            "error should mention gh: {err}"
        );
        crate::test_support::report_isolated_test_probe();
    }

    // -- generate_pr_description cancellation ---------------------------------

    fn minimal_compiled_workflow(command: Vec<String>) -> CompiledWorkflow {
        use indexmap::IndexMap;
        use std::collections::HashMap;
        CompiledWorkflow {
            command,
            sdk: None,
            model: None,
            plan_model: None,
            subagent_model: None,
            env: HashMap::new(),
            plan_language: "English".to_string(),
            pr_language: "English".to_string(),
            mcp_servers: IndexMap::new(),
            create_pr: true,
            cleanup_after_pr: false,
            computer_use: false,
            permission: crate::config::PermissionMode::Full,
            steps: IndexMap::new(),
            after_pr: IndexMap::new(),
            invocations: HashMap::new(),
            after_pr_invocations: HashMap::new(),
            step_to_invocation: HashMap::new(),
            after_pr_step_to_invocation: HashMap::new(),
        }
    }

    /// Given: a command-backend `CompiledWorkflow` with a blocking command, a pre-cancelled token
    /// When: `generate_pr_description` is called
    /// Then: returns `Err(Interrupted)` before the 5-second timeout
    ///
    /// If this test times out, the `cancel_token` is not forwarded to `PromptRun`
    /// inside `generate_pr_description`.
    #[cfg(unix)]
    #[tokio::test]
    async fn generate_pr_description_pre_cancelled_token_returns_interrupted() {
        use crate::cancellation::CancellationToken;
        use crate::error::CruiseError;

        let _guard = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "# Plan\n- step 1").unwrap_or_else(|e| panic!("{e:?}"));

        // Given: blocking command and pre-cancelled token
        let compiled = minimal_compiled_workflow(vec!["sleep".to_string(), "100".to_string()]);
        let mut vars = VariableStore::new("implement feature X".to_string());
        vars.set_named_file(crate::session::PLAN_VAR, plan_path);
        let token = CancellationToken::new();
        token.cancel();

        // When
        let timed = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            generate_pr_description(tmp.path(), &compiled, &mut vars, 0, Some(&token), None),
        )
        .await;

        // Then: completes before timeout and propagates Interrupted
        assert!(
            timed.is_ok(),
            "timed out — cancel_token is not forwarded to PromptRun in generate_pr_description"
        );
        let result = timed.unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            matches!(result, Err(CruiseError::Interrupted)),
            "expected Interrupted on cancellation, got {result:?}"
        );
    }

    /// Given: no cancel token and a fast command (cat)
    /// When: `generate_pr_description` is called
    /// Then: completes without hanging — baseline regression that None is an accepted value
    #[cfg(unix)]
    #[tokio::test]
    async fn generate_pr_description_no_token_completes_with_cat() {
        let _guard = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "# Plan\n- step 1").unwrap_or_else(|e| panic!("{e:?}"));

        let compiled = minimal_compiled_workflow(vec!["cat".to_string()]);
        let mut vars = VariableStore::new("implement feature X".to_string());
        vars.set_named_file(crate::session::PLAN_VAR, plan_path);

        // cat echoes the prompt; the function should complete without panicking.
        let _ = generate_pr_description(tmp.path(), &compiled, &mut vars, 0, None, None).await;
        // No assertion on values — only that the function accepts None and returns without hanging.
    }

    /// Given: a command backend and a temporary worktree path
    /// When: PR metadata generation runs
    /// Then: the command executes from that worktree rather than the process cwd
    #[cfg(unix)]
    #[tokio::test]
    async fn generate_pr_description_runs_in_worktree_directory() {
        let _guard = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = tmp.path().join("plan.md");
        std::fs::write(&plan_path, "# Plan\n- step 1").unwrap_or_else(|e| panic!("{e:?}"));
        let cwd_path = tmp.path().join("cwd.txt");
        let compiled = minimal_compiled_workflow(vec![
            "sh".to_string(),
            "-c".to_string(),
            "pwd > \"$1\"; cat".to_string(),
            "_".to_string(),
            cwd_path.to_string_lossy().into_owned(),
        ]);
        let mut vars = VariableStore::new("implement feature X".to_string());
        vars.set_named_file(crate::session::PLAN_VAR, plan_path);

        let _ = generate_pr_description(tmp.path(), &compiled, &mut vars, 0, None, None).await;

        let expected = std::fs::canonicalize(tmp.path()).unwrap_or_else(|e| panic!("{e:?}"));
        let actual_path = std::fs::read_to_string(&cwd_path)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .trim()
            .to_string();
        let actual = std::fs::canonicalize(actual_path).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(actual, expected);
    }

    /// Given: the SDK PR-metadata helper with a deterministic command executor
    /// When: it runs against a temporary worktree
    /// Then: its `PromptRun` uses that worktree as the command/agent directory
    #[cfg(unix)]
    #[tokio::test]
    async fn generate_pr_via_sdk_tool_runs_in_worktree_directory() {
        let _guard = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cwd_path = tmp.path().join("sdk-cwd.txt");
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            "pwd > \"$1\"; cat".to_string(),
            "_".to_string(),
            cwd_path.to_string_lossy().into_owned(),
        ];
        let executor = crate::executor::Executor::new(None, &command);
        let env = std::collections::HashMap::new();
        let mcp_servers = crate::config::McpServers::new();
        let spinner = crate::spinner::Spinner::start("test");

        let _ = generate_pr_via_sdk_tool(
            &executor,
            tmp.path(),
            "test prompt",
            None,
            0,
            &env,
            &mcp_servers,
            &spinner,
            None,
            None,
        )
        .await;
        drop(spinner);

        let expected = std::fs::canonicalize(tmp.path()).unwrap_or_else(|e| panic!("{e:?}"));
        let actual_path = std::fs::read_to_string(&cwd_path)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .trim()
            .to_string();
        let actual = std::fs::canonicalize(actual_path).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(actual, expected);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn clean_worktree_without_branch_commits_completes_without_pr_processing() {
        let _lock = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        crate::test_support::init_git_repo(tmp.path());
        let plan_temp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let plan_path = plan_temp.path().join("plan.md");
        std::fs::write(&plan_path, "# Plan\n- no code changes").unwrap_or_else(|e| panic!("{e:?}"));

        let mut compiled = minimal_compiled_workflow(vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf description".to_string(),
        ]);
        compiled.model = Some(crate::config::ModelSpec::Single(
            "provider/pr-description-model:free:xhigh".to_string(),
        ));
        let mut vars = VariableStore::new("PR description logging task".to_string());
        vars.set_named_file(crate::session::PLAN_VAR, plan_path);
        let ctx = worktree::WorktreeContext {
            path: tmp.path().to_path_buf(),
            branch: "main".to_string(),
            original_dir: tmp.path().to_path_buf(),
        };
        let mut tracker = FileTracker::with_root(tmp.path().to_path_buf());
        let mut session = SessionState::new(
            "20260921070001".to_string(),
            tmp.path().to_path_buf(),
            crate::session_config::SessionConfigRef::File {
                path: tmp.path().join("test.yaml"),
            },
            "PR description logging task".to_string(),
        );
        let bin_dir = plan_temp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let gh_log = plan_temp.path().join("gh.log");
        let gh = bin_dir.join("gh");
        std::fs::write(
            &gh,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\nexit 1\n",
                gh_log.display()
            ),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let mut permissions = std::fs::metadata(&gh)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&gh, permissions).unwrap_or_else(|e| panic!("{e:?}"));
        let _path = crate::test_support::prepend_to_path(&bin_dir);

        let logs = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let logs_sink = std::sync::Arc::clone(&logs);
        let on_step_log = move |stream: &str, line: &str| {
            logs_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("{stream}:{line}"));
        };
        let processing_calls = std::sync::atomic::AtomicUsize::new(0);
        let on_pr_processing = || {
            processing_calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        };
        let result = handle_worktree_pr_with_persistence(
            &ctx,
            &compiled,
            &mut vars,
            &mut tracker,
            &mut session,
            0,
            0,
            &[],
            &crate::flags::NO_FLAGS,
            None,
            &crate::option_handler::NoOpOptionHandler,
            Some(&on_step_log),
            Some(&on_pr_processing),
            None,
        )
        .await;

        assert!(result.is_ok(), "clean worktree should complete: {result:?}");
        assert!(session.pr_url.is_none());
        assert_eq!(
            session.pr_skipped_reason,
            Some(crate::session::PrSkippedReason::NoCommits)
        );
        assert_eq!(
            processing_calls.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "a clean branch should not enter PR processing"
        );
        assert!(
            logs.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        assert!(
            !gh_log.exists(),
            "clean branch should not call the forge CLI"
        );
    }

    #[tokio::test]
    async fn native_merge_opt_in_makes_prior_after_pr_errors_fatal_only_when_enabled() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let mut compiled = minimal_compiled_workflow(Vec::new());
        compiled.after_pr.insert(
            "before-merge".to_string(),
            crate::config::StepConfig {
                command: Some(crate::config::StringOrVec::Single("true".to_string())),
                timeout: Some("invalid-timeout".to_string()),
                ..Default::default()
            },
        );
        compiled.after_pr.insert(
            "squash-merge".to_string(),
            crate::config::StepConfig {
                flag: Some(crate::config::StringOrVec::Single("merge".to_string())),
                squash_merge: Some(true),
                ..Default::default()
            },
        );

        let mut vars = VariableStore::new("test".to_string());
        let mut tracker = FileTracker::with_root(tmp.path().to_path_buf());
        let no_merge = crate::flags::FlagValues::new();
        let warned = run_after_pr_steps(
            &compiled,
            &mut vars,
            &mut tracker,
            10,
            0,
            tmp.path(),
            &[],
            &no_merge,
            None,
            &crate::option_handler::NoOpOptionHandler,
            None,
        )
        .await;
        assert!(
            warned.is_ok(),
            "legacy after-pr failures should remain warning-only without merge opt-in: {warned:?}"
        );

        let merge_enabled = [("merge".to_string(), true)].into();
        let mut vars = VariableStore::new("test".to_string());
        let mut tracker = FileTracker::with_root(tmp.path().to_path_buf());
        let fatal = run_after_pr_steps(
            &compiled,
            &mut vars,
            &mut tracker,
            10,
            0,
            tmp.path(),
            &[],
            &merge_enabled,
            None,
            &crate::option_handler::NoOpOptionHandler,
            None,
        )
        .await;
        assert!(
            matches!(fatal, Err(CruiseError::InvalidStepConfig(_))),
            "native merge opt-in must propagate preceding after-pr errors: {fatal:?}"
        );
    }

    #[tokio::test]
    async fn native_merge_requires_a_pr_context_before_calling_the_backend() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let mut compiled = minimal_compiled_workflow(Vec::new());
        compiled.after_pr.insert(
            "squash-merge".to_string(),
            crate::config::StepConfig {
                flag: Some(crate::config::StringOrVec::Single("merge".to_string())),
                squash_merge: Some(true),
                ..Default::default()
            },
        );
        let mut vars = VariableStore::new("test".to_string());
        let mut tracker = FileTracker::with_root(tmp.path().to_path_buf());
        let merge_enabled = [("merge".to_string(), true)].into();

        let result = run_after_pr_steps(
            &compiled,
            &mut vars,
            &mut tracker,
            10,
            0,
            tmp.path(),
            &[],
            &merge_enabled,
            None,
            &crate::option_handler::NoOpOptionHandler,
            None,
        )
        .await;

        assert!(
            matches!(result, Err(CruiseError::SquashMergeFailed(_))),
            "merge must fail closed when the PR context is missing: {result:?}"
        );

        let invalid_cwd = tmp.path().join("not-a-worktree-directory");
        std::fs::write(&invalid_cwd, "not a directory").unwrap_or_else(|e| panic!("{e:?}"));
        let mut vars = VariableStore::new("test".to_string());
        vars.set_named_value(
            PR_URL_VAR,
            "https://github.com/owner/repo/pull/42".to_string(),
        );
        let mut tracker = FileTracker::with_root(tmp.path().to_path_buf());
        let result = run_after_pr_steps(
            &compiled,
            &mut vars,
            &mut tracker,
            10,
            0,
            &invalid_cwd,
            &[],
            &merge_enabled,
            None,
            &crate::option_handler::NoOpOptionHandler,
            None,
        )
        .await;
        assert!(
            matches!(result, Err(CruiseError::SquashMergeFailed(_))),
            "merge must fail closed unless execution runs in a worktree directory: {result:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_merge_without_flag_does_not_call_the_forge_cli() {
        let _lock = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        crate::test_support::init_git_repo(tmp.path());
        let bin_dir = tmp.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let gh_log = tmp.path().join("gh.log");
        let gh = bin_dir.join("gh");
        std::fs::write(
            &gh,
            format!("#!/bin/sh\nprintf called >> '{}'\n", gh_log.display()),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let mut permissions = std::fs::metadata(&gh)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&gh, permissions).unwrap_or_else(|e| panic!("{e:?}"));
        let _path = crate::test_support::prepend_to_path(&bin_dir);

        let mut compiled = minimal_compiled_workflow(Vec::new());
        compiled.after_pr.insert(
            "squash-merge".to_string(),
            crate::config::StepConfig {
                flag: Some(crate::config::StringOrVec::Single("merge".to_string())),
                squash_merge: Some(true),
                ..Default::default()
            },
        );
        let mut vars = VariableStore::new("test".to_string());
        vars.set_named_value(
            PR_URL_VAR,
            "https://github.com/owner/repo/pull/42".to_string(),
        );
        let mut tracker = FileTracker::with_root(tmp.path().to_path_buf());

        let result = run_after_pr_steps(
            &compiled,
            &mut vars,
            &mut tracker,
            10,
            0,
            tmp.path(),
            &[],
            &crate::flags::NO_FLAGS,
            None,
            &crate::option_handler::NoOpOptionHandler,
            None,
        )
        .await;

        assert!(
            result.is_ok(),
            "merge-off after-pr action should skip: {result:?}"
        );
        assert!(
            !gh_log.exists(),
            "native merge must not contact the forge without merge opt-in"
        );
    }

    #[cfg(unix)]
    #[expect(
        clippy::too_many_lines,
        reason = "This test exercises the complete PR metadata abort path."
    )]
    #[tokio::test]
    async fn missing_jcode_model_for_pr_metadata_stops_before_pr_creation() {
        let _process = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let mut guards = crate::test_support::set_fake_home(tmp.path());
        #[cfg(unix)]
        {
            let bin = tmp.path().join("bin");
            guards.push(crate::test_support::install_exiting_jcode_stub(&bin));
            let gh_log = tmp.path().join("gh-invocations.log");
            guards.push(crate::test_support::EnvGuard::set(
                "CRUISE_TEST_GH_LOG",
                &gh_log,
            ));
            crate::test_support::write_executable_script(
                &bin.join("gh"),
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nprintf '%s\\n' \"$*\" >> \"$CRUISE_TEST_GH_LOG\"\nexit 1\n",
            );
        }
        guards.push(crate::test_support::EnvGuard::set(
            "JCODE_HOME",
            tmp.path().join("jcode-home"),
        ));
        for key in ["JCODE_MODEL", "CRUISE_MODEL", "CRUISE_PLAN_MODEL"] {
            guards.push(crate::test_support::EnvGuard::remove(key));
        }
        let original_dir = tmp.path().join("original");
        let worktree_path = tmp.path().join("worktree");
        std::fs::create_dir_all(&original_dir).unwrap_or_else(|error| panic!("{error}"));
        crate::test_support::init_git_repo(&original_dir);
        crate::test_support::run_git_ok(
            tmp.path(),
            &[
                "clone",
                original_dir
                    .to_str()
                    .unwrap_or_else(|| panic!("non-UTF8 test path")),
                worktree_path
                    .to_str()
                    .unwrap_or_else(|| panic!("non-UTF8 test path")),
            ],
        );
        crate::test_support::run_git_ok(
            &worktree_path,
            &["config", "user.email", "test@example.com"],
        );
        crate::test_support::run_git_ok(&worktree_path, &["config", "user.name", "Test"]);
        crate::test_support::run_git_ok(&worktree_path, &["checkout", "-b", "cruise/test"]);
        let plan_path = worktree_path.join("plan.md");
        std::fs::write(&plan_path, "# Plan\n- implement the change")
            .unwrap_or_else(|error| panic!("{error}"));
        crate::test_support::run_git_ok(&worktree_path, &["add", "plan.md"]);
        crate::test_support::run_git_ok(&worktree_path, &["commit", "-m", "worktree change"]);
        let mut compiled = minimal_compiled_workflow(Vec::new());
        compiled.sdk = Some("jcode".to_string());
        compiled.plan_model = Some(crate::config::ModelSpec::Single(
            "provider/planning-model".to_string(),
        ));
        let mut vars = VariableStore::new("PR metadata task".to_string());
        vars.set_named_file(crate::session::PLAN_VAR, plan_path);
        let ctx = worktree::WorktreeContext {
            path: worktree_path,
            branch: "cruise/test".to_string(),
            original_dir,
        };
        let mut tracker = FileTracker::with_root(ctx.path.clone());
        let mut session = SessionState::new(
            "pr-missing-model".to_string(),
            ctx.path.clone(),
            crate::session_config::SessionConfigRef::inline_snapshot(),
            "PR metadata task".to_string(),
        );
        let pr_created = std::sync::atomic::AtomicBool::new(false);
        let on_pr_created = |_: &SessionState| {
            pr_created.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        };

        let result = handle_worktree_pr_with_persistence(
            &ctx,
            &compiled,
            &mut vars,
            &mut tracker,
            &mut session,
            0,
            0,
            &[],
            &crate::flags::NO_FLAGS,
            None,
            &crate::option_handler::NoOpOptionHandler,
            None,
            None,
            Some(&on_pr_created),
        )
        .await;

        assert!(
            matches!(result, Err(CruiseError::MissingJcodeModel(_))),
            "SDK metadata generation must preserve the missing-model error: {result:?}"
        );
        assert!(
            !pr_created.load(std::sync::atomic::Ordering::SeqCst),
            "PR creation must not be reached after metadata model validation fails"
        );
        #[cfg(unix)]
        assert!(
            !tmp.path().join("gh-invocations.log").exists(),
            "PR metadata model validation must stop before invoking the gh create command"
        );
        assert!(tmp.path().join(".config/cruise/default.yaml").is_file());
    }

    #[tokio::test]
    async fn missing_jcode_model_in_after_pr_steps_is_not_best_effort() {
        let _process = crate::test_support::lock_process();
        let tmp = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
        let mut guards = crate::test_support::set_fake_home(tmp.path());
        #[cfg(unix)]
        {
            let bin = tmp.path().join("bin");
            guards.push(crate::test_support::install_exiting_jcode_stub(&bin));
        }
        guards.push(crate::test_support::EnvGuard::set(
            "JCODE_HOME",
            tmp.path().join("jcode-home"),
        ));
        for key in ["JCODE_MODEL", "CRUISE_MODEL", "CRUISE_PLAN_MODEL"] {
            guards.push(crate::test_support::EnvGuard::remove(key));
        }
        let config = crate::config::WorkflowConfig::from_yaml(
            "sdk: jcode\nsteps:\n  main:\n    command: echo done\nafter-pr:\n  final-check:\n    prompt: verify the change\n",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let compiled = crate::workflow::compile(config).unwrap_or_else(|error| panic!("{error}"));
        let mut vars = VariableStore::new("after-pr task".to_string());
        let mut tracker = FileTracker::with_root(tmp.path().to_path_buf());

        let result = run_after_pr_steps_for_test(
            &compiled,
            &mut vars,
            &mut tracker,
            tmp.path(),
            &crate::option_handler::NoOpOptionHandler,
        )
        .await;

        assert!(
            matches!(result, Err(CruiseError::MissingJcodeModel(_))),
            "after-pr must propagate a missing-model error instead of warning and succeeding: {result:?}"
        );
        assert!(tmp.path().join(".config/cruise/default.yaml").is_file());
    }
}
