use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use console::style;
use inquire::InquireError;

use crate::cancellation::CancellationToken;
use crate::cli::RunArgs;
use crate::config::validate_config;
use crate::engine::{execute_steps, print_dry_run};
use crate::error::{CruiseError, Result};
use crate::file_tracker::FileTracker;
use crate::option_handler::CliOptionHandler;
use crate::session::PLAN_VAR;
use crate::session::{
    SessionFileContents, SessionLogger, SessionManager, SessionPhase, SessionState,
    SessionStateFingerprint, WorkspaceMode, current_iso8601, get_cruise_home,
};
use crate::variable::VariableStore;
use crate::workflow::CompiledWorkflow;
use crate::workspace::{ExecutionWorkspace, prepare_execution_workspace, update_session_workspace};

const SESSION_STATE_CONFLICT_ABORT_LABEL: &str = "Abort run";
const SESSION_STATE_CONFLICT_OVERWRITE_LABEL: &str = "Overwrite external state";
const WORKSPACE_WORKTREE_LABEL: &str = "Create worktree (new branch)";
const WORKSPACE_CURRENT_BRANCH_LABEL: &str = "Use current branch";

#[cfg(test)]
const TEST_STATE_CONFLICT_ACTION_ENV: &str = "CRUISE_TEST_STATE_CONFLICT_ACTION";
#[cfg(test)]
const TEST_STATE_CONFLICT_LOG_ENV: &str = "CRUISE_TEST_STATE_CONFLICT_LOG";
#[cfg(test)]
const TEST_STDIN_IS_TERMINAL_ENV: &str = "CRUISE_TEST_STDIN_IS_TERMINAL";
#[cfg(test)]
const TEST_WORKSPACE_MODE_ENV: &str = "CRUISE_TEST_WORKSPACE_MODE";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkspaceOverride {
    RespectSession,
    ForceWorktree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionStateConflictChoice {
    Abort,
    Overwrite,
}

/// Returns a safe fallback directory when `set_current_dir` fails.
fn fallback_root() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(std::env::var("SYSTEMDRIVE").unwrap_or_else(|_| "C:".into()) + "\\")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/")
    }
}

struct CurrentDirGuard {
    original: PathBuf,
}

impl CurrentDirGuard {
    fn capture() -> Result<Self> {
        Ok(Self {
            original: std::env::current_dir()?,
        })
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        if std::env::set_current_dir(&self.original).is_err() {
            let _ = std::env::set_current_dir(fallback_root());
        }
    }
}

fn save_session_state_with_conflict_resolution(
    manager: &SessionManager,
    session: &SessionState,
    expected_fingerprint: SessionStateFingerprint,
) -> Result<SessionStateFingerprint> {
    // Single read: inspect gives us both the fingerprint and parsed contents.
    let current_contents = manager.inspect_state_file(&session.id)?;
    let current_fingerprint = current_contents.fingerprint();
    if current_fingerprint == Some(expected_fingerprint) {
        return manager.save_with_fingerprint(session);
    }

    // Conflict detected -- build a user-facing message from the already-read contents.
    let state_path = manager.state_path(&session.id);
    let message = session_state_conflict_message(&state_path, &current_contents);

    if !stdin_is_terminal() {
        return Err(CruiseError::SessionStateConflict(message));
    }

    match prompt_for_session_state_conflict(&message)? {
        SessionStateConflictChoice::Abort => {
            #[cfg(test)]
            record_session_state_conflict_choice("abort");
            Err(CruiseError::SessionStateConflictAborted(message))
        }
        SessionStateConflictChoice::Overwrite => {
            #[cfg(test)]
            record_session_state_conflict_choice("overwrite");
            manager.save_with_fingerprint(session)
        }
    }
}

fn session_state_conflict_message(path: &Path, current_contents: &SessionFileContents) -> String {
    match current_contents {
        SessionFileContents::Missing => {
            format!(
                "{} was deleted while the session was running",
                path.display()
            )
        }
        SessionFileContents::Parsed { .. } => {
            format!(
                "{} changed externally while the session was running",
                path.display()
            )
        }
        SessionFileContents::Invalid { error, .. } => format!(
            "{} changed externally and now contains invalid JSON: {}",
            path.display(),
            error
        ),
    }
}

fn prompt_for_session_state_conflict(message: &str) -> Result<SessionStateConflictChoice> {
    #[cfg(test)]
    if let Some(choice) = test_session_state_conflict_choice() {
        return Ok(choice);
    }

    eprintln!("{} {}", style("!").yellow().bold(), message);
    let options = vec![
        SESSION_STATE_CONFLICT_ABORT_LABEL,
        SESSION_STATE_CONFLICT_OVERWRITE_LABEL,
    ];
    match inquire::Select::new("How should cruise proceed?", options).prompt() {
        Ok(choice) if choice == SESSION_STATE_CONFLICT_ABORT_LABEL => {
            Ok(SessionStateConflictChoice::Abort)
        }
        Ok(_) => Ok(SessionStateConflictChoice::Overwrite),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            Ok(SessionStateConflictChoice::Abort)
        }
        Err(e) => Err(CruiseError::Other(format!("selection error: {e}"))),
    }
}

fn stdin_is_terminal() -> bool {
    #[cfg(test)]
    if let Ok(value) = std::env::var(TEST_STDIN_IS_TERMINAL_ENV) {
        return value == "1";
    }

    std::io::stdin().is_terminal()
}

fn prompt_workspace_mode() -> Result<WorkspaceMode> {
    #[cfg(test)]
    if let Some(mode) = test_workspace_mode_choice() {
        return Ok(mode);
    }

    if !stdin_is_terminal() {
        return Ok(WorkspaceMode::Worktree);
    }

    let options = vec![WORKSPACE_WORKTREE_LABEL, WORKSPACE_CURRENT_BRANCH_LABEL];
    match inquire::Select::new("Where should cruise execute?", options).prompt() {
        Ok(choice) if choice == WORKSPACE_CURRENT_BRANCH_LABEL => Ok(WorkspaceMode::CurrentBranch),
        Ok(_) => Ok(WorkspaceMode::Worktree),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Err(
            CruiseError::Other("workspace mode selection cancelled".to_string()),
        ),
        Err(e) => Err(CruiseError::Other(format!("selection error: {e}"))),
    }
}

#[cfg(test)]
fn test_workspace_mode_choice() -> Option<WorkspaceMode> {
    std::env::var(TEST_WORKSPACE_MODE_ENV)
        .ok()
        .and_then(|v| match v.as_str() {
            "current_branch" => Some(WorkspaceMode::CurrentBranch),
            "worktree" => Some(WorkspaceMode::Worktree),
            _ => None,
        })
}

#[cfg(test)]
fn test_session_state_conflict_choice() -> Option<SessionStateConflictChoice> {
    std::env::var(TEST_STATE_CONFLICT_ACTION_ENV)
        .ok()
        .and_then(|value| match value.as_str() {
            "abort" => Some(SessionStateConflictChoice::Abort),
            "overwrite" => Some(SessionStateConflictChoice::Overwrite),
            _ => None,
        })
}

#[cfg(test)]
fn record_session_state_conflict_choice(choice: &str) {
    if let Ok(path) = std::env::var(TEST_STATE_CONFLICT_LOG_ENV)
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        use std::io::Write;

        let _ = writeln!(file, "{choice}");
    }
}

fn load_run_all_result_state(
    manager: &SessionManager,
    fallback: &SessionState,
) -> Result<SessionState> {
    let contents = manager.inspect_state_file(&fallback.id)?;
    if let SessionFileContents::Parsed { state, .. } = contents {
        Ok(*state)
    } else {
        let state_path = manager.state_path(&fallback.id);
        let message = session_state_conflict_message(&state_path, &contents);
        let mut state = fallback.clone();
        state.phase = SessionPhase::Failed(message);
        state.completed_at = Some(current_iso8601());
        Ok(state)
    }
}

pub async fn run(args: RunArgs) -> Result<()> {
    if args.all {
        if args.session.is_some() {
            return Err(CruiseError::Other(
                "Cannot specify both --all and a session ID".to_string(),
            ));
        }
        return run_all(args).await;
    }

    match run_single(args, WorkspaceOverride::RespectSession).await {
        Err(CruiseError::StepPaused) => {
            eprintln!("Session paused. Resume with `cruise run`.");
            Ok(())
        }
        other => other,
    }
}

