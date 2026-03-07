use std::cell::RefCell;
use std::path::Path;

use console::style;
use inquire::InquireError;

use crate::cli::RunArgs;
use crate::config::{WorkflowConfig, validate_groups};
use crate::engine::{execute_steps, print_dry_run, resolve_command_with_model};
use crate::error::{CruiseError, Result};
use crate::file_tracker::FileTracker;
use crate::session::{SessionManager, SessionPhase, current_iso8601, get_cruise_home};
use crate::variable::VariableStore;
use crate::worktree;

/// Variable name that maps to the plan file.
const PLAN_VAR: &str = "plan";
const PR_NUMBER_VAR: &str = "pr.number";
const PR_URL_VAR: &str = "pr.url";
const CREATE_PR_PROMPT_TEMPLATE: &str = include_str!("../prompts/create-pr.md");

pub async fn run(args: RunArgs) -> Result<()> {
    let manager = SessionManager::new(get_cruise_home()?);

    // Determine which session to run.
    let session_id = match args.session {
        Some(id) => id,
        None => select_pending_session(&manager)?,
    };

    let mut session = manager.load(&session_id)?;

    // Load config from session dir.
    let config_path = manager.sessions_dir().join(&session_id).join("config.yaml");
    let yaml = std::fs::read_to_string(&config_path).map_err(|e| {
        CruiseError::Other(format!(
            "failed to read session config {}: {}",
            config_path.display(),
            e
        ))
    })?;
    let config = WorkflowConfig::from_yaml(&yaml)
        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
    validate_groups(&config)?;

    if args.dry_run {
        eprintln!("{}", style(format!("Session: {}", session_id)).dim());
        return print_dry_run(&config, session.current_step.as_deref());
    }

    ensure_gh_available()?;

    // Determine start step.
    let start_step = session.current_step.clone().map(Ok).unwrap_or_else(|| {
        config
            .steps
            .keys()
            .next()
            .ok_or_else(|| CruiseError::Other("config has no steps".to_string()))
            .cloned()
    })?;

    // Show resume message if restarting an interrupted session.
    if let Some(ref step) = session.current_step {
        match &session.phase {
            SessionPhase::Running => {
                eprintln!("{} Resuming from step: {}", style("↺").cyan(), step);
            }
            SessionPhase::Failed(_) => {
                eprintln!(
                    "{} Retrying from failed step: {}",
                    style("↺").yellow(),
                    step
                );
            }
            _ => {}
        }
    }

    // chdir to base_dir.
    let base_dir = session.base_dir.clone();
    std::env::set_current_dir(&base_dir)?;

    // Create or reuse worktree at ~/.cruise/worktrees/{session_id}/.
    let worktrees_dir = manager.worktrees_dir();
    let (ctx, reused) = worktree::setup_session_worktree(
        &base_dir,
        &session_id,
        &session.input,
        &worktrees_dir,
        session.worktree_branch.as_deref(),
    )?;
    let suffix = if reused { " (reused)" } else { "" };
    eprintln!(
        "{} worktree: {}{}",
        style("→").cyan(),
        ctx.path.display(),
        suffix
    );

    session.worktree_path = Some(ctx.path.clone());
    session.worktree_branch = Some(ctx.branch.clone());
    session.phase = SessionPhase::Running;
    manager.save(&session)?;

    // chdir to worktree.
    std::env::set_current_dir(&ctx.path)?;

    // Set up variables.
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path);

    let mut tracker = FileTracker::with_root(ctx.path.clone());

    // Use RefCell for interior mutability in the callback.
    let session_cell = RefCell::new(&mut session);

    let exec_result = execute_steps(
        &config,
        &mut vars,
        &mut tracker,
        &start_step,
        args.max_retries,
        args.rate_limit_retries,
        &|step| {
            let mut s = session_cell.borrow_mut();
            s.current_step = Some(step.to_string());
            manager.save(&s)
        },
    )
    .await;

    let session = session_cell.into_inner();

    // Update session phase based on result.
    match &exec_result {
        Ok(_) => {
            // Commit all changes before creating PR.
            match commit_changes(&ctx.path, &session.input) {
                Ok(true) => {
                    eprintln!("{} Changes committed", style("✓").green().bold());
                }
                Ok(false) => {}
                Err(e) => {
                    eprintln!("warning: commit failed: {}", e);
                }
            }

            // Generate PR title and body via LLM using the create-pr prompt template.
            let (pr_title, pr_body) = match vars.resolve(CREATE_PR_PROMPT_TEMPLATE) {
                Err(e) => {
                    eprintln!("warning: PR prompt resolution failed: {}", e);
                    (String::new(), String::new())
                }
                Ok(pr_prompt) => {
                    let pr_model = config.model.as_deref();
                    let has_placeholder = config.command.iter().any(|s| s.contains("{model}"));
                    let (resolved_command, model_arg) = if has_placeholder {
                        (resolve_command_with_model(&config.command, pr_model), None)
                    } else {
                        (config.command.clone(), pr_model.map(str::to_string))
                    };
                    let spinner = crate::spinner::Spinner::start("Generating PR description...");
                    let env = std::collections::HashMap::new();
                    let llm_output = {
                        let on_retry = |msg: &str| spinner.suspend(|| eprintln!("{}", msg));
                        match crate::step::prompt::run_prompt(
                            &resolved_command,
                            model_arg.as_deref(),
                            &pr_prompt,
                            args.rate_limit_retries,
                            &env,
                            Some(&on_retry),
                        )
                        .await
                        {
                            Ok(r) => r.output,
                            Err(e) => {
                                eprintln!("warning: PR description generation failed: {}", e);
                                String::new()
                            }
                        }
                    };
                    drop(spinner);
                    parse_pr_metadata(&llm_output)
                }
            };

            // Try to create a PR automatically.
            match create_pr(&ctx.path, &ctx.branch, &pr_title, &pr_body) {
                Ok(url) => {
                    eprintln!("{} PR created: {}", style("✓").green().bold(), url);
                    if let Some(number) = extract_last_path_segment(&url) {
                        vars.set_named_value(PR_NUMBER_VAR, number);
                    }
                    vars.set_named_value(PR_URL_VAR, url.clone());
                    session.pr_url = Some(url);

                    // Run after_pr steps if any.
                    if let Some(first_step) = config.after_pr.keys().next() {
                        let mut after_config = config.clone();
                        after_config.steps = std::mem::take(&mut after_config.after_pr);
                        if let Err(e) = execute_steps(
                            &after_config,
                            &mut vars,
                            &mut tracker,
                            first_step,
                            args.max_retries,
                            args.rate_limit_retries,
                            &|_| Ok(()),
                        )
                        .await
                        {
                            eprintln!("warning: after-pr steps failed: {}", e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("warning: PR creation failed: {}", e);
                }
            }
            session.phase = SessionPhase::Completed;
            session.completed_at = Some(current_iso8601());
        }
        Err(e) => {
            session.phase = SessionPhase::Failed(e.to_string());
            session.completed_at = Some(current_iso8601());
        }
    }
    manager.save(session)?;

    exec_result.map(|_| ())
}

/// Stage all changes and commit them. Returns `true` if a commit was created,
/// `false` if there was nothing to commit.
fn commit_changes(worktree_path: &Path, message: &str) -> Result<bool> {
    // git add -A
    let add = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git add: {}", e)))?;
    if !add.status.success() {
        let stderr = String::from_utf8_lossy(&add.stderr);
        return Err(CruiseError::Other(format!(
            "git add -A failed: {}",
            stderr.trim()
        )));
    }

    // Check if there are staged changes
    let diff = std::process::Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git diff: {}", e)))?;
    if diff.status.success() {
        // No changes to commit
        return Ok(false);
    }

    // git commit
    let commit = std::process::Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run git commit: {}", e)))?;
    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr);
        return Err(CruiseError::Other(format!(
            "git commit failed: {}",
            stderr.trim()
        )));
    }

    Ok(true)
}

