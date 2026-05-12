/// Shared worktree PR helper functions used by both the CLI and the GUI.
///
/// Functions in this module are extracted from `run_cmd.rs` so that
/// `src-tauri` (the GUI crate) can call the same PR post-processing logic
/// without duplicating it.
use std::path::{Path, PathBuf};

use console::style;

use crate::engine::{ExecutionContext, execute_steps, resolve_command_with_model};
use crate::error::{CruiseError, Result};
use crate::file_tracker::FileTracker;
use crate::option_handler::CliOptionHandler;
use crate::session::{PLAN_VAR, SessionState};
use crate::variable::VariableStore;
use crate::workflow::CompiledWorkflow;
use crate::worktree;

// --- Constants ----------------------------------------------------------------

const PR_NUMBER_VAR: &str = "pr.number";
const PR_URL_VAR: &str = "pr.url";
const PR_LANGUAGE_VAR: &str = "pr.language";
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
            eprintln!("{} Changes committed", style("v").green().bold());
        }
        CommitOutcome::NoChanges => {
            // sakoku-ignore-next-line
            eprintln!(
                "{} No new changes to commit; using existing branch commits",
                style("->").cyan()
            );
        }
    }
}

// --- Preflight ---------------------------------------------------------------

/// Verify that the `gh` CLI is available in `PATH`.
///
/// Called as a preflight check before starting worktree-mode execution so
/// that users get a clear, actionable error at run-start rather than only
/// at PR-creation time (after the full workflow has already completed).
///
/// # Errors
///
/// Returns [`CruiseError::Other`] if `gh` is not found in `PATH` or exits
/// with a non-zero status.
pub fn ensure_gh_available() -> Result<()> {
    let ok = std::process::Command::new("gh")
        .arg("--version")
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

/// Handle PR creation and after-PR steps for a worktree execution.
///
/// # Errors
///
/// Returns an error if the branch has no commits beyond its base, or if an
/// underlying git or `gh` operation fails.
pub async fn handle_worktree_pr(
    ctx: &worktree::WorktreeContext,
    compiled: &CompiledWorkflow,
    vars: &mut VariableStore,
    tracker: &mut FileTracker,
    session: &mut SessionState,
    rate_limit_retries: usize,
    max_retries: usize,
) -> Result<()> {
    let (pr_title, pr_body) =
        generate_pr_description(compiled, vars, rate_limit_retries, &ctx.path).await;

    let pr_attempt = attempt_pr_creation(ctx, &session.input, &pr_title, &pr_body)?;
    pr_attempt.report();
    match pr_attempt {
        PrAttemptOutcome::Created { url, .. } => {
            eprintln!("{} PR created: {}", style("v").green().bold(), url);
            if let Some(number) = extract_last_path_segment(&url) {
                vars.set_named_value(PR_NUMBER_VAR, number);
            }
            vars.set_named_value(PR_URL_VAR, url.clone());
            session.pr_url = Some(url);
            run_after_pr_steps(
                compiled,
                vars,
                tracker,
                max_retries,
                rate_limit_retries,
                ctx.path.as_path(),
            )
            .await;
            Ok(())
        }
        PrAttemptOutcome::SkippedNoCommits => Err(CruiseError::Other(format!(
            "cannot create PR for {}: branch has no commits beyond its base; make changes and rerun `cruise run`",
            ctx.branch
        ))),
        PrAttemptOutcome::CreateFailed { error, .. } => {
            eprintln!("warning: PR creation failed: {error}");
            Ok(())
        }
    }
}

/// Generate a PR title and body using the LLM, returning empty strings on failure.
async fn generate_pr_description(
    compiled: &CompiledWorkflow,
    vars: &mut VariableStore,
    rate_limit_retries: usize,
    working_dir: &Path,
) -> (String, String) {
    // If LLM API is configured, try the API path first.
    if let Some(ref api_config) = compiled.llm_api
        && let Ok(plan_path_str) = vars.get_variable(PLAN_VAR)
    {
        let plan_path = PathBuf::from(&plan_path_str);
        match crate::llm_api::generate_pr_metadata(
            api_config,
            &plan_path,
            &compiled.pr_language,
            working_dir,
        )
        .await
        {
            Ok((title, body)) => return (title, body),
            Err(e) => {
                eprintln!("warning: LLM API call failed, falling back to CLI: {e}");
            }
        }
    }

    let pr_prompt = match build_pr_prompt(vars, compiled) {
        Err(e) => {
            eprintln!("warning: PR prompt resolution failed: {e}");
            return (String::new(), String::new());
        }
        Ok(p) => p,
    };
    let pr_model = compiled.model.as_deref();
    let has_placeholder = compiled.command.iter().any(|s| s.contains("{model}"));
    let (resolved_command, model_arg) = if has_placeholder {
        (
            resolve_command_with_model(&compiled.command, pr_model),
            None,
        )
    } else {
        (compiled.command.clone(), pr_model.map(str::to_string))
    };
    let spinner = crate::spinner::Spinner::start("Generating PR description...");
    let env = std::collections::HashMap::new();
    let llm_output = {
        let on_retry = |msg: &str| spinner.suspend(|| eprintln!("{msg}"));
        match crate::step::prompt::run_prompt(
            &resolved_command,
            model_arg.as_deref(),
            &pr_prompt,
            rate_limit_retries,
            &env,
            Some(&on_retry),
            None,
            None,
            None,
        )
        .await
        {
            Ok(r) => r.output,
            Err(e) => {
                eprintln!("warning: PR description generation failed: {e}");
                String::new()
            }
        }
    };
    drop(spinner);
    let (pr_title, pr_body) = parse_pr_metadata(&llm_output);
    if pr_title.is_empty() && !llm_output.trim().is_empty() {
        let truncated: String = llm_output.chars().take(500).collect();
        eprintln!(
            "{} Failed to parse PR metadata from LLM output (first 500 chars):\n{}",
            style("!").yellow(),
            truncated
        );
    }
    (pr_title, pr_body)
}

/// Run the after-PR workflow steps, logging any errors.
async fn run_after_pr_steps(
    compiled: &CompiledWorkflow,
    vars: &mut VariableStore,
    tracker: &mut FileTracker,
    max_retries: usize,
    rate_limit_retries: usize,
    working_dir: &std::path::Path,
) {
    let Some(first_step) = compiled.after_pr.keys().next() else {
        return;
    };
    let after_compiled = compiled.to_after_pr_compiled();
    let ctx = ExecutionContext {
        compiled: &after_compiled,
        max_retries,
        rate_limit_retries,
        on_step_start: &|_| Ok(()),
        cancel_token: None,
        option_handler: &CliOptionHandler,
        config_reloader: None,
        working_dir: Some(working_dir),
        skipped_steps: &[],
        on_step_log: None,
    };
    match execute_steps(&ctx, vars, tracker, first_step).await {
        Ok(_) | Err(CruiseError::StepPaused) => {}
        Err(e) => {
            eprintln!("warning: after-pr steps failed: {e}");
        }
    }
}

pub(crate) fn build_pr_prompt(
    vars: &mut VariableStore,
    compiled: &CompiledWorkflow,
) -> Result<String> {
    let lang = compiled.pr_language.trim();
    let lang = if lang.is_empty() {
        crate::config::DEFAULT_PR_LANGUAGE
    } else {
        lang
    };
    vars.set_named_value(PR_LANGUAGE_VAR, lang.to_string());
    vars.resolve(CREATE_PR_PROMPT_TEMPLATE)
}

pub(crate) fn attempt_pr_creation(
    ctx: &worktree::WorktreeContext,
    message: &str,
    title: &str,
    body: &str,
) -> Result<PrAttemptOutcome> {
    let trimmed_title = title.trim();
    let commit_message = if trimmed_title.is_empty() {
        message
    } else {
        trimmed_title
    };
    let commit_outcome = commit_changes(&ctx.path, commit_message)?;
    if branch_commit_count(ctx)? == 0 {
        return Ok(PrAttemptOutcome::SkippedNoCommits);
    }

    push_branch(&ctx.path, &ctx.branch)?;

    match create_pr(&ctx.path, &ctx.branch, trimmed_title, body) {
        Ok(url) => Ok(PrAttemptOutcome::Created {
            url,
            commit_outcome,
        }),
        Err(e) => Ok(PrAttemptOutcome::CreateFailed {
            error: e.to_string(),
            commit_outcome,
        }),
    }
}

fn branch_commit_count(ctx: &worktree::WorktreeContext) -> Result<usize> {
    let base_head = git_stdout(
        &ctx.original_dir,
        &["rev-parse", "HEAD"],
        "git rev-parse HEAD failed",
    )?;
    let merge_base = git_stdout(
        &ctx.path,
        &["merge-base", "HEAD", &base_head],
        "git merge-base failed",
    )?;
    let count = git_stdout(
        &ctx.path,
        &["rev-list", "--count", &format!("{merge_base}..HEAD")],
        "git rev-list --count failed",
    )?;
    count.parse::<usize>().map_err(|e| {
        CruiseError::Other(format!(
            "failed to parse branch commit count from `{count}`: {e}"
        ))
    })
}

fn git_stdout(current_dir: &Path, args: &[&str], context: &str) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(current_dir)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git {}: {}", args.join(" "), e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::Other(format!("{context}: {}", stderr.trim())));
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
fn commit_changes(worktree_path: &Path, message: &str) -> Result<CommitOutcome> {
    let add = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git add: {e}")))?;
    if !add.status.success() {
        let stderr = String::from_utf8_lossy(&add.stderr);
        return Err(CruiseError::Other(format!(
            "git add -A failed: {}",
            stderr.trim()
        )));
    }

    let diff = std::process::Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git diff: {e}")))?;
    if diff.status.success() {
        return Ok(CommitOutcome::NoChanges);
    }

    let commit = std::process::Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git commit: {e}")))?;
    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr);
        return Err(CruiseError::Other(format!(
            "git commit failed: {}",
            stderr.trim()
        )));
    }

    Ok(CommitOutcome::Created)
}

