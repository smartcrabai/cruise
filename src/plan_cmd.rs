use std::collections::HashSet;
use std::fmt;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use console::style;
use inquire::InquireError;

use std::sync::Arc;

use crate::ask_handler::{AskHandler, CliAskHandler, NoninteractiveAskHandler};
use crate::cancellation::CancellationToken;
use crate::cli::{
    DEFAULT_MAX_RETRIES, DEFAULT_RATE_LIMIT_RETRIES, PLAN_STDIN_SENTINEL, PlanArgs, PlanWorkerArgs,
};
use crate::config::{WorkflowConfig, validate_config};
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};
use crate::new_session_history::{
    BUILTIN_CONFIG_KEY, HistoryScope, NewSessionHistory, resolved_config_key_for_session,
};
use crate::planning::{
    PlanPromptCtx, ask_plan_template, fix_plan_template, initial_plan_template, setup_plan_vars,
};
use crate::resolver::ConfigSource;
use crate::session::{SessionManager, SessionPhase, SessionState};
use crate::variable::VariableStore;
use crate::workflow::{SkippableStepNode, list_skippable_after_pr_steps, list_skippable_steps};

/// Build a CLI planning context (interactive prompts via [`CliAskHandler`]).
fn cli_plan_ctx<'a>(
    config: &'a WorkflowConfig,
    plan_path: &'a Path,
    working_dir: Option<&'a Path>,
    interactive: bool,
    rate_limit_retries: usize,
    grill: bool,
    cancel_token: Option<&'a CancellationToken>,
) -> PlanPromptCtx<'a> {
    // Only the interactive approve loop can prompt the user; non-TTY contexts use
    // a handler that errors rather than blocking on stdin (ask_user is not
    // registered there anyway).
    let ask: Arc<dyn AskHandler> = if interactive {
        Arc::new(CliAskHandler)
    } else {
        Arc::new(NoninteractiveAskHandler)
    };
    PlanPromptCtx {
        config,
        ask,
        plan_path,
        interactive,
        rate_limit_retries,
        working_dir,
        grill,
        cancel_token,
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "plan-creation flow: input + dry-run + attachments + worktree + plan + approve loop"
)]
pub async fn run(args: PlanArgs) -> Result<()> {
    // Resolve config first so the path is visible before prompting for input.
    // For repo sessions the config lives in the temporary clone, which doesn't
    // exist yet; resolution is deferred until after cloning.
    let target = resolve_plan_target(args.repo.as_deref(), args.config.as_deref())?;

    // noninteractive is true whenever stdin is not a terminal (pipe, redirect,
    // or backward-compat path where cli.rs already consumed stdin and placed
    // the content in args.input).  This prevents inquire from attempting to
    // read interactive input from a non-TTY file descriptor.
    let noninteractive = !std::io::stdin().is_terminal();

    // Grill mode interviews the user via interactive prompts, so it cannot run in
    // a non-TTY context. Fail before creating any session.
    if args.grill && noninteractive {
        return Err(CruiseError::Other(
            "--grill requires an interactive terminal (it interviews you one \
             question at a time); it cannot run in a non-TTY context"
                .to_string(),
        ));
    }

    let (raw_input, from_interactive) = read_plan_input(args.input, noninteractive)?;
    // Auto-detect image paths only from the interactive prompt (Claude-Code-like
    // drag-and-drop). For arg/stdin input, treat the text as opaque so prose
    // mentioning a path like "what changed in /tmp/x.png" stays intact.
    let (input, mut images) = if from_interactive {
        crate::attachments::extract_image_paths(&raw_input)
    } else {
        (raw_input, vec![])
    };
    for p in &args.images {
        images.push(PathBuf::from(p));
    }

    if args.dry_run {
        eprintln!(
            "{}",
            style(format!("Would plan: \"{}\"", input.trim())).dim()
        );
        if !images.is_empty() {
            for img in &images {
                eprintln!(
                    "{}",
                    style(format!("  attached image: {}", img.display())).dim()
                );
            }
        }
        return Ok(());
    }

    let manager = SessionManager::new(crate::paths::data_dir()?);
    let (mut config, mut session) =
        create_session_for_target(&manager, target, args.config.as_deref(), input.trim())?;

    if let Err(e) = attach_images_or_discard(&manager, &mut session, &images) {
        eprintln!(
            "\n{} Failed to attach images. Session {} discarded.",
            style("✗").red().bold(),
            session.id
        );
        return Err(e);
    }

    if args.no_interactive_planning {
        config.interactive_planning = false;
    }

    // Grill mode relies on the SDK `ask_user` tool, which is only registered in
    // the interactive tool-based planning flow. Reject when the SDK backend is
    // absent or when `interactive_planning` is disabled, discarding the session
    // we just created.
    if args.grill && !crate::planning::sdk_plan_tools_enabled(&config) {
        if let Err(del_err) = manager.delete(&session.id) {
            eprintln!("warning: failed to clean up session: {del_err}");
        }
        return Err(CruiseError::Other(
            "--grill requires the SDK backend with interactive planning enabled \
             (`sdk:` must be set and `interactive_planning` must not be disabled); \
             the command backend and tool-less planning have no interactive \
             ask_user tool"
                .to_string(),
        ));
    }

    setup_planning_worktree_or_discard(&manager, &mut session)?;

    // Set up variables with the session plan path.
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = setup_plan_vars(session.input_with_attachments(), plan_path.clone(), &config);

    // SDK session id, shared across the plan / fix / ask turns so they resume the
    // same conversation. Stays `None` in command mode.
    let mut resume: Option<String> = None;
    let interactive = !noninteractive;

    if args.skip_planning {
        // Write the augmented input so the saved plan references the attached
        // images; the LLM running steps will pick those up via plan.md.
        let plan_content = session.input_with_attachments();
        if let Err(e) = crate::planning::write_input_as_plan(&plan_path, &plan_content) {
            eprintln!(
                "\n{} Failed to write input as plan. Session {} discarded.",
                style("✗").red().bold(),
                session.id
            );
            cleanup_discarded_session_workspace(&manager, &session);
            if let Err(del_err) = manager.delete(&session.id) {
                eprintln!("warning: failed to clean up session: {del_err}");
            }
            return Err(e);
        }
    } else {
        let work_dir = plan_working_dir(&session).to_path_buf();
        let cancel_token = CancellationToken::new();
        let ctx = cli_plan_ctx(
            &config,
            &plan_path,
            Some(&work_dir),
            interactive,
            args.rate_limit_retries,
            args.grill,
            Some(&cancel_token),
        );
        let plan_result = tokio::select! {
            result = generate_plan_markdown(&ctx, &mut vars, &mut resume) => result,
            _ = tokio::signal::ctrl_c() => {
                cancel_token.cancel();
                Err(CruiseError::Interrupted)
            },
        };
        if let Err(e) = plan_result {
            eprintln!(
                "\n{} Plan generation failed. Session {} discarded.",
                style("✗").red().bold(),
                session.id
            );
            cleanup_discarded_session_workspace(&manager, &session);
            if let Err(del_err) = manager.delete(&session.id) {
                eprintln!("warning: failed to clean up session: {del_err}");
            }
            return Err(e);
        }
    }

    // Approve-plan loop.
    run_approve_loop(
        &config,
        &manager,
        &mut session,
        &plan_path,
        &mut vars,
        args.rate_limit_retries,
        noninteractive,
        &mut resume,
    )
    .await
}