#[expect(clippy::too_many_lines)]
async fn run_single(args: RunArgs, workspace_override: WorkspaceOverride) -> Result<()> {
    let _current_dir_guard = CurrentDirGuard::capture()?;
    let manager = SessionManager::new(get_cruise_home()?);
    let session_id = args
        .session
        .map_or_else(|| select_pending_session(&manager), Ok)?;
    let (mut session, initial_fingerprint) = manager.load_with_fingerprint(&session_id)?;

    if !session.phase.is_runnable() {
        return Err(CruiseError::Other(format!(
            "Session {} is in '{}' phase and cannot be run. Approve it first with `cruise list`.",
            session_id,
            session.phase.label()
        )));
    }

    let config = manager.load_config(&session)?;
    validate_config(&config)?;

    if args.dry_run {
        eprintln!("{}", style(format!("Session: {session_id}")).dim());
        print_dry_run(&config, session.current_step.as_deref());
        return Ok(());
    }

    let compiled = crate::workflow::compile(config)?;
    let effective_workspace_mode = match workspace_override {
        WorkspaceOverride::ForceWorktree => WorkspaceMode::Worktree,
        WorkspaceOverride::RespectSession => {
            if session.current_step.is_none() && session.workspace_mode == WorkspaceMode::Worktree {
                prompt_workspace_mode()?
            } else {
                session.workspace_mode
            }
        }
    };
    session.workspace_mode = effective_workspace_mode;
    if effective_workspace_mode == WorkspaceMode::Worktree {
        crate::worktree_pr::ensure_gh_available()?;
    }
    let start_step = session.current_step.clone().map_or_else(
        || {
            compiled
                .steps
                .keys()
                .next()
                .ok_or_else(|| CruiseError::Other("config has no steps".to_string()))
                .cloned()
        },
        Ok,
    )?;
    log_resume_message(&session);
    std::env::set_current_dir(session.base_dir.clone())?;
    let execution_workspace =
        prepare_execution_workspace(&manager, &mut session, effective_workspace_mode)?;
    log_execution_workspace(&execution_workspace);
    update_session_workspace(&mut session, &execution_workspace);
    session.phase = SessionPhase::Running;
    let initial_fingerprint =
        save_session_state_with_conflict_resolution(&manager, &session, initial_fingerprint)?;
    std::env::set_current_dir(execution_workspace.path())?;

    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = VariableStore::new(session.input.clone());
    vars.set_named_file(PLAN_VAR, plan_path);
    let mut tracker = FileTracker::with_root(execution_workspace.path().to_path_buf());
    let config_reloader: Option<Box<dyn Fn() -> Result<Option<CompiledWorkflow>>>> =
        session.config_path.as_ref().map(|path| {
            let path = path.clone();
            let last_mtime = Cell::new(std::fs::metadata(&path).and_then(|m| m.modified()).ok());
            Box::new(move || {
                let current_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                if current_mtime == last_mtime.get() {
                    return Ok(None);
                }
                let yaml = std::fs::read_to_string(&path).map_err(|e| {
                    CruiseError::Other(format!("failed to read config {}: {}", path.display(), e))
                })?;
                let config = crate::config::WorkflowConfig::from_yaml(&yaml)
                    .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?;
                let compiled = crate::workflow::compile(config)?;
                last_mtime.set(current_mtime);
                Ok(Some(compiled))
            }) as Box<dyn Fn() -> Result<Option<CompiledWorkflow>>>
        });
    let log_path = manager.run_log_path(&session_id);
    let logger = SessionLogger::new(log_path);
    logger.write("--- run started ---");
    let skipped_steps = session.skipped_steps.clone();
    let session_cell = RefCell::new(&mut session);
    let session_fingerprint = Cell::new(initial_fingerprint);
    let on_step_start = |step: &str| {
        logger.write(step);
        let mut s = session_cell.borrow_mut();
        s.current_step = Some(step.to_string());
        let fingerprint =
            save_session_state_with_conflict_resolution(&manager, &s, session_fingerprint.get())?;
        session_fingerprint.set(fingerprint);
        Ok(())
    };
    let cancel_token = CancellationToken::new();
    let ctx = crate::engine::ExecutionContext {
        compiled: &compiled,
        max_retries: args.max_retries,
        rate_limit_retries: args.rate_limit_retries,
        on_step_start: &on_step_start,
        cancel_token: Some(&cancel_token),
        option_handler: &CliOptionHandler,
        config_reloader: config_reloader.as_deref(),
        working_dir: Some(execution_workspace.path()),
        skipped_steps: &skipped_steps,
    };
    let exec_result = tokio::select! {
        result = execute_steps(&ctx, &mut vars, &mut tracker, &start_step) => result,
        _ = tokio::signal::ctrl_c() => {
            cancel_token.cancel();
            Err(CruiseError::Interrupted)
        },
    };
    let session = session_cell.into_inner();

    // Handle Ctrl+C: save as Suspended so the session can be resumed later.
    if matches!(exec_result, Err(CruiseError::Interrupted)) {
        logger.write("|| cancelled");
        eprintln!(
            "\n{} Interrupted -- session saved as Suspended.",
            style("||").yellow().bold()
        );
        session.phase = SessionPhase::Suspended;
        manager.save(session)?;
        return Err(CruiseError::Interrupted);
    }

    let overall_result = match exec_result {
        Ok(exec) => {
            logger.write(&format!(
                "v completed -- run: {}, skipped: {}, failed: {}",
                exec.run, exec.skipped, exec.failed
            ));
            match &execution_workspace {
                ExecutionWorkspace::CurrentBranch { .. } => Ok(()),
                ExecutionWorkspace::Worktree { ctx, .. } => {
                    crate::worktree_pr::handle_worktree_pr(
                        ctx,
                        &compiled,
                        &mut vars,
                        &mut tracker,
                        session,
                        args.rate_limit_retries,
                        args.max_retries,
                    )
                    .await
                }
            }
        }
        Err(e) => {
            logger.write(&format!("x failed: {}", e.detailed_message()));
            Err(e)
        }
    };

    if let Err(e) = &overall_result
        && matches!(
            e,
            CruiseError::SessionStateConflict(_) | CruiseError::SessionStateConflictAborted(_)
        )
    {
        return overall_result;
    }

    apply_run_result_to_session(session, &overall_result);
    save_session_state_with_conflict_resolution(&manager, session, session_fingerprint.get())?;
    overall_result
}

/// Apply the result of a step execution to the session state.
///
/// - `Ok(())` -> `Completed`
/// - `Err(StepPaused)` -> keep `Running` (session can be resumed with `cruise run`)
/// - `Err(other)` -> `Failed`
fn apply_run_result_to_session(session: &mut SessionState, result: &Result<()>) {
    match result {
        Ok(()) => {
            session.phase = SessionPhase::Completed;
            session.completed_at = Some(current_iso8601());
        }
        Err(CruiseError::StepPaused) => {
            // Keep Running phase so the session can be resumed later.
        }
        Err(e) => {
            session.phase = SessionPhase::Failed(e.to_string());
            session.completed_at = Some(current_iso8601());
        }
    }
}

/// Log a resume message if the session is being restarted.
fn log_resume_message(session: &SessionState) {
    let Some(ref step) = session.current_step else {
        return;
    };
    match &session.phase {
        SessionPhase::Running | SessionPhase::Suspended => {
            eprintln!("{} Resuming from step: {}", style("R").cyan(), step);
        }
        SessionPhase::Failed(_) => {
            eprintln!(
                "{} Retrying from failed step: {}",
                style("R").yellow(),
                step
            );
        }
        _ => {}
    }
}

/// Log the chosen execution workspace for CLI users.
fn log_execution_workspace(ws: &ExecutionWorkspace) {
    match ws {
        ExecutionWorkspace::Worktree { ctx, reused } => {
            let suffix = if *reused { " (reused)" } else { "" };
            eprintln!(
                "{} worktree: {}{}",
                style("->").cyan(),
                ctx.path.display(),
                suffix
            );
        }
        ExecutionWorkspace::CurrentBranch { path } => {
            eprintln!("{} current branch: {}", style("->").cyan(), path.display());
        }
    }
}

async fn run_all(args: RunArgs) -> Result<()> {
    let manager = SessionManager::new(get_cruise_home()?);
    let mut seen: HashSet<String> = HashSet::new();
    let mut results: Vec<SessionState> = Vec::new();

    loop {
        let remaining = manager.run_all_remaining(&seen)?;
        let Some(session) = remaining.into_iter().next() else {
            break;
        };
        seen.insert(session.id.clone());

        let session_args = RunArgs {
            session: Some(session.id.clone()),
            all: false,
            max_retries: args.max_retries,
            rate_limit_retries: args.rate_limit_retries,
            dry_run: args.dry_run,
        };
        let run_result = Box::pin(run_single(session_args, WorkspaceOverride::ForceWorktree)).await;
        let interrupted = matches!(run_result, Err(CruiseError::Interrupted));
        match run_result {
            Err(CruiseError::StepPaused) => {
                eprintln!("session {} paused by user", session.id);
            }
            Err(e) if !interrupted => {
                eprintln!(
                    "warning: session {} encountered an error: {}",
                    session.id,
                    e.detailed_message()
                );
            }
            Ok(()) | Err(_) => {}
        }
        results.push(load_run_all_result_state(&manager, &session)?);
        if interrupted {
            break;
        }
    }

    let summary = format_run_all_summary(&results);
    if !summary.is_empty() {
        eprintln!("\n{summary}");
    }

    Ok(())
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
            "{} Selected session: {} -- {}",
            style("->").cyan(),
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
    let label_refs: Vec<&str> = labels.iter().map(std::string::String::as_str).collect();

    let selected = match inquire::Select::new("Select a session to run:", label_refs).prompt() {
        Ok(s) => s,
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            return Err(CruiseError::Other(
                "session selection cancelled".to_string(),
            ));
        }
        Err(e) => return Err(CruiseError::Other(format!("selection error: {e}"))),
    };

    let idx = labels
        .iter()
        .position(|l| l.as_str() == selected)
        .ok_or_else(|| CruiseError::Other(format!("selected session not found: {selected}")))?;
    Ok(pending[idx].id.clone())
}