/// Create a PR using `gh pr create`. Uses `--title`/`--body` if provided, otherwise `--fill`.
/// Falls back to `gh pr view` if a PR already exists.
fn create_pr(worktree_path: &Path, branch: &str, title: &str, body: &str) -> Result<String> {
    let mut gh_args = vec!["pr", "create", "--head", branch];
    if !title.is_empty() {
        gh_args.extend(["--title", title, "--body", body]);
    } else {
        gh_args.push("--fill");
    }
    let output = std::process::Command::new("gh")
        .args(&gh_args)
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh pr create: {}", e)))?;

    if output.status.success()
        && let Some(url) = gh_output_line(&output.stdout)
    {
        return Ok(url);
    }

    // PR may already exist — try to fetch the URL.
    let fallback = std::process::Command::new("gh")
        .args(["pr", "view", branch, "--json", "url", "--jq", ".url"])
        .current_dir(worktree_path)
        .output()
        .map_err(|e| CruiseError::Other(format!("failed to run gh pr view: {}", e)))?;

    if fallback.status.success()
        && let Some(url) = gh_output_line(&fallback.stdout)
    {
        return Ok(url);
    }

    let create_stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let view_stderr = String::from_utf8_lossy(&fallback.stderr).trim().to_string();
    Err(CruiseError::Other(format!(
        "gh pr create failed: {}; gh pr view also failed: {}",
        create_stderr, view_stderr
    )))
}