pub fn launch_background_plan(
    plan_input: &str,
    skip_planning: bool,
    repo: Option<&str>,
    images: &[String],
) -> Result<()> {
    let target = resolve_plan_target(repo, None)?;

    // Background planning input never comes from the interactive prompt, so
    // skip text-based path extraction (which would shadow legitimate prose
    // mentions of file paths) and accept image attachments only via `--image`.
    let input = read_background_plan_input(plan_input)?;
    let mut detected_images: Vec<PathBuf> = Vec::with_capacity(images.len());
    for p in images {
        detected_images.push(PathBuf::from(p));
    }

    let manager = SessionManager::new(crate::paths::data_dir()?);
    let (_config, mut session) = create_session_for_target(&manager, target, None, &input)?;
    attach_images_or_discard(&manager, &mut session, &detected_images)?;
    setup_planning_worktree_or_discard(&manager, &mut session)?;

    if skip_planning {
        let plan_path = session.plan_path(&manager.sessions_dir());
        let plan_content = session.input_with_attachments();
        let write_result = crate::planning::write_input_as_plan(&plan_path, &plan_content);
        match write_result {
            Ok(content) => {
                crate::metadata::refresh_session_title_from_plan(&mut session, &content);
                if let Err(e) = finalize_skip_planning_session(&manager, &mut session) {
                    cleanup_discarded_session_workspace(&manager, &session);
                    if let Err(del_err) = manager.delete(&session.id) {
                        eprintln!("warning: failed to clean up session: {del_err}");
                    }
                    return Err(e);
                }
            }
            Err(e) => {
                cleanup_discarded_session_workspace(&manager, &session);
                if let Err(del_err) = manager.delete(&session.id) {
                    eprintln!("warning: failed to clean up session: {del_err}");
                }
                return Err(e);
            }
        }
        eprintln!(
            "\n{} Session {} created (input used as plan).",
            style("✓").green().bold(),
            session.id
        );
        eprintln!("  Check status with: {}", style("cruise list").cyan());
        eprintln!(
            "  Run with: {}",
            style(format!("cruise run {}", session.id)).cyan()
        );
    } else {
        spawn_plan_worker(&session.id, DEFAULT_RATE_LIMIT_RETRIES)?;
        eprintln!(
            "\n{} Session {} created. Planning in background.",
            style("✓").green().bold(),
            session.id
        );
        eprintln!("  Check status with: {}", style("cruise list").cyan());
        eprintln!(
            "  Run once ready: {}",
            style(format!("cruise run {}", session.id)).cyan()
        );
    }
    Ok(())
}

fn finalize_skip_planning_session(
    manager: &SessionManager,
    session: &mut SessionState,
) -> Result<()> {
    session.approve(); // AwaitingApproval -> Planned, clears plan_error
    crate::repo_clone::cleanup_after_approval(manager, session);
    manager.save(session)
}

pub async fn run_plan_worker(args: PlanWorkerArgs) -> Result<()> {
    let manager = SessionManager::new(crate::paths::data_dir()?);
    let mut session = match manager.load(&args.session) {
        Ok(session) => session,
        Err(CruiseError::SessionError(_)) => return Ok(()),
        Err(err) => return Err(err),
    };
    session.plan_error = None;
    manager.save(&session)?;

    let result = generate_plan_for_session(&manager, &session, args.rate_limit_retries).await;
    match result {
        Ok(plan_markdown) => {
            crate::metadata::refresh_session_title_from_plan(&mut session, &plan_markdown);
            session.plan_error = None;
            manager.save(&session)?;
            Ok(())
        }
        Err(err) => {
            let plan_error = err.to_string();
            session.plan_error = Some(plan_error.clone());
            manager.save(&session)?;
            Err(CruiseError::Other(plan_error))
        }
    }
}

/// Read task input from CLI arg, piped stdin, or interactive prompt. The
/// returned `from_interactive` flag is true only when the text came from the
/// `reedline` prompt — callers use it to decide whether to auto-detect image
/// paths in the body (a drag-and-drop UX) versus preserving piped/arg text
/// verbatim.
fn read_plan_input(input: Option<String>, noninteractive: bool) -> Result<(String, bool)> {
    let stdin_input = if input.is_none() && noninteractive {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(CruiseError::IoError)?;
        Some(s)
    } else {
        None
    };
    let from_arg_or_stdin = input.is_some() || stdin_input.is_some();
    let text = resolve_input(input, stdin_input, || {
        if noninteractive {
            return Err(CruiseError::Other(
                "no input provided: stdin is not a terminal and no --input flag was given"
                    .to_string(),
            ));
        }
        prompt_for_plan_input()
    })?;
    Ok((text, !from_arg_or_stdin))
}

fn read_background_plan_input(input: &str) -> Result<String> {
    if input == PLAN_STDIN_SENTINEL {
        if std::io::stdin().is_terminal() {
            return Err(CruiseError::Other(format!(
                "--plan {PLAN_STDIN_SENTINEL} requires piped stdin"
            )));
        }
        let mut stdin_input = String::new();
        std::io::stdin()
            .read_to_string(&mut stdin_input)
            .map_err(CruiseError::IoError)?;
        let trimmed = stdin_input.trim().to_string();
        if trimmed.is_empty() {
            return Err(CruiseError::Other(
                "no input provided on stdin for background planning".to_string(),
            ));
        }
        return Ok(trimmed);
    }

    let trimmed = input.trim().to_string();
    if trimmed.is_empty() {
        return Err(CruiseError::Other(
            "background planning input cannot be empty".to_string(),
        ));
    }
    Ok(trimmed)
}

async fn approve_with_title(
    session: &mut SessionState,
    manager: &SessionManager,
    config: &WorkflowConfig,
    plan_content: &str,
    cancel_token: Option<&CancellationToken>,
) -> Result<()> {
    if config.sdk.is_some() {
        match generate_title_via_sdk(config, &session.input, plan_content, cancel_token).await {
            Ok(title) => session.title = Some(title),
            Err(CruiseError::Interrupted) => return Err(CruiseError::Interrupted),
            Err(e) => {
                eprintln!("warning: SDK title generation failed: {e}");
                crate::metadata::refresh_session_title_from_plan(session, plan_content);
            }
        }
    } else {
        crate::metadata::refresh_session_title_from_plan(session, plan_content);
    }
    session.approve();
    crate::repo_clone::cleanup_after_approval(manager, session);
    manager.save(session)
}

async fn generate_title_via_sdk(
    config: &WorkflowConfig,
    input: &str,
    plan_content: &str,
    cancel_token: Option<&CancellationToken>,
) -> Result<String> {
    let title_store = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let tool = crate::sdk_tools::generate_title_tool(std::sync::Arc::clone(&title_store));

    let executor = crate::executor::Executor::new(config.sdk.as_deref(), &config.command);
    let model_or_mode =
        executor.plan_model_or_mode(config.plan_model.as_deref(), config.model.as_deref());
    let prompt = format!(
        "Generate a concise session title (max 80 chars) for this task and plan. \
         Call the generate_title tool with your title.\n\n\
         Task: {input}\n\nPlan:\n{plan_content}"
    );
    let env = std::collections::HashMap::new();
    executor
        .run(crate::executor::PromptRun {
            prompt: &prompt,
            model_or_mode: model_or_mode.as_deref(),
            max_retries: 1,
            env: &env,
            on_retry: None,
            cancel_token,
            working_dir: None,
            stream: None,
            tools: vec![tool],
            resume: None,
        })
        .await?;

    title_store
        .lock()
        .map_err(|e| CruiseError::Other(format!("title store lock poisoned: {e}")))?
        .clone()
        .ok_or_else(|| CruiseError::Other("SDK agent did not call generate_title tool".to_string()))
}

/// Return the directory where plan-related LLM calls should run.
///
/// If a planning worktree was created the LLM executes inside it, keeping the
/// original working copy clean.  Falls back to `base_dir` for non-git repos.
fn plan_working_dir(session: &SessionState) -> &Path {
    session
        .worktree_path
        .as_deref()
        .unwrap_or(session.base_dir.as_path())
}