fn push_branch(worktree_path: &Path, branch: &str) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(["push", "-u", "origin", branch])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git push: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CruiseError::Other(format!(
            "git push failed: {}",
            stderr.trim()
        )));
    }
    Ok(())
}

/// Create a draft PR using `gh pr create --draft`. Uses `--title`/`--body` if provided, otherwise
/// `--fill`. Falls back to `gh pr view` if a PR already exists.
fn create_pr(worktree_path: &Path, branch: &str, title: &str, body: &str) -> Result<String> {
    let mut gh_args = vec!["pr", "create", "--head", branch, "--draft"];
    if title.is_empty() {
        gh_args.push("--fill");
    } else {
        gh_args.extend(["--title", title, "--body", body]);
    }
    let output = std::process::Command::new("gh")
        .args(&gh_args)
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh pr create: {e}")))?;

    if output.status.success()
        && let Some(url) = gh_output_line(&output.stdout)
    {
        return Ok(url);
    }

    // PR may already exist -- try to fetch the URL.
    let fallback = std::process::Command::new("gh")
        .args(["pr", "view", branch, "--json", "url", "--jq", ".url"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh pr view: {e}")))?;

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

/// Trim and return a non-empty line from `gh` stdout bytes, or `None`.
fn gh_output_line(bytes: &[u8]) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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

        let _lock = crate::test_support::lock_process();
        let _path_guard = crate::test_support::EnvGuard::set("PATH", empty_bin.as_os_str());

        // When
        let result = ensure_gh_available();

        // Then
        let Err(result_err) = result else {
            panic!("expected Err when gh is absent");
        };
        let err = result_err.to_string();
        assert!(
            err.to_lowercase().contains("gh"),
            "error should mention gh: {err}"
        );
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
            use std::os::unix::fs::PermissionsExt;

            let script_path = bin_dir.join("gh");
            fs::write(&script_path, "#!/bin/sh\nexit 1\n").unwrap_or_else(|e| panic!("{e:?}"));
            let mut perms = fs::metadata(&script_path)
                .unwrap_or_else(|e| panic!("{e:?}"))
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script_path, perms).unwrap_or_else(|e| panic!("{e:?}"));
        }

        let _lock = crate::test_support::lock_process();
        let _path_guard = crate::test_support::EnvGuard::set("PATH", bin_dir.as_os_str());

        // When
        let result = ensure_gh_available();

        // Then
        let Err(result_err) = result else {
            panic!("expected Err when gh exits non-zero");
        };
        let err = result_err.to_string();
        assert!(
            err.to_lowercase().contains("gh"),
            "error should mention gh: {err}"
        );
    }
}