/// Format a summary of all sessions run by `run --all`.
/// Returns an empty string if `results` is empty.
fn format_run_all_summary(results: &[SessionState]) -> String {
    const MAX_INPUT_CHARS: usize = 60;

    if results.is_empty() {
        return String::new();
    }

    let mut lines = Vec::with_capacity(results.len() + 1);
    lines.push(format!(
        "=== Run All Summary ({} sessions) ===",
        results.len()
    ));

    for (i, result) in results.iter().enumerate() {
        let truncated = crate::display::truncate(&result.input, MAX_INPUT_CHARS);

        let line = match &result.phase {
            SessionPhase::Completed => {
                let pr = result
                    .pr_url
                    .as_deref()
                    .map(|url| format!(" {} {url}", style("->").yellow()))
                    .unwrap_or_default();
                format!(
                    "[{}] {} {}{}",
                    i + 1,
                    style("v").green().bold(),
                    truncated,
                    pr
                )
            }
            SessionPhase::Failed(err) => {
                format!(
                    "[{}] {} {} -- Failed: {}",
                    i + 1,
                    style("x").red().bold(),
                    truncated,
                    err
                )
            }
            SessionPhase::Suspended => {
                format!(
                    "[{}] {} {} -- Suspended",
                    i + 1,
                    style("||").yellow().bold(),
                    truncated
                )
            }
            SessionPhase::AwaitingApproval | SessionPhase::Planned | SessionPhase::Running => {
                format!("[{}] ? {}", i + 1, truncated)
            }
        };
        lines.push(line);
    }

    lines.join("\n")
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::cli::{DEFAULT_MAX_RETRIES, DEFAULT_RATE_LIMIT_RETRIES};
    use crate::session::WorkspaceMode;
    use crate::worktree_pr::{
        CommitOutcome, PrAttemptOutcome, attempt_pr_creation, build_pr_prompt,
        extract_last_path_segment, parse_pr_metadata, strip_code_block,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

    use crate::test_binary_support::PathEnvGuard;
    use crate::test_support::{init_git_repo, run_git_ok};
    use crate::worktree;

    fn git_stdout_ok(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| panic!("git command failed to start: {e}"));
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn create_worktree(tmp: &TempDir, session_id: &str) -> (PathBuf, worktree::WorktreeContext) {
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);

        // Set up a local bare repo as "origin" so git push works in tests
        let bare = tmp.path().join("origin.git");
        run_git_ok(tmp.path(), &["init", "--bare", "origin.git"]);
        run_git_ok(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                bare.to_str().unwrap_or_else(|| panic!("unexpected None")),
            ],
        );

        let worktrees_dir = tmp.path().join("worktrees");
        let (ctx, reused) =
            worktree::setup_session_worktree(&repo, session_id, "test task", &worktrees_dir, None)
                .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(!reused, "test worktree should be created fresh");
        (repo, ctx)
    }

    fn install_fake_gh(bin_dir: &Path, log_path: &Path, head_path: &Path, url: &str) {
        fs::create_dir_all(bin_dir).unwrap_or_else(|e| panic!("{e:?}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let script_path = bin_dir.join("gh");
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\ngit rev-parse HEAD > \"{}\"\nprintf '%s\\n' \"{}\"\n",
                log_path.display(),
                head_path.display(),
                url
            );
            fs::write(&script_path, script).unwrap_or_else(|e| panic!("{e:?}"));
            let mut perms = fs::metadata(&script_path)
                .unwrap_or_else(|e| panic!("{e:?}"))
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script_path, perms).unwrap_or_else(|e| panic!("{e:?}"));
        }
        #[cfg(windows)]
        {
            let script_path = bin_dir.join("gh.cmd");
            let script = format!(
                "@echo off\r\necho %*>>\"{}\"\r\ngit rev-parse HEAD > \"{}\"\r\necho {}\r\n",
                log_path.display(),
                head_path.display(),
                url
            );
            fs::write(&script_path, script).unwrap();
        }
    }

    fn install_logging_gh(bin_dir: &Path, log_path: &Path, url: &str) {
        fs::create_dir_all(bin_dir).unwrap_or_else(|e| panic!("{e:?}"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let script_path = bin_dir.join("gh");
            let script = format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf '%s\\n' 'gh version test'\n  exit 0\nfi\nprintf '%s\\n' \"$*\" >> \"{}\"\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then\n  printf '%s\\n' \"{}\"\nfi\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"view\" ]; then\n  printf '%s\\n' \"{}\"\nfi\n",
                log_path.display(),
                url,
                url
            );
            fs::write(&script_path, script).unwrap_or_else(|e| panic!("{e:?}"));
            let mut perms = fs::metadata(&script_path)
                .unwrap_or_else(|e| panic!("{e:?}"))
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script_path, perms).unwrap_or_else(|e| panic!("{e:?}"));
        }
        #[cfg(windows)]
        {
            let script_path = bin_dir.join("gh.cmd");
            let script = format!(
                "@echo off\r\nif \"%1\"==\"--version\" (\r\n  echo gh version test\r\n  exit /b 0\r\n)\r\necho %*>>\"{}\"\r\nif \"%1\"==\"pr\" if \"%2\"==\"create\" echo {}\r\nif \"%1\"==\"pr\" if \"%2\"==\"view\" echo {}\r\n",
                log_path.display(),
                url,
                url
            );
            fs::write(&script_path, script).unwrap();
        }
    }

    struct ProcessStateGuard {
        prev_home: Option<std::ffi::OsString>,
        prev_userprofile: Option<std::ffi::OsString>,
        prev_path: Option<std::ffi::OsString>,
        prev_dir: PathBuf,
        extra_env: Vec<(String, Option<std::ffi::OsString>)>,
        lock: crate::test_support::ProcessLock,
    }

    impl ProcessStateGuard {
        fn new(home: &Path) -> Self {
            let lock = crate::test_support::lock_process();
            let prev_home = std::env::var_os("HOME");
            let prev_userprofile = std::env::var_os("USERPROFILE");
            let prev_path = std::env::var_os("PATH");
            let prev_dir = std::env::current_dir().unwrap_or_else(|_| fallback_root());
            unsafe {
                std::env::set_var("HOME", home);
                std::env::set_var("USERPROFILE", home);
            }
            Self {
                prev_home,
                prev_userprofile,
                prev_path,
                prev_dir,
                extra_env: Vec::new(),
                lock,
            }
        }

        fn prepend_path(&self, dir: &Path) {
            // `self.lock` ensures the caller holds a `ProcessStateGuard` (and therefore the process lock).
            let _ = &self.lock;
            let mut paths = vec![dir.to_path_buf()];
            if let Some(existing) = std::env::var_os("PATH") {
                paths.extend(std::env::split_paths(&existing));
            }
            if let Ok(joined) = std::env::join_paths(paths) {
                unsafe { std::env::set_var("PATH", joined) };
            }
        }

        fn set_current_dir(&self, dir: &Path) {
            // `self.lock` ensures the caller holds a `ProcessStateGuard` (and therefore the process lock).
            let _ = &self.lock;
            let _ = std::env::set_current_dir(dir);
        }

        fn set_env(&mut self, key: &str, value: impl AsRef<std::ffi::OsStr>) {
            self.remember_env(key);
            unsafe {
                std::env::set_var(key, value);
            }
        }

        fn remove_env(&mut self, key: &str) {
            self.remember_env(key);
            unsafe {
                std::env::remove_var(key);
            }
        }

        fn remember_env(&mut self, key: &str) {
            if self.extra_env.iter().all(|(existing, _)| existing != key) {
                self.extra_env
                    .push((key.to_string(), std::env::var_os(key)));
            }
        }
    }

    impl Drop for ProcessStateGuard {
        fn drop(&mut self) {
            if std::env::set_current_dir(&self.prev_dir).is_err() {
                let _ = std::env::set_current_dir(fallback_root());
            }
            unsafe {
                for (key, previous) in self.extra_env.iter().rev() {
                    if let Some(value) = previous {
                        std::env::set_var(key, value);
                    } else {
                        std::env::remove_var(key);
                    }
                }

                if let Some(ref prev_home) = self.prev_home {
                    std::env::set_var("HOME", prev_home);
                } else {
                    std::env::remove_var("HOME");
                }

                if let Some(ref prev_userprofile) = self.prev_userprofile {
                    std::env::set_var("USERPROFILE", prev_userprofile);
                } else {
                    std::env::remove_var("USERPROFILE");
                }

                if let Some(ref prev_path) = self.prev_path {
                    std::env::set_var("PATH", prev_path);
                } else {
                    std::env::remove_var("PATH");
                }
            }
        }
    }

    fn create_repo_with_origin(tmp: &TempDir) -> PathBuf {
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);

        let bare = tmp.path().join("origin.git");
        run_git_ok(tmp.path(), &["init", "--bare", "origin.git"]);
        run_git_ok(
            &repo,
            &[
                "remote",
                "add",
                "origin",
                bare.to_str().unwrap_or_else(|| panic!("unexpected None")),
            ],
        );

        repo
    }

    fn make_current_branch_session(
        id: &str,
        repo: &Path,
        input: &str,
        target_branch: &str,
    ) -> SessionState {
        let mut session = SessionState::new(
            id.to_string(),
            repo.to_path_buf(),
            "cruise.yaml".to_string(),
            input.to_string(),
        );
        session.phase = SessionPhase::Planned;
        session.workspace_mode = WorkspaceMode::CurrentBranch;
        session.target_branch = Some(target_branch.to_string());
        session
    }

    fn write_config(manager: &SessionManager, session_id: &str, yaml: &str) {
        let session_dir = manager.sessions_dir().join(session_id);
        fs::create_dir_all(&session_dir).unwrap_or_else(|e| panic!("{e:?}"));
        fs::write(session_dir.join("config.yaml"), yaml).unwrap_or_else(|e| panic!("{e:?}"));
    }

    fn single_command_config(step_name: &str, command: &str) -> String {
        format!("command:\n  - cat\nsteps:\n  {step_name}:\n    command: |\n      {command}\n")
    }

    fn run_args(session_id: &str) -> RunArgs {
        RunArgs {
            session: Some(session_id.to_string()),
            all: false,
            max_retries: 10,
            rate_limit_retries: 0,
            dry_run: false,
        }
    }

    fn blocking_conflict_config() -> String {
        r"command:
  - cat
steps:
  first:
    command: |
      while [ ! -f proceed.txt ]; do sleep 0.05; done
  second:
    command: |
      printf second > second.txt
"
        .to_string()
    }

    fn setup_current_branch_conflict_session(
        tmp: &TempDir,
        session_id: &str,
        input: &str,
    ) -> (ProcessStateGuard, PathBuf, SessionManager) {
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(tmp);
        process.set_current_dir(&repo);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session = make_current_branch_session(session_id, &repo, input, "main");
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(&manager, session_id, &blocking_conflict_config());

        (process, repo, manager)
    }

    fn configure_conflict_test_env(
        process: &mut ProcessStateGuard,
        is_terminal: bool,
        action: Option<&str>,
        log_path: &Path,
    ) {
        process.set_env(
            TEST_STDIN_IS_TERMINAL_ENV,
            if is_terminal { "1" } else { "0" },
        );
        if let Some(action) = action {
            process.set_env(TEST_STATE_CONFLICT_ACTION_ENV, action);
        } else {
            process.remove_env(TEST_STATE_CONFLICT_ACTION_ENV);
        }
        process.set_env(TEST_STATE_CONFLICT_LOG_ENV, log_path);
    }

    async fn wait_for_session_step(manager: &SessionManager, session_id: &str, step: &str) {
        for _ in 0..200 {
            if let Ok(state) = manager.load(session_id)
                && matches!(state.phase, SessionPhase::Running)
                && state.current_step.as_deref() == Some(step)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        panic!("timed out waiting for session {session_id} to reach step {step}");
    }

    async fn mutate_state_after_first_step<F, G>(
        manager: &SessionManager,
        session_id: &str,
        workspace_path: G,
        mutate: F,
    ) where
        F: FnOnce(&SessionManager, &str),
        G: FnOnce(&SessionState) -> PathBuf,
    {
        wait_for_session_step(manager, session_id, "first").await;
        let state = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        let workspace = workspace_path(&state);
        mutate(manager, session_id);
        fs::write(workspace.join("proceed.txt"), "go").unwrap_or_else(|e| panic!("{e:?}"));
    }

    fn write_external_failed_state(manager: &SessionManager, session_id: &str) {
        let mut external = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        external.phase = SessionPhase::Failed("external edit".to_string());
        external.current_step = Some("external-step".to_string());
        manager.save(&external).unwrap_or_else(|e| panic!("{e:?}"));
    }

    fn make_pr_prompt_config(pr_language_yaml: Option<&str>) -> CompiledWorkflow {
        let mut yaml = String::from("command: [claude, -p]\n");
        if let Some(pr_language_yaml) = pr_language_yaml {
            yaml.push_str(pr_language_yaml);
        }
        yaml.push_str("steps:\n  implement:\n    prompt: test\n");
        let config =
            crate::config::WorkflowConfig::from_yaml(&yaml).unwrap_or_else(|e| panic!("{e:?}"));
        crate::workflow::compile(config).unwrap_or_else(|e| panic!("{e:?}"))
    }

    fn make_pr_prompt_vars() -> VariableStore {
        let mut vars = VariableStore::new("test input".to_string());
        vars.set_named_file(PLAN_VAR, PathBuf::from("plan.md"));
        vars
    }

    #[test]
    fn test_extract_last_path_segment_github_pr_url() {
        // Given: a standard GitHub PR URL
        let url = "https://github.com/owner/repo/pull/42";
        // When: extracting the last segment
        let result = extract_last_path_segment(url);
        // Then: last segment is returned
        assert_eq!(result, Some("42".to_string()));
    }

    #[test]
    fn test_build_pr_prompt_includes_configured_pr_language() {
        // Given: a workflow config with a custom PR language
        let config = make_pr_prompt_config(Some("pr_language: Japanese\n"));
        let mut vars = make_pr_prompt_vars();

        // When: building the PR prompt
        let prompt = build_pr_prompt(&mut vars, &config).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the configured language is injected into the prompt
        assert!(
            prompt.contains("Japanese"),
            "prompt should include configured language: {prompt}"
        );
        assert!(
            prompt.contains("plan.md"),
            "prompt should continue resolving existing variables: {prompt}"
        );
    }

    #[test]
    fn test_build_pr_prompt_defaults_blank_pr_language_to_english() {
        // Given: a workflow config with a blank PR language
        let config = make_pr_prompt_config(Some("pr_language: \"   \"\n"));
        let mut vars = make_pr_prompt_vars();

        // When: building the PR prompt
        let prompt = build_pr_prompt(&mut vars, &config).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the prompt falls back to the built-in English default
        assert!(
            prompt.contains(crate::config::DEFAULT_PR_LANGUAGE),
            "prompt should include the default language: {prompt}"
        );
    }

    #[test]
    fn test_attempt_pr_creation_skips_gh_when_branch_has_no_commits() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let (_repo, ctx) = create_worktree(&tmp, "20260307225900");
        let bin_dir = tmp.path().join("bin");
        let log_path = tmp.path().join("gh.log");
        let head_path = tmp.path().join("gh-head.txt");
        install_fake_gh(
            &bin_dir,
            &log_path,
            &head_path,
            "https://github.com/owner/repo/pull/1",
        );
        let _path_guard = PathEnvGuard::prepend(&bin_dir);

        let result =
            attempt_pr_creation(&ctx, "test task", "", "").unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(result, PrAttemptOutcome::SkippedNoCommits);
        assert!(
            !log_path.exists(),
            "gh should not be called when no commit exists"
        );
        assert!(
            !head_path.exists(),
            "gh should not observe HEAD when skipped"
        );
        worktree::cleanup_worktree(&ctx).unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[test]
    fn test_attempt_pr_creation_commits_changes_before_calling_gh() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let f = setup_pr_test(
            &tmp,
            "20260307225901",
            "https://github.com/owner/repo/pull/2",
        );
        let base_head = git_stdout_ok(&f.repo, &["rev-parse", "HEAD"]);

        let result =
            attempt_pr_creation(&f.ctx, "add feature", "", "").unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(
            result,
            PrAttemptOutcome::Created {
                url: f.url.clone(),
                commit_outcome: CommitOutcome::Created,
            }
        );
        assert_eq!(
            git_stdout_ok(&f.ctx.path, &["log", "-1", "--pretty=%s"]),
            "add feature"
        );
        let worktree_head = git_stdout_ok(&f.ctx.path, &["rev-parse", "HEAD"]);
        assert_ne!(
            worktree_head, base_head,
            "helper should create a new commit"
        );
        assert_eq!(
            fs::read_to_string(&f.head_path)
                .unwrap_or_else(|e| panic!("{e:?}"))
                .trim(),
            worktree_head
        );
        let gh_args = fs::read_to_string(&f.log_path).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            gh_args.contains("pr create --head") && gh_args.contains("--fill"),
            "fake gh should receive a pr create invocation, got: {gh_args}"
        );
        assert!(
            gh_args.contains("--draft"),
            "gh pr create should include --draft flag, got: {gh_args}"
        );
        worktree::cleanup_worktree(&f.ctx).unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[test]
    fn test_attempt_pr_creation_reuses_existing_branch_commits() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let f = setup_pr_test(
            &tmp,
            "20260307225902",
            "https://github.com/owner/repo/pull/3",
        );
        let base_head = git_stdout_ok(&f.repo, &["rev-parse", "HEAD"]);
        run_git_ok(&f.ctx.path, &["add", "."]);
        run_git_ok(&f.ctx.path, &["commit", "-m", "existing commit"]);

        let existing_head = git_stdout_ok(&f.ctx.path, &["rev-parse", "HEAD"]);
        assert_ne!(existing_head, base_head);

        let result = attempt_pr_creation(&f.ctx, "rerun without changes", "", "")
            .unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(
            result,
            PrAttemptOutcome::Created {
                url: f.url.clone(),
                commit_outcome: CommitOutcome::NoChanges,
            }
        );
        assert_eq!(
            git_stdout_ok(&f.ctx.path, &["rev-parse", "HEAD"]),
            existing_head
        );
        assert_eq!(
            fs::read_to_string(&f.head_path)
                .unwrap_or_else(|e| panic!("{e:?}"))
                .trim(),
            existing_head
        );
        worktree::cleanup_worktree(&f.ctx).unwrap_or_else(|e| panic!("{e:?}"));
    }

    struct PrTestFixture {
        repo: PathBuf,
        ctx: worktree::WorktreeContext,
        #[expect(dead_code)]
        path_guard: PathEnvGuard,
        log_path: PathBuf,
        head_path: PathBuf,
        url: String,
    }

    fn setup_pr_test(tmp: &TempDir, session_id: &str, url: &str) -> PrTestFixture {
        let (repo, ctx) = create_worktree(tmp, session_id);
        fs::write(ctx.path.join("feature.txt"), "hello").unwrap_or_else(|e| panic!("{e:?}"));

        let bin_dir = tmp.path().join("bin");
        let log_path = tmp.path().join("gh.log");
        let head_path = tmp.path().join("gh-head.txt");
        install_fake_gh(&bin_dir, &log_path, &head_path, url);
        let path_guard = PathEnvGuard::prepend(&bin_dir);

        PrTestFixture {
            repo,
            ctx,
            path_guard,
            log_path,
            head_path,
            url: url.to_string(),
        }
    }

    #[test]
    fn test_attempt_pr_creation_uses_pr_title_as_commit_message_when_title_is_present() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let f = setup_pr_test(
            &tmp,
            "20260310pr_title_commit_01",
            "https://github.com/owner/repo/pull/10",
        );

        let pr_title = "feat: add user icon registration";
        let result = attempt_pr_creation(&f.ctx, "implement user icon feature", pr_title, "")
            .unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(
            result,
            PrAttemptOutcome::Created {
                url: f.url.clone(),
                commit_outcome: CommitOutcome::Created
            }
        );
        assert_eq!(
            git_stdout_ok(&f.ctx.path, &["log", "-1", "--pretty=%s"]),
            pr_title,
            "commit subject should equal the PR title when title is non-empty"
        );
        let gh_args = fs::read_to_string(&f.log_path).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            gh_args.contains("--title") && gh_args.contains(pr_title),
            "fake gh should receive --title with the PR title; got: {gh_args}"
        );
        worktree::cleanup_worktree(&f.ctx).unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[test]
    fn test_attempt_pr_creation_falls_back_to_message_when_pr_title_is_empty() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let f = setup_pr_test(
            &tmp,
            "20260310pr_title_commit_02",
            "https://github.com/owner/repo/pull/11",
        );

        let fallback = "implement user icon feature";
        let result =
            attempt_pr_creation(&f.ctx, fallback, "", "").unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(
            result,
            PrAttemptOutcome::Created {
                url: f.url.clone(),
                commit_outcome: CommitOutcome::Created
            }
        );
        assert_eq!(
            git_stdout_ok(&f.ctx.path, &["log", "-1", "--pretty=%s"]),
            fallback,
            "commit subject should equal the fallback message when PR title is empty"
        );
        let gh_args = fs::read_to_string(&f.log_path).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            gh_args.contains("--fill"),
            "fake gh should receive --fill when PR title is empty; got: {gh_args}"
        );
        worktree::cleanup_worktree(&f.ctx).unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[test]
    fn test_attempt_pr_creation_treats_whitespace_only_title_as_empty() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let f = setup_pr_test(
            &tmp,
            "20260310pr_title_commit_03",
            "https://github.com/owner/repo/pull/12",
        );

        let fallback = "implement user icon feature";
        let result =
            attempt_pr_creation(&f.ctx, fallback, "   ", "").unwrap_or_else(|e| panic!("{e:?}"));

        assert_eq!(
            result,
            PrAttemptOutcome::Created {
                url: f.url.clone(),
                commit_outcome: CommitOutcome::Created
            }
        );
        assert_eq!(
            git_stdout_ok(&f.ctx.path, &["log", "-1", "--pretty=%s"]),
            fallback,
            "whitespace-only title should be treated as empty and use fallback message"
        );
        let gh_args = fs::read_to_string(&f.log_path).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            gh_args.contains("--fill"),
            "fake gh should receive --fill when PR title is whitespace-only; got: {gh_args}"
        );
        worktree::cleanup_worktree(&f.ctx).unwrap_or_else(|e| panic!("{e:?}"));
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

    #[test]
    fn test_parse_pr_metadata_with_preamble_then_frontmatter() {
        // Given: LLM output with preamble text before frontmatter
        let output = "Here is the PR information:\n---\ntitle: \"feat: Add new feature\"\n---\nThis adds a new feature.";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: preamble is ignored, frontmatter is parsed
        assert_eq!(title, "feat: Add new feature");
        assert_eq!(body.trim(), "This adds a new feature.");
    }

    #[test]
    fn test_parse_pr_metadata_with_preamble_then_code_block() {
        // Given: LLM output with preamble then a code-block-wrapped frontmatter
        let output = "Here is the PR information:\n```md\n---\ntitle: \"fix: Fix the bug\"\n---\nFixed the critical bug.\n```";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: preamble and code block delimiters are stripped, frontmatter is parsed
        assert_eq!(title, "fix: Fix the bug");
        assert_eq!(body.trim(), "Fixed the critical bug.");
    }

    #[test]
    fn test_parse_pr_metadata_with_multiline_preamble() {
        // Given: LLM output with multiple lines of preamble
        let output = "I have reviewed the changes.\nBased on the commits, here is the PR:\n\n---\ntitle: \"refactor: Clean up code\"\n---\nRefactored the core module.";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: all preamble lines are skipped, frontmatter is parsed
        assert_eq!(title, "refactor: Clean up code");
        assert_eq!(body.trim(), "Refactored the core module.");
    }

    #[test]
    fn test_parse_pr_metadata_heading_format() {
        // Given: LLM output using Markdown h1 heading as title
        let output = "# feat: Add user icon registration feature\n## Overview\nEnabled users to upload icon images.";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: h1 line is used as title, rest as body
        assert_eq!(title, "feat: Add user icon registration feature");
        assert_eq!(
            body.trim(),
            "## Overview\nEnabled users to upload icon images."
        );
    }

    #[test]
    fn test_parse_pr_metadata_heading_format_in_code_block() {
        // Given: LLM output wrapped in code block using h1 heading
        let output = "```md\n# fix: Resolve login bug\nFixed the login issue.\n```";
        // When: parsing PR metadata
        let (title, body) = parse_pr_metadata(output);
        // Then: code block is stripped and h1 heading is used as title
        assert_eq!(title, "fix: Resolve login bug");
        assert_eq!(body.trim(), "Fixed the login issue.");
    }

    #[test]
    fn test_parse_pr_metadata_empty_input_returns_empty() {
        // Given: empty input
        let (title, body) = parse_pr_metadata("");
        // Then: both are empty
        assert_eq!(title, "");
        assert_eq!(body, "");
    }

    #[test]
    fn test_parse_pr_metadata_whitespace_only_returns_empty() {
        // Given: whitespace-only input
        let (title, body) = parse_pr_metadata("   \n  \n  ");
        // Then: both are empty
        assert_eq!(title, "");
        assert_eq!(body, "");
    }

    #[test]
    fn test_strip_code_block_with_preamble() {
        // Given: text with preamble before code block
        let input = "Some intro text\n```\n---\ntitle: test\n---\nbody\n```";
        // When: stripping code block
        let result = strip_code_block(input);
        // Then: inner content is extracted
        assert_eq!(result.trim(), "---\ntitle: test\n---\nbody");
    }

    #[test]
    fn test_strip_code_block_no_code_block_unchanged() {
        // Given: text without any code block
        let input = "Just plain text here.";
        // When: stripping code block
        let result = strip_code_block(input);
        // Then: input is returned unchanged (trimmed)
        assert_eq!(result, "Just plain text here.");
    }

    // -----------------------------------------------------------------------
    // run_all() integration tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_run_all_errors_when_session_and_all_both_specified() {
        // Given: both --all and a session ID are specified
        let args = RunArgs {
            session: Some("some-session-id".to_string()),
            all: true,
            max_retries: DEFAULT_MAX_RETRIES,
            rate_limit_retries: DEFAULT_RATE_LIMIT_RETRIES,
            dry_run: false,
        };

        // When: call run()
        let result = run(args).await;

        // Then: returns a "Cannot specify both --all and a session ID" error
        assert!(result.is_err(), "expected error but got Ok");
        let msg = result
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
            .to_string();
        assert!(
            msg.contains("Cannot specify both --all and a session ID"),
            "unexpected error message: {msg}"
        );
    }

    #[tokio::test]
    async fn test_run_all_returns_ok_when_no_planned_sessions() {
        // Given: empty cruise home with no planned sessions
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let cruise_home = tmp.path().join(".cruise");
        std::fs::create_dir_all(cruise_home.join("sessions")).unwrap_or_else(|e| panic!("{e:?}"));

        // Hold the lock in a narrow scope so it is dropped before the await.
        let orig_home = {
            let _guard = crate::test_support::lock_process();
            let orig = std::env::var("HOME").ok();
            // SAFETY: only modified within this test and restored before exit.
            unsafe {
                std::env::set_var("HOME", tmp.path());
            }
            orig
            // _guard is dropped here
        };

        let args = RunArgs {
            session: None,
            all: true,
            max_retries: DEFAULT_MAX_RETRIES,
            rate_limit_retries: DEFAULT_RATE_LIMIT_RETRIES,
            dry_run: false,
        };

        // When: call run() with 0 planned sessions
        let result = run(args).await;

        // Restore HOME
        {
            let _guard = crate::test_support::lock_process();
            unsafe {
                match orig_home {
                    Some(h) => std::env::set_var("HOME", h),
                    None => std::env::remove_var("HOME"),
                }
            }
        }

        // Then: returns Ok(()) without error
        assert!(result.is_ok(), "expected Ok but got: {:?}", result.err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_current_branch_mode_keeps_changes_in_base_repo_and_skips_pr_flow() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260309120000";
        let session = make_current_branch_session(session_id, &repo, "edit in place", "main");
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(
            &manager,
            session_id,
            &single_command_config("edit", "printf direct > current-branch.txt"),
        );

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/99");
        process.prepend_path(&bin_dir);

        let result = run(run_args(session_id)).await;
        assert!(
            result.is_ok(),
            "expected current-branch mode to succeed: {result:?}"
        );

        let loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            loaded.worktree_path.is_none(),
            "current-branch mode should not persist a worktree path"
        );
        assert!(
            loaded.worktree_branch.is_none(),
            "current-branch mode should not persist a worktree branch"
        );
        assert!(
            loaded.pr_url.is_none(),
            "current-branch mode should skip PR creation"
        );
        assert!(
            repo.join("current-branch.txt").exists(),
            "current-branch mode should write changes into the base repository"
        );
        assert_eq!(
            fs::read_to_string(repo.join("current-branch.txt")).unwrap_or_else(|e| panic!("{e:?}")),
            "direct"
        );
        assert!(
            git_stdout_ok(&repo, &["status", "--short"]).contains("current-branch.txt"),
            "current-branch mode should leave the new file uncommitted in the base repository"
        );
        assert!(
            !manager.worktrees_dir().join(session_id).exists(),
            "current-branch mode should not create a cruise worktree directory"
        );
        assert!(
            !gh_log.exists(),
            "current-branch mode should not invoke gh at all"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_current_branch_mode_errors_when_branch_has_changed() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260309121000";
        let session =
            make_current_branch_session(session_id, &repo, "stay on planned branch", "main");
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(
            &manager,
            session_id,
            &single_command_config("edit", "printf nope > wrong-branch.txt"),
        );

        run_git_ok(&repo, &["checkout", "-b", "other-branch"]);

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/100");
        process.prepend_path(&bin_dir);

        let result = run(run_args(session_id)).await;
        assert!(
            result.is_err(),
            "expected current-branch mode to reject a branch mismatch"
        );
        let message = result
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
            .to_string();
        assert!(message.contains("branch"), "unexpected error: {message}");
        assert!(message.contains("main"), "unexpected error: {message}");
        assert!(
            message.contains("other-branch"),
            "unexpected error: {message}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_current_branch_mode_errors_when_working_tree_is_dirty() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260309122000";
        let session = make_current_branch_session(session_id, &repo, "edit dirty tree", "main");
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(
            &manager,
            session_id,
            &single_command_config("edit", "printf more > new-file.txt"),
        );

        fs::write(repo.join("already-dirty.txt"), "dirty").unwrap_or_else(|e| panic!("{e:?}"));

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/101");
        process.prepend_path(&bin_dir);

        let result = run(run_args(session_id)).await;
        assert!(
            result.is_err(),
            "expected current-branch mode to reject a dirty working tree"
        );
        let message = result
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
            .to_string();
        assert!(message.contains("dirty"), "unexpected error: {message}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_current_branch_mode_errors_on_detached_head() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260309123000";
        let session = make_current_branch_session(session_id, &repo, "edit detached head", "main");
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(
            &manager,
            session_id,
            &single_command_config("edit", "printf nope > detached.txt"),
        );

        run_git_ok(&repo, &["checkout", "--detach"]);

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/102");
        process.prepend_path(&bin_dir);

        let result = run(run_args(session_id)).await;
        assert!(
            result.is_err(),
            "expected current-branch mode to reject detached HEAD"
        );
        let message = result
            .map_or_else(|e| e, |v| panic!("expected Err, got Ok({v:?})"))
            .to_string();
        assert!(message.contains("detached"), "unexpected error: {message}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_current_branch_mode_resumes_from_saved_step_without_pr_flow() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260309124000";
        let mut session = make_current_branch_session(session_id, &repo, "resume in place", "main");
        session.phase = SessionPhase::Running;
        session.current_step = Some("second".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(
            &manager,
            session_id,
            r"command:
  - cat
steps:
  first:
    command: |
      printf first > first.txt
  second:
    command: |
      printf second > second.txt
",
        );

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/103");
        process.prepend_path(&bin_dir);

        let result = run(run_args(session_id)).await;
        assert!(
            result.is_ok(),
            "expected current-branch resume to succeed: {result:?}"
        );

        let loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            loaded.worktree_path.is_none(),
            "current-branch resume should not persist a worktree path"
        );
        assert!(
            loaded.pr_url.is_none(),
            "current-branch resume should skip PR creation"
        );
        assert!(
            !repo.join("first.txt").exists(),
            "resume should continue from the saved step instead of rerunning earlier steps"
        );
        assert!(
            repo.join("second.txt").exists(),
            "resume should execute the saved current step in the base repository"
        );
        assert!(
            !gh_log.exists(),
            "current-branch resume should not invoke gh"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_current_branch_conflict_overwrite_continues_and_logs_choice() {
        // Given: a running current-branch session whose state.json is edited externally mid-run
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let session_id = "20260310140000";
        let (mut process, repo, manager) =
            setup_current_branch_conflict_session(&tmp, session_id, "overwrite external state");
        let log_path = tmp.path().join("conflict-overwrite.log");
        configure_conflict_test_env(&mut process, true, Some("overwrite"), &log_path);

        // When: the run reaches the next step after an external edit and the user chooses overwrite
        let run_fut = run(run_args(session_id));
        let mutate_fut = mutate_state_after_first_step(
            &manager,
            session_id,
            |_| repo.clone(),
            write_external_failed_state,
        );
        let (result, ()) = tokio::join!(run_fut, mutate_fut);

        // Then: the run completes using the in-memory state and records the conflict decision
        assert!(
            result.is_ok(),
            "overwrite choice should allow the run to continue: {result:?}"
        );
        let loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(matches!(loaded.phase, SessionPhase::Completed));
        assert_eq!(loaded.current_step.as_deref(), Some("second"));
        assert!(repo.join("second.txt").exists());
        let log = fs::read_to_string(&log_path).unwrap_or_else(|e| {
            panic!("conflict resolution should be logged for overwrite tests: {e:?}")
        });
        assert!(
            log.contains("overwrite"),
            "expected overwrite decision in log, got: {log}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_current_branch_conflict_abort_preserves_external_state() {
        // Given: a running current-branch session whose state.json is edited externally mid-run
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let session_id = "20260310140001";
        let (mut process, repo, manager) =
            setup_current_branch_conflict_session(&tmp, session_id, "abort on conflict");
        let log_path = tmp.path().join("conflict-abort.log");
        configure_conflict_test_env(&mut process, true, Some("abort"), &log_path);

        // When: the run reaches the next step after an external edit and the user chooses abort
        let run_fut = run(run_args(session_id));
        let mutate_fut = mutate_state_after_first_step(
            &manager,
            session_id,
            |_| repo.clone(),
            write_external_failed_state,
        );
        let (result, ()) = tokio::join!(run_fut, mutate_fut);

        // Then: the run stops, leaves the external state untouched, and does not execute later steps
        match result {
            Err(CruiseError::SessionStateConflictAborted(message)) => {
                assert!(
                    message.contains("state.json"),
                    "abort message should mention state.json: {message}"
                );
            }
            other => panic!("expected SessionStateConflictAborted, got {other:?}"),
        }
        let loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(loaded.current_step.as_deref(), Some("external-step"));
        assert!(matches!(
            loaded.phase,
            SessionPhase::Failed(ref message) if message == "external edit"
        ));
        assert!(
            !repo.join("second.txt").exists(),
            "aborting on conflict should prevent later steps from running"
        );
        let log = fs::read_to_string(&log_path).unwrap_or_else(|e| {
            panic!("conflict resolution should be logged for abort tests: {e:?}")
        });
        assert!(
            log.contains("abort"),
            "expected abort decision in log, got: {log}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_current_branch_conflict_noninteractive_returns_error_without_prompt() {
        // Given: a running session with an external state edit and stdin treated as non-terminal
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let session_id = "20260310140002";
        let (mut process, repo, manager) =
            setup_current_branch_conflict_session(&tmp, session_id, "noninteractive conflict");
        let log_path = tmp.path().join("conflict-noninteractive.log");
        configure_conflict_test_env(&mut process, false, None, &log_path);

        // When: the run hits the conflicting save point in noninteractive mode
        let run_fut = run(run_args(session_id));
        let mutate_fut = mutate_state_after_first_step(
            &manager,
            session_id,
            |_| repo.clone(),
            write_external_failed_state,
        );
        let (result, ()) = tokio::join!(run_fut, mutate_fut);

        // Then: the run errors immediately and preserves the externally edited state
        match result {
            Err(CruiseError::SessionStateConflict(message)) => {
                assert!(
                    message.contains("state.json"),
                    "noninteractive conflict should mention state.json: {message}"
                );
            }
            other => panic!("expected SessionStateConflict, got {other:?}"),
        }
        let loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(loaded.current_step.as_deref(), Some("external-step"));
        assert!(matches!(
            loaded.phase,
            SessionPhase::Failed(ref message) if message == "external edit"
        ));
        assert!(
            !repo.join("second.txt").exists(),
            "noninteractive conflicts should stop before later steps run"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_current_branch_conflict_abort_preserves_invalid_state_file() {
        // Given: a running session whose state.json becomes invalid JSON before the next save
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let session_id = "20260310140003";
        let (mut process, repo, manager) =
            setup_current_branch_conflict_session(&tmp, session_id, "invalid json conflict");
        let log_path = tmp.path().join("conflict-invalid-json.log");
        configure_conflict_test_env(&mut process, true, Some("abort"), &log_path);

        // When: the run reaches the conflicting save point and the user aborts
        let run_fut = run(run_args(session_id));
        let mutate_fut = mutate_state_after_first_step(
            &manager,
            session_id,
            |_| repo.clone(),
            |manager, id| {
                fs::write(manager.state_path(id), "{invalid json")
                    .unwrap_or_else(|e| panic!("{e:?}"));
            },
        );
        let (result, ()) = tokio::join!(run_fut, mutate_fut);

        // Then: the invalid external file is preserved and later steps do not run
        match result {
            Err(CruiseError::SessionStateConflictAborted(message)) => {
                assert!(
                    message.contains("state.json"),
                    "abort message should mention state.json: {message}"
                );
            }
            other => panic!("expected SessionStateConflictAborted, got {other:?}"),
        }
        assert_eq!(
            fs::read_to_string(manager.state_path(session_id)).unwrap_or_else(|e| panic!("{e:?}")),
            "{invalid json"
        );
        assert!(
            !repo.join("second.txt").exists(),
            "aborting on invalid external JSON should stop before later steps run"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_current_branch_conflict_noninteractive_preserves_missing_state_file() {
        // Given: a running session whose state.json is deleted before the next save
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let session_id = "20260310140004";
        let (mut process, repo, manager) =
            setup_current_branch_conflict_session(&tmp, session_id, "missing state conflict");
        let log_path = tmp.path().join("conflict-missing.log");
        configure_conflict_test_env(&mut process, false, None, &log_path);

        // When: the run reaches the conflicting save point in noninteractive mode
        let run_fut = run(run_args(session_id));
        let mutate_fut = mutate_state_after_first_step(
            &manager,
            session_id,
            |_| repo.clone(),
            |manager, id| {
                fs::remove_file(manager.state_path(id)).unwrap_or_else(|e| panic!("{e:?}"));
            },
        );
        let (result, ()) = tokio::join!(run_fut, mutate_fut);

        // Then: the run returns a conflict error and leaves the file deleted
        match result {
            Err(CruiseError::SessionStateConflict(message)) => {
                assert!(
                    message.contains("state.json"),
                    "missing-file conflict should mention state.json: {message}"
                );
            }
            other => panic!("expected SessionStateConflict, got {other:?}"),
        }
        assert!(
            !manager.state_path(session_id).exists(),
            "noninteractive conflict should preserve the missing state file"
        );
        assert!(
            !repo.join("second.txt").exists(),
            "noninteractive missing-file conflicts should stop before later steps run"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_all_forces_worktree_even_for_current_branch_sessions() {
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260309125000";
        let session = make_current_branch_session(session_id, &repo, "batch run", "main");
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(
            &manager,
            session_id,
            &single_command_config("edit", "printf batch > run-all.txt"),
        );

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/104");
        process.prepend_path(&bin_dir);

        let result = run(RunArgs {
            session: None,
            all: true,
            max_retries: 10,
            rate_limit_retries: 0,
            dry_run: false,
        })
        .await;
        assert!(result.is_ok(), "expected run --all to succeed: {result:?}");

        let loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            loaded.worktree_path.is_some(),
            "run --all should still use a worktree execution path"
        );
        assert!(
            loaded.worktree_branch.is_some(),
            "run --all should assign a worktree branch"
        );
        assert_eq!(
            loaded.pr_url.as_deref(),
            Some("https://github.com/owner/repo/pull/104"),
            "run --all should keep the existing PR flow"
        );
        assert!(
            !repo.join("run-all.txt").exists(),
            "run --all should not write changes into the base repository"
        );
        assert!(
            manager
                .worktrees_dir()
                .join(session_id)
                .join("run-all.txt")
                .exists(),
            "run --all should write changes inside the session worktree"
        );
        let gh_log_contents = fs::read_to_string(&gh_log).unwrap_or_default();
        assert!(
            gh_log_contents.contains("pr create --head"),
            "run --all should still invoke PR creation through gh"
        );
        assert!(
            gh_log_contents.contains("--draft"),
            "gh pr create should include --draft flag, got: {gh_log_contents}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_all_preserves_invalid_external_state_without_failing_summary_reload() {
        // Given: a planned session that will abort on a state.json conflict and leave invalid JSON
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let mut process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260310140005";
        let mut session = SessionState::new(
            session_id.to_string(),
            repo.clone(),
            "cruise.yaml".to_string(),
            "run all conflict".to_string(),
        );
        session.phase = SessionPhase::Planned;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(&manager, session_id, &blocking_conflict_config());

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/105");
        process.prepend_path(&bin_dir);

        let log_path = tmp.path().join("run-all-conflict.log");
        configure_conflict_test_env(&mut process, true, Some("abort"), &log_path);

        // When: run --all hits the conflict, aborts that session, and tries to build its summary
        let run_fut = run(RunArgs {
            session: None,
            all: true,
            max_retries: 10,
            rate_limit_retries: 0,
            dry_run: false,
        });
        let mutate_fut = mutate_state_after_first_step(
            &manager,
            session_id,
            |state| {
                state.worktree_path.clone().unwrap_or_else(|| {
                    panic!("run --all should persist a worktree path before step execution")
                })
            },
            |manager, id| {
                fs::write(manager.state_path(id), "{invalid json")
                    .unwrap_or_else(|e| panic!("{e:?}"));
            },
        );
        let (result, ()) = tokio::join!(run_fut, mutate_fut);

        // Then: run --all still returns Ok and leaves the invalid external file untouched
        assert!(
            result.is_ok(),
            "run --all should not fail when summary reload sees preserved invalid state: {result:?}"
        );
        assert_eq!(
            fs::read_to_string(manager.state_path(session_id)).unwrap_or_else(|e| panic!("{e:?}")),
            "{invalid json"
        );
        assert!(
            !gh_log.exists(),
            "the aborted conflict session should not reach PR creation"
        );
    }

    // -----------------------------------------------------------------------
    // format_run_all_summary() unit tests
    // -----------------------------------------------------------------------

    fn make_session(input: &str, phase: SessionPhase, pr_url: Option<&str>) -> SessionState {
        let mut s = SessionState::new(
            "20260101000000".to_string(),
            std::path::PathBuf::from("/tmp"),
            "test.yaml".to_string(),
            input.to_string(),
        );
        s.phase = phase;
        s.pr_url = pr_url.map(str::to_string);
        s
    }

    // -----------------------------------------------------------------------
    // format_run_all_summary -- Suspended
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_run_all_summary_suspended_session_shows_suspended_indicator() {
        // Given: a Suspended session is included in the results
        let results = vec![make_session("add feature", SessionPhase::Suspended, None)];

        // When
        let summary = format_run_all_summary(&results);
        let summary_plain = console::strip_ansi_codes(&summary).to_string();

        // Then: the session's input and "Suspended" marker are included
        assert!(
            summary_plain.contains("add feature"),
            "summary should contain input: {summary_plain}"
        );
        assert!(
            summary_plain.contains("Suspended"),
            "summary should contain Suspended indicator: {summary_plain}"
        );
        assert!(
            !summary_plain.is_empty(),
            "summary should not be empty: {summary_plain}"
        );
    }

    #[test]
    fn test_format_run_all_summary_mixed_with_suspended() {
        // Given: mixed results of Completed, Suspended, and Failed
        let results = vec![
            make_session(
                "task a",
                SessionPhase::Completed,
                Some("https://github.com/org/repo/pull/1"),
            ),
            make_session("task b", SessionPhase::Suspended, None),
            make_session(
                "task c",
                SessionPhase::Failed("build error".to_string()),
                None,
            ),
        ];

        // When
        let summary = format_run_all_summary(&results);
        let summary_plain = console::strip_ansi_codes(&summary).to_string();

        // Then: information for all 3 sessions is included and the header count is correct
        assert!(
            summary_plain.contains("task a"),
            "summary should contain task a: {summary_plain}"
        );
        assert!(
            summary_plain.contains("task b"),
            "summary should contain task b: {summary_plain}"
        );
        assert!(
            summary_plain.contains("task c"),
            "summary should contain task c: {summary_plain}"
        );
        assert!(
            summary_plain.contains("=== Run All Summary (3 sessions) ==="),
            "header should show correct count: {summary_plain}"
        );
        assert!(
            summary_plain.contains("Suspended"),
            "summary should distinguish Suspended from Failed: {summary_plain}"
        );
        assert!(
            summary_plain.contains("Failed"),
            "summary should show Failed reason: {summary_plain}"
        );
    }

    #[test]
    fn test_format_run_all_summary_empty_returns_empty_string() {
        // Given: empty result list
        let results: Vec<SessionState> = vec![];

        // When
        let summary = format_run_all_summary(&results);

        // Then: returns empty string
        assert!(
            summary.is_empty(),
            "expected empty string, got: {summary:?}"
        );
    }

    #[test]
    fn test_format_run_all_summary_single_completed_with_pr() {
        // Given: Completed session with PR URL
        let results = vec![make_session(
            "add login feature",
            SessionPhase::Completed,
            Some("https://github.com/org/repo/pull/42"),
        )];

        // When
        let summary = format_run_all_summary(&results);

        // Then: summary contains input and PR URL
        assert!(
            summary.contains("add login feature"),
            "summary should contain input: {summary}"
        );
        assert!(
            summary.contains("https://github.com/org/repo/pull/42"),
            "summary should contain PR URL: {summary}"
        );
        assert!(
            !summary.contains("Failed") && !summary.contains("x"),
            "completed session should not show failure: {summary}"
        );
    }

    #[test]
    fn test_format_run_all_summary_single_completed_without_pr() {
        // Given: Completed session without PR URL
        let results = vec![make_session(
            "refactor database layer",
            SessionPhase::Completed,
            None,
        )];

        // When
        let summary = format_run_all_summary(&results);

        // Then: summary contains input without failure indicators
        assert!(
            summary.contains("refactor database layer"),
            "summary should contain input: {summary}"
        );
        assert!(
            !summary.contains("Failed") && !summary.contains("x"),
            "completed session should not show failure: {summary}"
        );
    }

    #[test]
    fn test_format_run_all_summary_single_failed_session() {
        // Given: Failed session with an error message
        let results = vec![make_session(
            "fix login bug",
            SessionPhase::Failed("CI timeout".to_string()),
            None,
        )];

        // When
        let summary = format_run_all_summary(&results);

        // Then: summary contains input, failure indicator and error message
        assert!(
            summary.contains("fix login bug"),
            "summary should contain input: {summary}"
        );
        assert!(
            summary.contains("Failed: CI timeout"),
            "summary should contain failure prefix and error message: {summary}"
        );
    }

    #[test]
    fn test_format_run_all_summary_mixed_results() {
        // Given: mixed Completed and Failed sessions
        let results = vec![
            make_session(
                "add auth module",
                SessionPhase::Completed,
                Some("https://github.com/org/repo/pull/10"),
            ),
            make_session(
                "fix broken test",
                SessionPhase::Failed("build error".to_string()),
                None,
            ),
        ];

        // When
        let summary = format_run_all_summary(&results);

        // Then: summary contains info for both sessions
        assert!(
            summary.contains("add auth module"),
            "summary should contain first input: {summary}"
        );
        assert!(
            summary.contains("https://github.com/org/repo/pull/10"),
            "summary should contain PR URL: {summary}"
        );
        assert!(
            summary.contains("fix broken test"),
            "summary should contain second input: {summary}"
        );
        assert!(
            summary.contains("Failed: build error"),
            "summary should contain failure prefix and error message for second session: {summary}"
        );
    }

    #[test]
    fn test_format_run_all_summary_mixed_with_completed_no_pr() {
        // Given: 3 sessions -- success with PR, completed without PR, and explicit failure
        let results = vec![
            make_session(
                "add auth module",
                SessionPhase::Completed,
                Some("https://github.com/org/repo/pull/10"),
            ),
            make_session("refactor cache layer", SessionPhase::Completed, None),
            make_session(
                "fix broken test",
                SessionPhase::Failed("CI timeout".to_string()),
                None,
            ),
        ];

        // When
        let summary = format_run_all_summary(&results);

        // Then: first session shows success with PR URL
        assert!(
            summary.contains("add auth module"),
            "summary should contain first session: {summary}"
        );
        assert!(
            summary.contains("https://github.com/org/repo/pull/10"),
            "summary should show PR URL for success: {summary}"
        );

        // Then: second completed session remains a success even without PR URL
        assert!(
            summary.contains("refactor cache layer"),
            "summary should contain second session: {summary}"
        );
        let refactor_line = summary
            .lines()
            .find(|l| l.contains("refactor cache layer"))
            .unwrap_or_else(|| panic!("refactor cache layer line not found in summary"));
        assert!(
            !refactor_line.contains("Failed") && !refactor_line.contains("x"),
            "completed session should not show failure, got: {refactor_line:?}"
        );

        // Then: third session shows failure prefix and error message
        let failed_line = summary
            .lines()
            .find(|l| l.contains("fix broken test"))
            .unwrap_or_else(|| panic!("fix broken test line not found in summary"));
        assert!(
            failed_line.contains("Failed: CI timeout"),
            "failed session should show failure prefix and error message, got: {failed_line:?}"
        );
    }

    #[test]
    fn test_format_run_all_summary_long_input_is_truncated() {
        // Given: completed session with a very long input
        let long_input = "a".repeat(200);
        let results = vec![make_session(&long_input, SessionPhase::Completed, None)];

        // When
        let summary = format_run_all_summary(&results);

        // Then: each summary line is within a reasonable length (300 chars max)
        for line in summary.lines() {
            assert!(
                line.chars().count() <= 300,
                "line too long ({} chars): {line}",
                line.chars().count()
            );
        }
    }

    // -----------------------------------------------------------------------
    // apply_run_result_to_session() integration tests
    // These test the finalization logic across engine -> run_cmd -> session.
    // -----------------------------------------------------------------------

    #[test]
    fn test_apply_run_result_completed_sets_completed_phase() {
        // Given: a Running session and a successful result
        let mut session = make_session("some task", SessionPhase::Running, None);
        // When: applying a successful result
        apply_run_result_to_session(&mut session, &Ok(()));
        // Then: session phase becomes Completed
        assert!(
            matches!(session.phase, SessionPhase::Completed),
            "Expected Completed, got {:?}",
            session.phase
        );
    }

    #[test]
    fn test_apply_run_result_step_paused_keeps_running_phase() {
        // Given: a Running session and a StepPaused error
        // (StepPaused means user pressed Esc -- session should be resumable)
        let mut session = make_session("some task", SessionPhase::Running, None);
        // When: applying StepPaused
        apply_run_result_to_session(&mut session, &Err(CruiseError::StepPaused));
        // Then: session stays Running so it can be resumed later
        assert!(
            matches!(session.phase, SessionPhase::Running),
            "Expected Running after StepPaused, got {:?}",
            session.phase
        );
    }

    #[test]
    fn test_apply_run_result_other_error_sets_failed_phase() {
        // Given: a Running session and a generic error
        let mut session = make_session("some task", SessionPhase::Running, None);
        // When: applying a generic command error
        apply_run_result_to_session(
            &mut session,
            &Err(CruiseError::CommandError("some failure".to_string())),
        );
        // Then: session phase becomes Failed
        assert!(
            matches!(session.phase, SessionPhase::Failed(_)),
            "Expected Failed, got {:?}",
            session.phase
        );
    }

    #[test]
    fn test_apply_run_result_step_paused_does_not_set_completed_at() {
        // Given: a session and StepPaused
        let mut session = make_session("some task", SessionPhase::Running, None);
        // When: applying StepPaused
        apply_run_result_to_session(&mut session, &Err(CruiseError::StepPaused));
        // Then: completed_at is not set (session is not finished)
        assert!(
            session.completed_at.is_none(),
            "completed_at should not be set on pause"
        );
    }

    #[test]
    fn test_apply_run_result_completed_sets_completed_at() {
        // Given: a Running session
        let mut session = make_session("some task", SessionPhase::Running, None);
        // When: applying success
        apply_run_result_to_session(&mut session, &Ok(()));
        // Then: completed_at is recorded
        assert!(
            session.completed_at.is_some(),
            "completed_at should be set on completion"
        );
    }

    #[test]
    fn test_apply_run_result_failed_sets_completed_at() {
        // Given: a Running session
        let mut session = make_session("some task", SessionPhase::Running, None);
        // When: applying a fatal error
        apply_run_result_to_session(&mut session, &Err(CruiseError::Other("fatal".to_string())));
        // Then: completed_at is recorded
        assert!(
            session.completed_at.is_some(),
            "completed_at should be set on failure"
        );
    }

    // -- prompt_workspace_mode unit tests --------------------------------

    #[test]
    fn test_prompt_workspace_mode_returns_worktree_when_noninteractive() {
        // Given: stdin is not a terminal
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let mut process = ProcessStateGuard::new(tmp.path());
        process.set_env(TEST_STDIN_IS_TERMINAL_ENV, "0");
        process.remove_env(TEST_WORKSPACE_MODE_ENV);

        // When: prompt_workspace_mode is called in non-interactive environment
        let result = prompt_workspace_mode();

        // Then: returns Worktree without showing a prompt
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")),
            WorkspaceMode::Worktree,
            "non-interactive should default to Worktree"
        );
    }

    #[test]
    fn test_prompt_workspace_mode_returns_current_branch_via_test_env() {
        // Given: test env var selects current_branch
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let mut process = ProcessStateGuard::new(tmp.path());
        process.set_env(TEST_WORKSPACE_MODE_ENV, "current_branch");

        // When: prompt_workspace_mode is called
        let result = prompt_workspace_mode();

        // Then: returns CurrentBranch as the env var dictates
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")),
            WorkspaceMode::CurrentBranch,
            "env override should select CurrentBranch"
        );
    }

    #[test]
    fn test_prompt_workspace_mode_returns_worktree_via_test_env() {
        // Given: test env var selects worktree
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let mut process = ProcessStateGuard::new(tmp.path());
        process.set_env(TEST_WORKSPACE_MODE_ENV, "worktree");

        // When: prompt_workspace_mode is called
        let result = prompt_workspace_mode();

        // Then: returns Worktree as the env var dictates
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")),
            WorkspaceMode::Worktree,
            "env override should select Worktree"
        );
    }

    // -- run_single workspace mode selection integration tests ------------

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_single_prompts_on_fresh_default_session_and_selects_current_branch() {
        // Given: a fresh session with default workspace_mode (Worktree) and no current_step
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let mut process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);
        // prompt resolves to current_branch via test env
        process.set_env(TEST_STDIN_IS_TERMINAL_ENV, "1");
        process.set_env(TEST_WORKSPACE_MODE_ENV, "current_branch");

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260321091000";
        let mut session = SessionState::new(
            session_id.to_string(),
            repo.clone(),
            "cruise.yaml".to_string(),
            "run in place".to_string(),
        );
        session.phase = SessionPhase::Planned;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(
            &manager,
            session_id,
            &single_command_config("edit", "printf in-place > in-place.txt"),
        );

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/300");
        process.prepend_path(&bin_dir);

        // When: run is called on the fresh session
        let result = run(run_args(session_id)).await;

        // Then: run succeeds and changes land in the base repo (not a worktree)
        assert!(
            result.is_ok(),
            "expected run to succeed when prompt selects current_branch: {result:?}"
        );
        assert!(
            repo.join("in-place.txt").exists(),
            "current_branch selection should write changes into the base repository"
        );
        assert!(
            !manager.worktrees_dir().join(session_id).exists(),
            "current_branch selection should not create a worktree directory"
        );
        assert!(
            !gh_log.exists(),
            "current_branch selection should not invoke gh"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_single_saves_workspace_mode_and_target_branch_after_prompt_selects_current_branch()
     {
        // Given: a fresh session with default workspace_mode (Worktree)
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let mut process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);
        process.set_env(TEST_STDIN_IS_TERMINAL_ENV, "1");
        process.set_env(TEST_WORKSPACE_MODE_ENV, "current_branch");

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260321091001";
        let mut session = SessionState::new(
            session_id.to_string(),
            repo.clone(),
            "cruise.yaml".to_string(),
            "save mode test".to_string(),
        );
        session.phase = SessionPhase::Planned;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(
            &manager,
            session_id,
            &single_command_config("edit", "printf mode > mode.txt"),
        );

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/301");
        process.prepend_path(&bin_dir);

        // When: run completes
        run(run_args(session_id))
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the persisted session state reflects the chosen mode and target branch
        let loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            loaded.workspace_mode,
            WorkspaceMode::CurrentBranch,
            "workspace_mode should be persisted as CurrentBranch after prompt selection"
        );
        assert_eq!(
            loaded.target_branch.as_deref(),
            Some("main"),
            "target_branch should be set to the current branch at run time"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_single_does_not_prompt_when_session_already_has_current_branch_mode() {
        // Given: a session with workspace_mode=CurrentBranch already set (no current_step)
        // TEST_WORKSPACE_MODE_ENV is intentionally absent -- if prompt were called, it would hang
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let mut process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);
        process.set_env(TEST_STDIN_IS_TERMINAL_ENV, "1");
        process.remove_env(TEST_WORKSPACE_MODE_ENV);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260321091002";
        let session =
            make_current_branch_session(session_id, &repo, "already current branch", "main");
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(
            &manager,
            session_id,
            &single_command_config("edit", "printf skip > skip-prompt.txt"),
        );

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/302");
        process.prepend_path(&bin_dir);

        // When: run is called
        let result = run(run_args(session_id)).await;

        // Then: run succeeds in the base repo without prompting
        assert!(
            result.is_ok(),
            "expected run to succeed without prompting: {result:?}"
        );
        assert!(
            repo.join("skip-prompt.txt").exists(),
            "already-CurrentBranch session should write changes into the base repository"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_single_does_not_prompt_when_resuming_saved_current_branch_session() {
        // Given: a session being resumed (current_step is Some) in CurrentBranch mode
        // TEST_WORKSPACE_MODE_ENV is absent -- if prompt were called, it would hang
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let mut process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);
        process.set_env(TEST_STDIN_IS_TERMINAL_ENV, "1");
        process.remove_env(TEST_WORKSPACE_MODE_ENV);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260321091003";
        let mut session =
            make_current_branch_session(session_id, &repo, "resume no prompt", "main");
        session.phase = SessionPhase::Running;
        session.current_step = Some("second".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(
            &manager,
            session_id,
            r"command:
  - cat
steps:
  first:
    command: |
      printf first > first.txt
  second:
    command: |
      printf second > second.txt
",
        );

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/303");
        process.prepend_path(&bin_dir);

        // When: run is called to resume from second step
        let result = run(run_args(session_id)).await;

        // Then: run continues from the saved step without prompting
        assert!(
            result.is_ok(),
            "expected resume to succeed without prompting: {result:?}"
        );
        assert!(
            !repo.join("first.txt").exists(),
            "resume should skip already-executed earlier steps"
        );
        assert!(
            repo.join("second.txt").exists(),
            "resume should execute from the saved current step"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_single_defaults_to_worktree_when_stdin_is_not_terminal() {
        // Given: a fresh session with default workspace_mode (Worktree) and non-interactive stdin
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let mut process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);
        process.set_env(TEST_STDIN_IS_TERMINAL_ENV, "0");
        process.remove_env(TEST_WORKSPACE_MODE_ENV);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));
        let session_id = "20260321091004";
        let mut session = SessionState::new(
            session_id.to_string(),
            repo.clone(),
            "cruise.yaml".to_string(),
            "default to worktree".to_string(),
        );
        session.phase = SessionPhase::Planned;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));
        write_config(
            &manager,
            session_id,
            &single_command_config("edit", "printf wt > wt.txt"),
        );

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/304");
        process.prepend_path(&bin_dir);

        // When: run is called in non-interactive mode
        let result = run(run_args(session_id)).await;

        // Then: run succeeds using worktree mode (non-interactive defaults to safe Worktree)
        assert!(
            result.is_ok(),
            "expected run to succeed in worktree mode: {result:?}"
        );
        let loaded = manager.load(session_id).unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(
            loaded.workspace_mode,
            WorkspaceMode::Worktree,
            "non-interactive stdin should default to Worktree mode"
        );
        assert!(
            !repo.join("wt.txt").exists(),
            "worktree mode should not write changes into the base repository"
        );
        assert!(
            manager
                .worktrees_dir()
                .join(session_id)
                .join("wt.txt")
                .exists(),
            "worktree mode should write changes into the session worktree"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_run_all_picks_up_session_added_while_first_session_is_running() {
        // Given: one Planned session with a blocking first step (blocks until proceed.txt exists)
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let process = ProcessStateGuard::new(tmp.path());
        let repo = create_repo_with_origin(&tmp);
        process.set_current_dir(&repo);

        let manager = SessionManager::new(get_cruise_home().unwrap_or_else(|e| panic!("{e:?}")));

        let session_id_1 = "20260403400000";
        let session_id_2 = "20260403400001"; // added mid-run -- newer ID

        let mut session_1 = SessionState::new(
            session_id_1.to_string(),
            repo.clone(),
            "cruise.yaml".to_string(),
            "first task".to_string(),
        );
        session_1.phase = SessionPhase::Planned;
        manager
            .create(&session_1)
            .unwrap_or_else(|e| panic!("{e:?}"));
        write_config(&manager, session_id_1, &blocking_conflict_config());

        let bin_dir = tmp.path().join("bin");
        let gh_log = tmp.path().join("gh.log");
        install_logging_gh(&bin_dir, &gh_log, "https://github.com/owner/repo/pull/201");
        process.prepend_path(&bin_dir);

        // When: run --all starts. Concurrently, add session_2 once session_1 is blocking.
        let run_fut = run(RunArgs {
            session: None,
            all: true,
            max_retries: 10,
            rate_limit_retries: 0,
            dry_run: false,
        });

        let add_and_unblock_fut = async {
            // Wait for session_1 to reach its blocking "first" step
            wait_for_session_step(&manager, session_id_1, "first").await;

            // Add session_2 as a Planned session with a simple command
            let mut session_2 = SessionState::new(
                session_id_2.to_string(),
                repo.clone(),
                "cruise.yaml".to_string(),
                "second task added mid-run".to_string(),
            );
            session_2.phase = SessionPhase::Planned;
            manager
                .create(&session_2)
                .unwrap_or_else(|e| panic!("{e:?}"));
            write_config(
                &manager,
                session_id_2,
                &single_command_config("do", "printf done2 > session2-output.txt"),
            );

            // Unblock session_1 by writing proceed.txt in its worktree
            let state_1 = manager
                .load(session_id_1)
                .unwrap_or_else(|e| panic!("{e:?}"));
            let worktree = state_1
                .worktree_path
                .clone()
                .unwrap_or_else(|| panic!("session_1 should have worktree_path set when Running"));
            fs::write(worktree.join("proceed.txt"), "go").unwrap_or_else(|e| panic!("{e:?}"));
        };

        let (result, ()) = tokio::join!(run_fut, add_and_unblock_fut);

        // Then: run --all completes without error
        assert!(
            result.is_ok(),
            "run --all should succeed even when a session is added mid-run: {result:?}"
        );

        // And: session_2 was also executed and completed
        let state_2 = manager
            .load(session_id_2)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            matches!(state_2.phase, SessionPhase::Completed),
            "session_2 added mid-run should be Completed, got {:?}",
            state_2.phase
        );
        assert!(
            manager
                .worktrees_dir()
                .join(session_id_2)
                .join("session2-output.txt")
                .exists(),
            "session_2 command should have written session2-output.txt in its worktree"
        );
    }
}