/// Create a planning worktree for `session` and record its path/branch.
///
/// On success the session is saved with `worktree_path` and `worktree_branch`
/// set.  If `base_dir` is not a git repository the call succeeds silently and
/// the session keeps `worktree_path = None` (plan runs in `base_dir`).
fn setup_planning_worktree(manager: &SessionManager, session: &mut SessionState) -> Result<()> {
    let worktrees_dir = manager.worktrees_dir();
    match crate::worktree::setup_session_worktree(
        &session.base_dir,
        &session.id,
        &session.input,
        &worktrees_dir,
        session.worktree_branch.as_deref(),
    ) {
        Ok((ctx, _reused)) => {
            eprintln!(
                "{} worktree: {} (planning)",
                style("->").cyan(),
                ctx.path.display()
            );
            session.worktree_path = Some(ctx.path.clone());
            session.worktree_branch = Some(ctx.branch.clone());
            manager.save(session)?;
        }
        Err(CruiseError::NotGitRepository) => {
            eprintln!("warning: not a git repository; planning in base directory");
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

/// [`setup_planning_worktree`], discarding the freshly created session (and
/// its temporary clone, for repo-backed sessions) when worktree setup fails so
/// nothing leaks on the error path.
fn setup_planning_worktree_or_discard(
    manager: &SessionManager,
    session: &mut SessionState,
) -> Result<()> {
    setup_planning_worktree(manager, session).inspect_err(|_| {
        eprintln!(
            "\n{} Worktree setup failed. Session {} discarded.",
            style("✗").red().bold(),
            session.id
        );
        cleanup_discarded_session_workspace(manager, session);
        if let Err(del_err) = manager.delete(&session.id) {
            eprintln!("warning: failed to clean up session: {del_err}");
        }
    })
}

/// Clean up the planning worktree if one was created.  Failures are logged as
/// warnings and do not propagate — cleanup is best-effort.
fn cleanup_planning_worktree(session: &SessionState) {
    if let Some(ctx) = session.worktree_context()
        && let Err(e) = crate::worktree::cleanup_worktree(&ctx)
    {
        eprintln!("warning: failed to clean up planning worktree: {e}");
    }
}

fn create_planning_session(
    manager: &SessionManager,
    source: &ConfigSource,
    input: String,
) -> Result<SessionState> {
    let session_id = SessionManager::new_session_id();
    let base_dir = std::env::current_dir()?;
    let mut session =
        SessionState::new(session_id.clone(), base_dir, source.display_string(), input);
    session.config_path = source.path().cloned();
    manager.create(&session)?;

    Ok(session)
}

/// Where a new plan session sources its working copy and config from.
enum PlanTarget {
    /// Plan in the current directory using an already-resolved config.
    Local { yaml: String, source: ConfigSource },
    /// Clone `owner/repo` into a temporary directory and plan there.
    Repo(String),
}

/// Validate `--repo` and resolve the workflow config for the upcoming session.
///
/// For local sessions the config is resolved (and displayed) immediately; for
/// repo sessions only the spec is validated -- the config lives in the
/// temporary clone, which is created later.
fn resolve_plan_target(repo: Option<&str>, explicit_config: Option<&str>) -> Result<PlanTarget> {
    match repo.map(str::trim) {
        Some(spec) if !spec.is_empty() => {
            crate::repo_clone::validate_repo_spec(spec)?;
            crate::worktree_pr::ensure_gh_available()?;
            Ok(PlanTarget::Repo(spec.to_string()))
        }
        _ => {
            let (yaml, source) = crate::resolver::resolve_config(explicit_config)?;
            eprintln!("{}", style(source.display_string()).dim());
            Ok(PlanTarget::Local { yaml, source })
        }
    }
}

/// Create the planning session (and, for repo targets, the temporary clone)
/// for `target`, returning the parsed config alongside the session.
fn create_session_for_target(
    manager: &SessionManager,
    target: PlanTarget,
    explicit_config: Option<&str>,
    input: &str,
) -> Result<(WorkflowConfig, SessionState)> {
    match target {
        PlanTarget::Local { yaml, source } => {
            let config = match source.path() {
                Some(path) => crate::workflow_call::resolve_workflow_calls_from_path(path)?,
                None => crate::workflow_call::resolve_workflow_calls(
                    WorkflowConfig::from_yaml(&yaml)
                        .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?,
                    std::env::current_dir()?,
                )?,
            };
            validate_config(&config)?;
            let session = create_planning_session(manager, &source, input.to_string())?;
            Ok((config, session))
        }
        PlanTarget::Repo(repo) => {
            create_repo_planning_session(manager, &repo, explicit_config, input.to_string())
        }
    }
}

/// Clone `repo` into the session clone directory and create a session whose
/// `base_dir` points at the clone. On failure the clone and the session
/// directory are removed.
fn create_repo_planning_session(
    manager: &SessionManager,
    repo: &str,
    explicit_config: Option<&str>,
    input: String,
) -> Result<(WorkflowConfig, SessionState)> {
    let session_id = SessionManager::new_session_id();
    let clone_path = manager.clones_dir().join(&session_id);
    eprintln!("{} cloning {} (planning)...", style("->").cyan(), repo);
    crate::repo_clone::clone_repo(repo, &clone_path)?;

    let result = build_repo_planning_session(
        manager,
        repo,
        explicit_config,
        input,
        &session_id,
        &clone_path,
    );
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&clone_path);
        if let Err(del_err) = manager.delete(&session_id) {
            eprintln!("warning: failed to clean up session: {del_err}");
        }
    }
    result
}

fn build_repo_planning_session(
    manager: &SessionManager,
    repo: &str,
    explicit_config: Option<&str>,
    input: String,
    session_id: &str,
    clone_path: &Path,
) -> Result<(WorkflowConfig, SessionState)> {
    let (yaml, source) = crate::resolver::resolve_config_in_dir(explicit_config, clone_path)?;
    eprintln!("{}", style(source.display_string()).dim());
    let config = match source.path() {
        Some(path) => crate::workflow_call::resolve_workflow_calls_from_path(path)?,
        None => crate::workflow_call::resolve_workflow_calls(
            WorkflowConfig::from_yaml(&yaml)
                .map_err(|e| CruiseError::ConfigParseError(e.to_string()))?,
            clone_path,
        )?,
    };
    validate_config(&config)?;

    let mut session = SessionState::new(
        session_id.to_string(),
        clone_path.to_path_buf(),
        source.display_string(),
        input,
    );
    session.repo = Some(repo.to_string());
    // Configs that live inside the clone (or the builtin default) are copied
    // into the session directory so they stay readable after the clone is
    // removed at approval time.
    session.config_path = crate::repo_clone::persistent_config_path(&source, clone_path);
    manager.create(&session)?;
    if session.config_path.is_none() {
        let session_dir = manager.sessions_dir().join(session_id);
        let config_to_persist = serde_yaml::to_string(&config).map_err(|e| {
            CruiseError::Other(format!(
                "failed to serialize resolved workflow config for session: {e}"
            ))
        })?;
        std::fs::write(session_dir.join("config.yaml"), config_to_persist)?;
    }
    Ok((config, session))
}

/// Copy `images` into the freshly-created `session`'s attachments directory
/// and persist the stored paths on `session.attachments`. On any failure the
/// session (and, for repo-backed sessions, its temporary clone) is removed
/// before propagating the error so nothing leaks behind a half-attached
/// session. No-op when `images` is empty.
fn attach_images_or_discard(
    manager: &SessionManager,
    session: &mut SessionState,
    images: &[PathBuf],
) -> Result<()> {
    if images.is_empty() {
        return Ok(());
    }
    let session_dir = manager.sessions_dir().join(&session.id);
    match crate::attachments::copy_images_into_session(&session_dir, images) {
        Ok(stored) => {
            session.attachments = stored;
            manager.save(session)
        }
        Err(e) => {
            cleanup_discarded_session_workspace(manager, session);
            if let Err(del_err) = manager.delete(&session.id) {
                eprintln!("warning: failed to clean up session: {del_err}");
            }
            Err(e)
        }
    }
}

/// Best-effort cleanup when a session is discarded before approval: removes
/// the planning worktree and, for repo-backed sessions, the temporary clone.
fn cleanup_discarded_session_workspace(manager: &SessionManager, session: &SessionState) {
    if session.repo.is_some() {
        crate::repo_clone::cleanup_session_workspace(manager, session);
    } else {
        cleanup_planning_worktree(session);
    }
}

fn spawn_plan_worker(session_id: &str, rate_limit_retries: usize) -> Result<()> {
    let exe = std::env::current_exe().map_err(CruiseError::IoError)?;
    std::process::Command::new(exe)
        .arg("plan-worker")
        .arg("--session")
        .arg(session_id)
        .arg("--rate-limit-retries")
        .arg(rate_limit_retries.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| CruiseError::ProcessSpawnError(format!("failed to spawn plan worker: {e}")))?;
    Ok(())
}

async fn generate_plan_for_session(
    manager: &SessionManager,
    session: &SessionState,
    rate_limit_retries: usize,
) -> Result<String> {
    let config = manager.load_config(session)?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = setup_plan_vars(session.input_with_attachments(), plan_path.clone(), &config);
    // Background worker: no interactive user, so the SDK agent proceeds on
    // assumptions (no `ask_user`). `resume` is unused for a one-shot generation.
    let mut resume: Option<String> = None;
    // Background worker is non-interactive, so grill mode is never used here.
    let ctx = cli_plan_ctx(
        &config,
        &plan_path,
        Some(plan_working_dir(session)),
        false,
        rate_limit_retries,
        false,
        None,
    );
    generate_plan_markdown(&ctx, &mut vars, &mut resume).await
}

async fn generate_plan_markdown(
    ctx: &PlanPromptCtx<'_>,
    vars: &mut VariableStore,
    resume: &mut Option<String>,
) -> Result<String> {
    let prompt_result = crate::planning::run_plan_prompt_template(
        ctx,
        vars,
        initial_plan_template(ctx.config, ctx.grill),
        "[plan] creating plan...",
        None,
        resume,
        true,
    )
    .await?;
    // If the SDK backend returned a session ID, try to read its transcript
    // for a more useful error message when plan output is empty.
    let transcript = resume
        .as_deref()
        .and_then(|session_id| crate::planning::read_sdk_transcript(ctx.working_dir, session_id));
    crate::planning::resolve_generated_plan_content(
        ctx.plan_path,
        &prompt_result.output,
        &prompt_result.stderr,
        transcript.as_deref(),
    )
}

#[derive(Clone)]
struct FlatNode {
    label: String,
    expanded_step_ids: Vec<String>,
}

impl fmt::Display for FlatNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

fn flatten_nodes(nodes: &[SkippableStepNode], prefix: &str) -> Vec<FlatNode> {
    let mut flat = Vec::new();
    flatten_nodes_into(nodes, 0, prefix, &mut flat);
    flat
}

fn flatten_nodes_into(
    nodes: &[SkippableStepNode],
    depth: usize,
    prefix: &str,
    flat: &mut Vec<FlatNode>,
) {
    for node in nodes {
        let label = if depth == 0 {
            format!("{}{}", prefix, node.id)
        } else {
            node.id
                .rsplit('/')
                .next()
                .unwrap_or(node.id.as_str())
                .to_string()
        };
        flat.push(FlatNode {
            label: format!("{}{}", "  ".repeat(depth), label),
            expanded_step_ids: node.expanded_step_ids.clone(),
        });
        flatten_nodes_into(&node.children, depth + 1, prefix, flat);
    }
}

fn collect_expanded_ids(selected_nodes: Vec<FlatNode>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut expanded_ids = Vec::new();

    for expanded_id in selected_nodes
        .into_iter()
        .flat_map(|node| node.expanded_step_ids)
    {
        if seen.insert(expanded_id.clone()) {
            expanded_ids.push(expanded_id);
        }
    }

    expanded_ids
}

fn flat_node_default_indices(flat: &[FlatNode], previously_skipped: &[String]) -> Vec<usize> {
    let skipped_set: HashSet<&str> = previously_skipped.iter().map(String::as_str).collect();
    flat.iter()
        .enumerate()
        .filter(|(_, node)| {
            !node.expanded_step_ids.is_empty()
                && node
                    .expanded_step_ids
                    .iter()
                    .all(|id| skipped_set.contains(id.as_str()))
        })
        .map(|(i, _)| i)
        .collect()
}

pub(crate) enum StepSkipSelection {
    Confirmed(Vec<String>),
    Cancelled,
}
/// Present a `MultiSelect` prompt so the user can choose which steps to skip.
///
/// Returns [`StepSkipSelection::Cancelled`] when the user cancels or an
/// interruption is received so the approve flow can continue unblocked.
/// Steps that were previously skipped are pre-selected via `previously_skipped`.
pub(crate) fn select_steps_to_skip(
    config: &WorkflowConfig,
    previously_skipped: &[String],
) -> Result<StepSkipSelection> {
    let main_nodes = list_skippable_steps(config)?;
    let after_pr_nodes = list_skippable_after_pr_steps(config)?;
    if main_nodes.is_empty() && after_pr_nodes.is_empty() {
        return Ok(StepSkipSelection::Confirmed(vec![]));
    }

    let mut flat = flatten_nodes(&main_nodes, "");
    flat.extend(flatten_nodes(&after_pr_nodes, "[after-pr] "));
    let defaults = flat_node_default_indices(&flat, previously_skipped);

    crate::platform::reclaim_terminal_foreground();
    match inquire::MultiSelect::new("Steps to skip (Space to toggle, Enter to confirm):", flat)
        .with_help_message("No selection = run all steps")
        .with_default(&defaults)
        .prompt()
    {
        Ok(selected_nodes) => Ok(StepSkipSelection::Confirmed(collect_expanded_ids(
            selected_nodes,
        ))),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            Ok(StepSkipSelection::Cancelled)
        }
        Err(e) => Err(CruiseError::Other(format!("selection error: {e}"))),
    }
}