/// Trim and return a non-empty line from `gh` stdout bytes, or `None`.
fn gh_output_line(bytes: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(bytes).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Extracts the last path segment from a URL, stripping any query string or fragment.
/// Returns `None` if the URL has no non-empty trailing path segment.
fn extract_last_path_segment(url: &str) -> Option<String> {
    url.rsplit('/')
        .next()
        .map(|s| s.split_once(['?', '#']).map_or(s, |(prefix, _)| prefix))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Verify that `gh` CLI is available in PATH.
fn ensure_gh_available() -> Result<()> {
    let ok = std::process::Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if ok {
        Ok(())
    } else {
        Err(CruiseError::Other(
            "gh CLI is not installed. Install it from https://cli.github.com/".to_string(),
        ))
    }
}

/// Select a pending session interactively (or automatically if only one).
fn select_pending_session(manager: &SessionManager) -> Result<String> {
    let pending = manager.pending()?;

    if pending.is_empty() {
        return Err(CruiseError::Other(
            "No pending sessions. Run `cruise plan` first.".to_string(),
        ));
    }

    if pending.len() == 1 {
        let s = &pending[0];
        eprintln!(
            "{} Selected session: {} — {}",
            style("→").cyan(),
            s.id,
            crate::display::truncate(&s.input, 60)
        );
        return Ok(s.id.clone());
    }

    // Multiple pending sessions: let the user choose.
    let labels: Vec<String> = pending
        .iter()
        .map(|s| {
            format!(
                "{} | {} | {}",
                s.id,
                s.phase.label(),
                crate::display::truncate(&s.input, 60)
            )
        })
        .collect();
    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();

    let selected = match inquire::Select::new("Select a session to run:", label_refs).prompt() {
        Ok(s) => s,
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            return Err(CruiseError::Other(
                "session selection cancelled".to_string(),
            ));
        }
        Err(e) => return Err(CruiseError::Other(format!("selection error: {e}"))),
    };

    let idx = labels.iter().position(|l| l.as_str() == selected).unwrap();
    Ok(pending[idx].id.clone())
}

/// Strip an optional markdown code block wrapper from `s`.
///
/// Handles both ` ```md ` and plain ` ``` ` prefixes. Returns the inner
/// content with trailing newlines removed, or the trimmed input unchanged if
/// no well-formed code block is found.
fn strip_code_block(s: &str) -> &str {
    let trimmed = s.trim();
    let Some(after_backticks) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(newline_pos) = after_backticks.find('\n') else {
        return trimmed;
    };
    let inner = &after_backticks[newline_pos + 1..];
    let Some(close) = inner.rfind("```") else {
        return trimmed;
    };
    inner[..close].trim_end_matches('\n')
}

/// Parse LLM output into (title, body) from frontmatter format:
///
/// ```text
/// ---
/// title: "My PR title"
/// ---
/// PR body here
/// ```
///
/// Returns `(String::new(), String::new())` if parsing fails.
fn parse_pr_metadata(output: &str) -> (String, String) {
    let content = strip_code_block(output);

    // Must start with ---
    if !content.starts_with("---") {
        return (String::new(), String::new());
    }

    // Skip the opening --- line
    let after_open = match content[3..].find('\n') {
        Some(pos) => &content[3 + pos + 1..],
        None => return (String::new(), String::new()),
    };

    // Find closing ---
    let close_pos = match after_open.find("\n---") {
        Some(pos) => pos,
        None => return (String::new(), String::new()),
    };

    let frontmatter = &after_open[..close_pos];
    let after_close = &after_open[close_pos + "\n---".len()..];
    let body = after_close.strip_prefix('\n').unwrap_or(after_close);

    // Find title in frontmatter
    let title = frontmatter.lines().find_map(|line| {
        line.strip_prefix("title:").map(|rest| {
            let rest = rest.trim();
            rest.strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| rest.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                .unwrap_or(rest)
                .to_string()
        })
    });

    match title {
        Some(t) => (t, body.to_string()),
        None => (String::new(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_last_path_segment_github_pr_url() {
        // Given: a standard GitHub PR URL
        let url = "https://github.com/owner/repo/pull/42";
        // When: extracting the last segment
        let result = extract_last_path_segment(url);
        // Then: last segment is returned
        assert_eq!(result, Some("42".to_string()));
    }

    // --- parse_pr_metadata tests ---

    #[test]
    fn test_parse_pr_metadata_standard_frontmatter() {
        // Given: standard frontmatter output from LLM
        let output = r#"---
title: "feat: Add user icon registration feature"
---
## Overview
Enabled users to upload icon images.

## Background
Previously, emojis were used as user icons."#;
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: title and body are extracted correctly
        assert_eq!(title, "feat: Add user icon registration feature");
        assert_eq!(
            body.trim(),
            "## Overview\nEnabled users to upload icon images.\n\n## Background\nPreviously, emojis were used as user icons."
        );
    }

    #[test]
    fn test_parse_pr_metadata_wrapped_in_markdown_code_block() {
        // Given: LLM output wrapped in ```md code block
        let output =
            "```md\n---\ntitle: \"fix: Resolve login bug\"\n---\nFixed the login issue.\n```";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: code block delimiters are stripped and content is parsed
        assert_eq!(title, "fix: Resolve login bug");
        assert_eq!(body.trim(), "Fixed the login issue.");
    }

    #[test]
    fn test_parse_pr_metadata_title_without_quotes() {
        // Given: frontmatter with unquoted title
        let output = "---\ntitle: feat: Add feature without quotes\n---\nBody text here.";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: title is extracted without quotes
        assert_eq!(title, "feat: Add feature without quotes");
        assert_eq!(body.trim(), "Body text here.");
    }

    #[test]
    fn test_parse_pr_metadata_no_frontmatter_returns_empty() {
        // Given: output without frontmatter delimiters
        let output = "This is just a plain text response without frontmatter.";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: both title and body are empty (caller falls back to session.input)
        assert_eq!(title, "");
        assert_eq!(body, "");
    }

    #[test]
    fn test_parse_pr_metadata_missing_title_field_returns_empty() {
        // Given: frontmatter without a title field
        let output = "---\nauthor: someone\n---\nBody without title.";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: both title and body are empty (fallback)
        assert_eq!(title, "");
        assert_eq!(body, "");
    }

    #[test]
    fn test_parse_pr_metadata_empty_body_after_frontmatter() {
        // Given: frontmatter with title but no body
        let output = "---\ntitle: \"chore: Update deps\"\n---\n";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: title is extracted, body is empty string
        assert_eq!(title, "chore: Update deps");
        assert_eq!(body.trim(), "");
    }

    #[test]
    fn test_parse_pr_metadata_only_closing_delimiter_missing_returns_empty() {
        // Given: frontmatter with only opening --- and no closing ---
        let output = "---\ntitle: \"feat: something\"\nBody without closing delimiter.";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: both title and body are empty (malformed frontmatter)
        assert_eq!(title, "");
        assert_eq!(body, "");
    }

    #[test]
    fn test_parse_pr_metadata_wrapped_in_plain_code_block() {
        // Given: LLM output wrapped in plain ``` (no language specifier)
        let output = "```\n---\ntitle: \"docs: Update README\"\n---\nUpdated documentation.\n```";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: code block is stripped and parsed correctly
        assert_eq!(title, "docs: Update README");
        assert_eq!(body.trim(), "Updated documentation.");
    }
}
