use std::cell::RefCell;

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

pub async fn run(args: RunArgs) -> Result<()> {
    let manager = SessionManager::new(get_cruise_home()?);

    // Auto-cleanup old completed sessions.
    if let Err(e) = manager.cleanup_old(3) {
        eprintln!("warning: cleanup failed: {}", e);
    }

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

    // Also set plan from config.plan if present (backward compat with old configs).
    if let Some(config_plan) = config.plan.as_ref().filter(|p| !p.as_os_str().is_empty()) {
        vars.set_named_file(PLAN_VAR, config_plan.clone());
    }

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
            session.phase = SessionPhase::Completed;
            session.completed_at = Some(current_iso8601());
        }
        Err(e) => {
            session.phase = SessionPhase::Failed(e.to_string());
            session.completed_at = Some(current_iso8601());
        }
    }
    manager.save(session)?;

    // Cleanup worktree unless --keep-worktree.
    if !args.keep_worktree
        && let Err(e) = worktree::cleanup_worktree(&ctx)
    {
        eprintln!("warning: worktree cleanup failed: {}", e);
    }

    exec_result.map(|_| ())
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