fn apply_skip_step_selection(
    history: &mut NewSessionHistory,
    scope: HistoryScope<'_>,
    resolved_config_key: &str,
    selection: StepSkipSelection,
) -> (Vec<String>, bool) {
    match selection {
        StepSkipSelection::Confirmed(skipped_steps) => {
            history.record_skip_selection_for_scope(
                scope,
                resolved_config_key,
                skipped_steps.clone(),
            );
            (skipped_steps, true)
        }
        StepSkipSelection::Cancelled => (vec![], false),
    }
}

/// Let the user choose steps to skip with history-based defaults, then record
/// the selection for future sessions. History is loaded once.
fn select_skipped_steps_with_history(
    session: &SessionState,
    config: &WorkflowConfig,
) -> Result<Vec<String>> {
    if config.steps.is_empty() {
        return Ok(vec![]);
    }

    let key = match session.config_path.as_deref() {
        Some(p) => resolved_config_key_for_session(p),
        None => BUILTIN_CONFIG_KEY.to_string(),
    };
    let mut history = NewSessionHistory::load_best_effort();

    let base_dir_str = session.base_dir.to_string_lossy();
    let scope = match session.repo.as_deref() {
        Some(repo) => HistoryScope::Repo(repo),
        None => HistoryScope::Directory(base_dir_str.as_ref()),
    };

    let previously_skipped = history
        .latest_entry_for_scope(scope, &key)
        .map(|entry| entry.skipped_steps.clone())
        .unwrap_or_default();

    let selection = select_steps_to_skip(config, &previously_skipped)?;
    let (skipped_steps, should_persist) =
        apply_skip_step_selection(&mut history, scope, &key, selection);
    if should_persist {
        history.save_best_effort();
    }

    Ok(skipped_steps)
}

