use std::cell::RefCell;
use std::path::Path;

use console::style;
use inquire::InquireError;

use crate::cli::RunArgs;
use crate::config::{WorkflowConfig, validate_groups};
use crate::engine::{execute_steps, print_dry_run};
use crate::error::{CruiseError, Result};
use crate::file_tracker::FileTracker;
use crate::session::{SessionManager, SessionPhase, current_iso8601, get_cruise_home};
use crate::variable::VariableStore;
use crate::worktree;

/// Variable name that maps to the plan file.
const PLAN_VAR: &str = "plan";
const PR_NUMBER_VAR: &str = "pr.number";
const PR_URL_VAR: &str = "pr.url";

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
            // Try to create a PR automatically.
            match create_pr(&ctx.path, &ctx.branch) {
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

/// Create a PR using `gh pr create --fill`. Falls back to `gh pr view` if a PR already exists.
fn create_pr(worktree_path: &Path, branch: &str) -> Result<String> {
    let output = std::process::Command::new("gh")
        .args(["pr", "create", "--fill", "--head", branch])
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
}