/// Interactive approve-plan loop: show plan, let user approve/fix/ask/execute.
/// When `noninteractive` is true (e.g. stdin was piped), auto-approves the plan
/// without prompting so that inquire never tries to read from a non-TTY stdin.
#[expect(
    clippy::too_many_lines,
    reason = "approve/fix/ask/execute loop with multiple action branches"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "approve loop threads session state, config, and the SDK resume id"
)]
async fn run_approve_loop(
    config: &WorkflowConfig,
    manager: &SessionManager,
    session: &mut SessionState,
    plan_path: &std::path::Path,
    vars: &mut VariableStore,
    rate_limit_retries: usize,
    noninteractive: bool,
    resume: &mut Option<String>,
) -> Result<()> {
    let working_dir = session
        .worktree_path
        .clone()
        .unwrap_or_else(|| session.base_dir.clone());
    // Fix / Ask turns reuse one planning context. It is only reached on the
    // interactive path — the noninteractive branch auto-approves below.
    // Grill affects only the initial plan template; fix/ask turns are standard.
    let cancel_token = CancellationToken::new();
    let ctx = cli_plan_ctx(
        config,
        plan_path,
        Some(working_dir.as_path()),
        !noninteractive,
        rate_limit_retries,
        false,
        Some(&cancel_token),
    );

    // Read the plan once up front; re-read only after Fix modifies it.
    let mut plan_content = match crate::metadata::read_plan_markdown(plan_path) {
        Ok(content) => content,
        Err(err) => {
            eprintln!(
                "\n{} Generated plan is missing or empty. Session {} discarded.",
                style("x").red().bold(),
                session.id
            );
            cleanup_discarded_session_workspace(manager, session);
            if let Err(del_err) = manager.delete(&session.id) {
                eprintln!("warning: failed to clean up session: {del_err}");
            }
            return Err(err);
        }
    };

    loop {
        crate::display::print_bordered(&plan_content, Some("plan.md"));

        if noninteractive {
            approve_with_title(session, manager, config, &plan_content, None).await?;
            eprintln!(
                "\n{} Session {} created.",
                style("v").green().bold(),
                session.id
            );
            eprintln!(
                "  Run with: {}",
                style(format!("cruise run {}", session.id)).cyan()
            );
            return Ok(());
        }

        let options = vec!["Approve", "Fix", "Ask", "Execute now", "Publish as Issue"];
        crate::platform::reclaim_terminal_foreground();
        let selected = match inquire::Select::new("Action:", options).prompt() {
            Ok(s) => s,
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                eprintln!("\nCancelled. Session {} discarded.", session.id);
                cleanup_discarded_session_workspace(manager, session);
                manager.delete(&session.id)?;
                return Ok(());
            }
            Err(e) => return Err(CruiseError::Other(format!("selection error: {e}"))),
        };

        match selected {
            "Approve" => {
                session.skipped_steps = select_skipped_steps_with_history(session, config)?;
                tokio::select! {
                    result = approve_with_title(session, manager, config, &plan_content, Some(&cancel_token)) => result?,
                    _ = tokio::signal::ctrl_c() => {
                        cancel_token.cancel();
                        eprintln!("\nCancelled. Session {} discarded.", session.id);
                        cleanup_discarded_session_workspace(manager, session);
                        if let Err(del_err) = manager.delete(&session.id) {
                            eprintln!("warning: failed to clean up session: {del_err}");
                        }
                        return Err(CruiseError::Interrupted);
                    },
                }
                eprintln!(
                    "\n{} Session {} created.",
                    style("v").green().bold(),
                    session.id
                );
                eprintln!(
                    "  Run with: {}",
                    style(format!("cruise run {}", session.id)).cyan()
                );
                return Ok(());
            }
            "Fix" => {
                let text = match prompt_multiline("Describe the changes needed:")? {
                    InputResult::Submitted(t) => t,
                    InputResult::Cancelled => continue,
                };
                vars.set_prev_input(Some(text));
                tokio::select! {
                    result = run_fix_plan(&ctx, vars, resume) => result?,
                    _ = tokio::signal::ctrl_c() => {
                        cancel_token.cancel();
                        return Err(CruiseError::Interrupted);
                    },
                }
                plan_content = crate::metadata::read_plan_markdown(plan_path)?;
            }
            "Ask" => {
                let text = match prompt_multiline("Your question:")? {
                    InputResult::Submitted(t) => t,
                    InputResult::Cancelled => continue,
                };
                vars.set_prev_input(Some(text));
                tokio::select! {
                    result = run_ask_plan(&ctx, vars, resume) => result?,
                    _ = tokio::signal::ctrl_c() => {
                        cancel_token.cancel();
                        return Err(CruiseError::Interrupted);
                    },
                }
            }
            "Execute now" => {
                session.skipped_steps = select_skipped_steps_with_history(session, config)?;
                tokio::select! {
                    result = approve_with_title(session, manager, config, &plan_content, Some(&cancel_token)) => result?,
                    _ = tokio::signal::ctrl_c() => {
                        cancel_token.cancel();
                        eprintln!("\nCancelled. Session {} discarded.", session.id);
                        cleanup_discarded_session_workspace(manager, session);
                        if let Err(del_err) = manager.delete(&session.id) {
                            eprintln!("warning: failed to clean up session: {del_err}");
                        }
                        return Err(CruiseError::Interrupted);
                    },
                }
                eprintln!(
                    "\n{} Executing session {}...",
                    style("->").cyan(),
                    session.id
                );
                let run_args = crate::cli::RunArgs {
                    session: Some(session.id.clone()),
                    all: false,
                    max_retries: DEFAULT_MAX_RETRIES,
                    rate_limit_retries,
                    dry_run: false,
                    cleanup_after_pr: false,
                    no_cleanup_after_pr: false,
                };
                return crate::run_cmd::run(run_args).await;
            }
            "Publish as Issue" => {
                let Some(mention_cruise) = prompt_mention_cruise()? else {
                    continue;
                };
                match crate::issue_publish::publish_plan_issue_and_delete(
                    manager,
                    session.clone(),
                    mention_cruise,
                ) {
                    Ok(published) => {
                        eprintln!(
                            "\n{} Published plan as issue: {}",
                            style("v").green().bold(),
                            published.url
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        eprintln!(
                            "\n{} Failed to publish plan as issue: {e}",
                            style("x").red().bold()
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// Prompt whether to mention `@cruise` in a published issue body.
///
/// Returns `Ok(None)` if the user cancels the prompt.
fn prompt_mention_cruise() -> Result<Option<bool>> {
    crate::platform::reclaim_terminal_foreground();
    match inquire::Confirm::new("Mention @cruise in the issue body?")
        .with_default(false)
        .prompt()
    {
        Ok(answer) => Ok(Some(answer)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(e) => Err(CruiseError::Other(format!("selection error: {e}"))),
    }
}

/// Replan an existing session using the built-in fix-plan prompt.
pub async fn replan_session(
    manager: &SessionManager,
    session: &mut SessionState,
    feedback: String,
    rate_limit_retries: usize,
) -> Result<()> {
    if crate::repo_clone::ensure_repo_session_workspace(manager, session)? {
        manager.save(session)?;
    }
    let config = manager.load_config(session)?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = setup_plan_vars(session.input_with_attachments(), plan_path.clone(), &config);
    vars.set_prev_input(Some(feedback));
    let working_dir = session
        .worktree_path
        .clone()
        .unwrap_or_else(|| session.base_dir.clone());
    let mut resume: Option<String> = None;
    // Fix-plan reuses the standard template regardless of grill.
    let ctx = cli_plan_ctx(
        &config,
        &plan_path,
        Some(working_dir.as_path()),
        std::io::stdin().is_terminal(),
        rate_limit_retries,
        false,
        None,
    );
    run_fix_plan(&ctx, &mut vars, &mut resume).await?;

    let plan_markdown = crate::metadata::read_plan_markdown(&plan_path)?;
    crate::metadata::refresh_session_title_from_plan(session, &plan_markdown);
    session.plan_error = None;
    manager.save(session)?;
    Ok(())
}

/// Run the built-in fix-plan prompt.
async fn run_fix_plan(
    ctx: &PlanPromptCtx<'_>,
    vars: &mut VariableStore,
    resume: &mut Option<String>,
) -> Result<()> {
    run_plan_prompt(
        ctx,
        vars,
        fix_plan_template(ctx.config),
        "[fix-plan] applying fixes...",
        resume,
        true,
    )
    .await
}

/// Run the built-in ask-plan prompt.
async fn run_ask_plan(
    ctx: &PlanPromptCtx<'_>,
    vars: &mut VariableStore,
    resume: &mut Option<String>,
) -> Result<()> {
    run_plan_prompt(
        ctx,
        vars,
        ask_plan_template(ctx.config),
        "[ask-plan] answering question...",
        resume,
        // Read-only: the Ask flow must never overwrite the saved plan, so no
        // plan-writing tools are registered.
        false,
    )
    .await
}

/// Shared implementation for fix-plan and ask-plan: resolve the given
/// `template`, display `label`, and run it as a prompt step.
async fn run_plan_prompt(
    ctx: &PlanPromptCtx<'_>,
    vars: &mut VariableStore,
    template: &str,
    label: &str,
    resume: &mut Option<String>,
    register_plan_tools: bool,
) -> Result<()> {
    let result = crate::planning::run_plan_prompt_template(
        ctx,
        vars,
        template,
        label,
        None,
        resume,
        register_plan_tools,
    )
    .await?;
    vars.set_prev_output(Some(result.output));
    vars.set_prev_stderr(Some(result.stderr));
    vars.set_prev_input(None);
    Ok(())
}

fn resolve_input<F>(
    arg: Option<String>,
    stdin_input: Option<String>,
    interactive: F,
) -> Result<String>
where
    F: FnOnce() -> Result<String>,
{
    if let Some(input) = arg {
        return Ok(input);
    }

    if let Some(input) = stdin_input {
        let trimmed = input.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    interactive()
}

/// Prompt interactively for the initial plan input.
fn prompt_for_plan_input() -> Result<String> {
    prompt_multiline("What would you like to implement?")?.into_result()
}

/// Drive plan generation for a session currently in `Draft` phase.
///
/// On success the session transitions to `AwaitingApproval`.
pub async fn generate_plan_for_draft_session(
    manager: &SessionManager,
    session: &mut SessionState,
    rate_limit_retries: usize,
) -> Result<()> {
    if !matches!(session.phase, SessionPhase::Draft) {
        return Err(CruiseError::Other(format!(
            "expected Draft phase, got {}",
            session.phase.label()
        )));
    }
    if crate::repo_clone::ensure_repo_session_workspace(manager, session)? {
        manager.save(session)?;
    }
    let config = manager.load_config(session)?;
    setup_planning_worktree(manager, session)?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = setup_plan_vars(session.input_with_attachments(), plan_path.clone(), &config);

    // Own the working dir so `ctx` doesn't borrow `session` across the
    // mutable `inspect_err` below.
    let work_dir = plan_working_dir(session).to_path_buf();
    let mut resume: Option<String> = None;
    // Draft regeneration uses the standard plan flow; grill is a `cruise plan`
    // flag and is not threaded through drafts.
    let ctx = cli_plan_ctx(
        &config,
        &plan_path,
        Some(&work_dir),
        std::io::stdin().is_terminal(),
        rate_limit_retries,
        false,
        None,
    );
    generate_plan_markdown(&ctx, &mut vars, &mut resume)
        .await
        .inspect_err(|e| {
            session.plan_error = Some(e.to_string());
            cleanup_planning_worktree(session);
            session.worktree_path = None;
            session.worktree_branch = None;
            if let Err(save_err) = manager.save(session) {
                eprintln!("warning: failed to persist plan error state: {save_err}");
            }
        })?;

    let plan_markdown = crate::metadata::read_plan_markdown(&plan_path)?;
    crate::metadata::refresh_session_title_from_plan(session, &plan_markdown);

    session.phase = SessionPhase::AwaitingApproval;
    session.plan_error = None;
    manager.save(session)?;
    Ok(())
}

/// Regenerate the plan for a session that is already past the Draft phase.
///
/// Unlike [`generate_plan_for_draft_session`], this function accepts sessions in
/// `Draft | AwaitingInput | AwaitingApproval | Planned` phases. The phase
/// transition after successful regeneration matches the GUI's `regenerate_plan`
/// command (`src-tauri/src/commands.rs:1422-1427`):
///
/// - `Draft | AwaitingInput` → `AwaitingApproval`
/// - `AwaitingApproval` → `AwaitingApproval` (no-op)
/// - `Planned` → `Planned` (preserve approval; do NOT silently un-approve)
pub async fn regenerate_plan_for_session(
    manager: &SessionManager,
    session: &mut SessionState,
    rate_limit_retries: usize,
) -> Result<()> {
    match &session.phase {
        SessionPhase::Draft
        | SessionPhase::AwaitingInput
        | SessionPhase::AwaitingApproval
        | SessionPhase::Planned => {}
        other => {
            return Err(CruiseError::Other(format!(
                "expected Draft, AwaitingInput, AwaitingApproval, or Planned phase, got {}",
                other.label()
            )));
        }
    }

    let original_phase = session.phase.clone();

    if crate::repo_clone::ensure_repo_session_workspace(manager, session)? {
        manager.save(session)?;
    }
    let config = manager.load_config(session)?;
    // Save worktree context before planning; a Planned session already has an
    // approved worktree that must survive a planning failure.
    let saved_worktree_path = session.worktree_path.clone();
    let saved_worktree_branch = session.worktree_branch.clone();
    setup_planning_worktree(manager, session)?;
    let plan_path = session.plan_path(&manager.sessions_dir());
    let mut vars = setup_plan_vars(session.input_with_attachments(), plan_path.clone(), &config);

    let work_dir = plan_working_dir(session).to_path_buf();
    let mut resume: Option<String> = None;
    let ctx = cli_plan_ctx(
        &config,
        &plan_path,
        Some(&work_dir),
        std::io::stdin().is_terminal(),
        rate_limit_retries,
        false,
        None,
    );
    generate_plan_markdown(&ctx, &mut vars, &mut resume)
        .await
        .inspect_err(|e| {
            session.plan_error = Some(e.to_string());
            if saved_worktree_path.is_none() {
                // Worktree was freshly created for this planning attempt; clean it up.
                cleanup_planning_worktree(session);
                session.worktree_path = None;
                session.worktree_branch = None;
            } else {
                // Restore the pre-existing approved worktree; do not delete it.
                session.worktree_path = saved_worktree_path;
                session.worktree_branch = saved_worktree_branch;
            }
            if let Err(save_err) = manager.save(session) {
                eprintln!("warning: failed to persist plan error state: {save_err}");
            }
        })?;

    let plan_markdown = crate::metadata::read_plan_markdown(&plan_path)?;
    crate::metadata::refresh_session_title_from_plan(session, &plan_markdown);

    session.plan_error = None;
    // Phase transition matching GUI's regenerate_plan (src-tauri/src/commands.rs:1422-1427):
    // - Draft | AwaitingInput → AwaitingApproval
    // - AwaitingApproval → AwaitingApproval (no-op)
    // - Planned → Planned (preserve approval; do NOT silently un-approve)
    session.phase = match original_phase {
        SessionPhase::Draft | SessionPhase::AwaitingInput => SessionPhase::AwaitingApproval,
        _ => original_phase,
    };
    manager.save(session)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::new_session_history::{NewSessionHistory, NewSessionHistoryEntry};
    use crate::session::SessionManager;
    use crate::test_support::{init_git_repo, lock_process, make_session};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_setup_plan_vars_sets_configured_plan_language() {
        // Given: a workflow config with a custom planning language
        let yaml = r"
command: [echo]
plan_language: Japanese
steps:
  s1:
    command: echo hi
";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When: planning variables are set up
        let vars = setup_plan_vars("task".to_string(), PathBuf::from("plan.md"), &config);

        // Then: the plan language variable resolves to the configured value
        assert_eq!(
            vars.resolve("{plan.language}")
                .unwrap_or_else(|e| panic!("{e:?}")),
            "Japanese"
        );
    }

    #[test]
    fn test_setup_plan_vars_defaults_blank_plan_language_to_english() {
        // Given: a workflow config with a blank planning language
        let yaml = r#"
command: [echo]
plan_language: "   "
steps:
  s1:
    command: echo hi
"#;
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("{e:?}"));

        // When: planning variables are set up
        let vars = setup_plan_vars("task".to_string(), PathBuf::from("plan.md"), &config);

        // Then: the plan language variable falls back to English
        assert_eq!(
            vars.resolve("{plan.language}")
                .unwrap_or_else(|e| panic!("{e:?}")),
            crate::config::DEFAULT_PLAN_LANGUAGE
        );
    }

    #[test]
    fn test_setup_planning_worktree_creates_worktree_and_sets_session_fields() {
        let _lock = lock_process();
        // Given: a valid git repo and a persisted session
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        let cruise_home = tmp.path().join(".cruise");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        let manager = SessionManager::new(cruise_home);
        let mut session = make_session("20260522080000", &repo);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: the planning worktree is set up
        setup_planning_worktree(&manager, &mut session).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: the session has a worktree path and branch recorded
        assert!(
            session.worktree_path.is_some(),
            "worktree_path should be set after setup_planning_worktree"
        );
        let wt_path = session
            .worktree_path
            .as_ref()
            .unwrap_or_else(|| panic!("worktree_path should be set"));
        assert!(wt_path.exists(), "worktree directory should exist on disk");
        assert!(
            session.worktree_branch.is_some(),
            "worktree_branch should be set after setup_planning_worktree"
        );

        // Cleanup
        let ctx = session
            .worktree_context()
            .unwrap_or_else(|| panic!("expected worktree context"));
        crate::worktree::cleanup_worktree(&ctx).unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[test]
    fn test_setup_planning_worktree_noop_for_non_git_repo() {
        // Given: a directory that is NOT a git repository
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let non_git_dir = tmp.path().join("not-a-repo");
        let cruise_home = tmp.path().join(".cruise");
        fs::create_dir_all(&non_git_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(cruise_home);
        let mut session = make_session("20260522080001", &non_git_dir);

        // When: the planning worktree is set up for a non-git directory
        let result = setup_planning_worktree(&manager, &mut session);

        // Then: no error is returned and worktree fields remain unset (graceful fallback)
        assert!(
            result.is_ok(),
            "setup_planning_worktree should not fail for non-git repo: {result:?}"
        );
        assert!(
            session.worktree_path.is_none(),
            "worktree_path should remain None for non-git repo"
        );
        assert!(
            session.worktree_branch.is_none(),
            "worktree_branch should remain None for non-git repo"
        );
    }

    #[test]
    fn test_resolve_input_from_arg() {
        // Given: a CLI arg is provided
        let result = resolve_input(Some("add feature X".to_string()), None, || {
            panic!("interactive prompt should not run")
        });
        assert_eq!(result.unwrap_or_else(|e| panic!("{e:?}")), "add feature X");
    }

    #[test]
    fn test_resolve_input_from_stdin() {
        // Given: stdin input is present and no CLI arg is provided
        let result = resolve_input(None, Some("  add feature from pipe\n".to_string()), || {
            panic!("interactive prompt should not run")
        });
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")),
            "add feature from pipe"
        );
    }

    #[test]
    fn test_resolve_input_without_arg_or_stdin_uses_interactive_result() {
        // Given: no CLI arg or stdin input is available
        let result = resolve_input(None, None, || Ok("resume in place".to_string()));
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")),
            "resume in place"
        );
    }

    // -- resolve_input with multiline stdin ----------------------------------

    #[test]
    fn test_resolve_input_multiline_from_stdin_preserves_internal_newlines() {
        // Given: multi-line stdin input (piped, etc.)
        let stdin = "line1\nline2\nline3\n".to_string();
        let result = resolve_input(None, Some(stdin), || {
            panic!("interactive prompt should not run")
        });
        // Then: only leading/trailing whitespace is trimmed, internal newlines are preserved
        assert_eq!(
            result.unwrap_or_else(|e| panic!("{e:?}")),
            "line1\nline2\nline3"
        );
    }

    #[test]
    fn test_resolve_input_multiline_trims_only_leading_trailing_whitespace() {
        // Given: multi-line stdin input with extra whitespace at start and end
        let stdin = "  line1\nline2  \n".to_string();
        let result = resolve_input(None, Some(stdin), || {
            panic!("interactive prompt should not run")
        });
        // Then: only leading/trailing whitespace is removed, internal newlines are preserved
        assert_eq!(result.unwrap_or_else(|e| panic!("{e:?}")), "line1\nline2");
    }

    #[test]
    fn test_collect_expanded_ids_deduplicates_parent_and_child_selection() {
        let selected = vec![
            FlatNode {
                label: "review-pass".to_string(),
                expanded_step_ids: vec![
                    "review-pass/simplify".to_string(),
                    "review-pass/coderabbit".to_string(),
                ],
            },
            FlatNode {
                label: "  simplify".to_string(),
                expanded_step_ids: vec!["review-pass/simplify".to_string()],
            },
        ];

        assert_eq!(
            collect_expanded_ids(selected),
            vec!["review-pass/simplify", "review-pass/coderabbit"]
        );
    }

    #[test]
    fn test_apply_skip_step_selection_records_confirmed_empty_selection() {
        let mut history = NewSessionHistory::default();
        let (skipped_steps, should_persist) = apply_skip_step_selection(
            &mut history,
            HistoryScope::Directory("/home/user/proj"),
            "/config/a.yaml",
            StepSkipSelection::Confirmed(vec![]),
        );

        assert!(should_persist);
        assert!(skipped_steps.is_empty());
        assert_eq!(history.entries.len(), 1);
        assert_eq!(
            history.entries[0],
            NewSessionHistoryEntry {
                selected_at: history.entries[0].selected_at.clone(),
                input: String::new(),
                requested_config_path: None,
                working_dir: "/home/user/proj".to_string(),
                repo: None,
                resolved_config_key: "/config/a.yaml".to_string(),
                skipped_steps: vec![],
            }
        );
    }

    #[test]
    fn test_apply_skip_step_selection_does_not_record_cancelled_prompt() {
        let mut history = NewSessionHistory::default();
        history.record_selection(NewSessionHistoryEntry {
            selected_at: String::new(),
            input: String::new(),
            requested_config_path: None,
            working_dir: String::new(),
            repo: None,
            resolved_config_key: "/config/a.yaml".to_string(),
            skipped_steps: vec!["review".to_string()],
        });

        let (skipped_steps, should_persist) = apply_skip_step_selection(
            &mut history,
            HistoryScope::Directory(""),
            "/config/a.yaml",
            StepSkipSelection::Cancelled,
        );

        assert!(!should_persist);
        assert!(skipped_steps.is_empty());
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].skipped_steps, vec!["review"]);
    }

    #[test]
    fn test_apply_skip_step_selection_updates_existing_gui_history_entry() {
        let mut history = NewSessionHistory::default();
        history.record_selection(NewSessionHistoryEntry {
            selected_at: "2026-04-07T00:00:00Z".to_string(),
            input: String::new(),
            requested_config_path: Some("/config/a.yaml".to_string()),
            working_dir: "/Users/test/project".to_string(),
            repo: None,
            resolved_config_key: "/config/a.yaml".to_string(),
            skipped_steps: vec!["plan".to_string()],
        });

        let (skipped_steps, should_persist) = apply_skip_step_selection(
            &mut history,
            HistoryScope::Directory("/Users/test/project"),
            "/config/a.yaml",
            StepSkipSelection::Confirmed(vec!["review".to_string()]),
        );

        assert!(should_persist);
        assert_eq!(skipped_steps, vec!["review"]);
        assert_eq!(history.entries.len(), 1);
        assert_eq!(
            history.entries[0].requested_config_path.as_deref(),
            Some("/config/a.yaml")
        );
        assert_eq!(history.entries[0].working_dir, "/Users/test/project");
        assert_eq!(history.entries[0].skipped_steps, vec!["review"]);
    }

    // -- generate_title_via_sdk cancellation -----------------------------------

    /// Given: a command-backend config, a pre-cancelled token
    /// When: `generate_title_via_sdk` is called
    /// Then: returns `CruiseError::Interrupted` before the 5-second timeout
    ///
    /// If this test times out, the `cancel_token` is not forwarded to `PromptRun`
    /// inside `generate_title_via_sdk`.
    #[cfg(unix)]
    #[tokio::test]
    async fn generate_title_via_sdk_pre_cancelled_token_returns_interrupted() {
        use crate::cancellation::CancellationToken;
        use crate::config::WorkflowConfig;
        use crate::error::CruiseError;

        let _guard = crate::test_support::lock_process();

        // Given: command backend that blocks indefinitely, pre-cancelled token
        let yaml = "command: [\"sleep\", \"100\"]\nsteps:\n  s1:\n    prompt: hi\n";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("bad yaml: {e:?}"));
        let token = CancellationToken::new();
        token.cancel();

        // When
        let timed = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            generate_title_via_sdk(&config, "my task", "# Plan\n- step", Some(&token)),
        )
        .await;

        // Then: completes before timeout and returns Interrupted
        assert!(
            timed.is_ok(),
            "timed out — cancel_token is not forwarded inside generate_title_via_sdk"
        );
        assert!(
            matches!(
                timed.unwrap_or_else(|e| panic!("{e:?}")),
                Err(CruiseError::Interrupted)
            ),
            "expected CruiseError::Interrupted"
        );
    }

    /// Given: a command-backend config, no cancel token
    /// When: `generate_title_via_sdk` is called with a fast command (cat)
    /// Then: returns an error (no title tool called), but does not panic
    ///       — this is a baseline test to verify no regression in the happy path signature
    #[cfg(unix)]
    #[tokio::test]
    async fn generate_title_via_sdk_no_token_returns_err_without_sdk_tool() {
        use crate::config::WorkflowConfig;

        let _guard = crate::test_support::lock_process();

        let yaml = "command: [\"cat\"]\nsteps:\n  s1:\n    prompt: hi\n";
        let config = WorkflowConfig::from_yaml(yaml).unwrap_or_else(|e| panic!("bad yaml: {e:?}"));

        // cat echoes the prompt; the generate_title tool is never called.
        // With no token, the function should complete (with an Err: None from the store).
        let result = generate_title_via_sdk(&config, "task", "# Plan", None).await;

        // The title store remains empty because cat doesn't call the tool,
        // so the function returns Err with "title store returned None".
        assert!(
            result.is_err(),
            "expected Err when title tool was not called"
        );
    }

    // -----------------------------------------------------------------------
    // regenerate_plan_for_session — phase gate
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[tokio::test]
    async fn test_regenerate_plan_for_session_running_phase_fails() {
        // Given: a Running session — cannot regenerate while executing
        let _guard = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        fs::write(
            repo.join("cruise.yaml"),
            "command: [echo]\nsteps:\n  s:\n    prompt: hi",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619200001", &repo);
        session.phase = SessionPhase::Running;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = regenerate_plan_for_session(&manager, &mut session, 0).await;

        // Then: must return an error (not silently succeed or panic)
        assert!(
            result.is_err(),
            "regenerate_plan_for_session should fail for Running phase"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_regenerate_plan_for_session_completed_phase_fails() {
        // Given: a Completed session
        let _guard = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        fs::write(
            repo.join("cruise.yaml"),
            "command: [echo]\nsteps:\n  s:\n    prompt: hi",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619200002", &repo);
        session.phase = SessionPhase::Completed;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = regenerate_plan_for_session(&manager, &mut session, 0).await;

        // Then
        assert!(
            result.is_err(),
            "regenerate_plan_for_session should fail for Completed phase"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_regenerate_plan_for_session_suspended_phase_fails() {
        // Given: a Suspended session
        let _guard = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        fs::write(
            repo.join("cruise.yaml"),
            "command: [echo]\nsteps:\n  s:\n    prompt: hi",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619200003", &repo);
        session.phase = SessionPhase::Suspended;
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = regenerate_plan_for_session(&manager, &mut session, 0).await;

        // Then
        assert!(
            result.is_err(),
            "regenerate_plan_for_session should fail for Suspended phase"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_regenerate_plan_for_session_failed_phase_fails() {
        // Given: a Failed session
        let _guard = lock_process();
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap_or_else(|e| panic!("{e:?}"));
        init_git_repo(&repo);
        fs::write(
            repo.join("cruise.yaml"),
            "command: [echo]\nsteps:\n  s:\n    prompt: hi",
        )
        .unwrap_or_else(|e| panic!("{e:?}"));

        let manager = SessionManager::new(tmp.path().join(".cruise"));
        let mut session = make_session("20260619200004", &repo);
        session.phase = SessionPhase::Failed("prior error".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When
        let result = regenerate_plan_for_session(&manager, &mut session, 0).await;

        // Then
        assert!(
            result.is_err(),
            "regenerate_plan_for_session should fail for Failed phase"
        );
    }

    #[test]
    fn test_finalize_skip_planning_session_lands_in_planned() {
        // Given: a session manager and a fresh session in AwaitingApproval state
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let base_dir = tmp.path().join("repo");
        let cruise_home = tmp.path().join(".cruise");
        fs::create_dir_all(&base_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(cruise_home);
        let mut session = SessionState::new(
            "20260621100000".to_string(),
            base_dir,
            "cruise.yaml".to_string(),
            "implement the feature".to_string(),
        );
        // SessionState::new starts in AwaitingApproval — the default
        assert_eq!(session.phase, SessionPhase::AwaitingApproval);
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: finalize_skip_planning_session is called
        finalize_skip_planning_session(&manager, &mut session).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: phase transitions to Planned (not stuck at AwaitingApproval)
        assert_eq!(
            session.phase,
            SessionPhase::Planned,
            "skip-planning session should land in Planned, got {:?}",
            session.phase
        );
    }

    #[test]
    fn test_finalize_skip_planning_session_clears_worktree_for_repo_session() {
        // Given: a repo-backed session in AwaitingApproval with a worktree_path set.
        // The "wt" directory is intentionally absent: worktree_context() returns None so
        // the git-level worktree removal is skipped, but cleanup_after_approval still
        // clears worktree_path on the session. Actual filesystem deletion is covered by
        // repo_clone::tests::test_cleanup_after_approval_removes_clone_and_clears_worktree_path.
        let tmp = TempDir::new().unwrap_or_else(|e| panic!("{e:?}"));
        let base_dir = tmp.path().join("repo");
        let cruise_home = tmp.path().join(".cruise");
        fs::create_dir_all(&base_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let manager = SessionManager::new(cruise_home);
        let mut session = SessionState::new(
            "20260621100001".to_string(),
            base_dir.clone(),
            "cruise.yaml".to_string(),
            "implement the feature".to_string(),
        );
        session.repo = Some("owner/repo".to_string());
        session.worktree_path = Some(base_dir.join("wt"));
        session.worktree_branch = Some("cruise/20260621100001-task".to_string());
        manager.create(&session).unwrap_or_else(|e| panic!("{e:?}"));

        // When: finalize_skip_planning_session is called
        finalize_skip_planning_session(&manager, &mut session).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: phase is Planned and the temporary clone worktree_path is cleared
        assert_eq!(
            session.phase,
            SessionPhase::Planned,
            "repo session should land in Planned after skip-planning, got {:?}",
            session.phase
        );
        assert!(
            session.worktree_path.is_none(),
            "worktree_path should be cleared by cleanup_after_approval for repo session"
        );
    }
}
