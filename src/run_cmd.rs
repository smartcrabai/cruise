use std::collections::HashSet;
use std::io::IsTerminal as _;

use console::style;
use futures::StreamExt as _;
use inquire::InquireError;
use serde_json::Value;

use crate::application::{
    ApplicationEvent, EventStream as OutputStream, RunAllRequest, RunRequest,
};
use crate::cli::RunArgs;
use crate::daemon::client::{DaemonClient, EventFilter};
use crate::error::{CruiseError, Result};
use crate::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode};

const WORKSPACE_WORKTREE_LABEL: &str = "Create worktree (new branch)";
const WORKSPACE_CURRENT_BRANCH_LABEL: &str = "Use current branch";

fn stdin_is_terminal() -> bool {
    std::io::stdin().is_terminal()
}

fn prompt_workspace_mode() -> Result<WorkspaceMode> {
    let _guard = crate::option_handler::prompt_lock_guard();
    let _blocked = crate::herdr::blocked("Where should cruise execute?");
    let options = vec![WORKSPACE_WORKTREE_LABEL, WORKSPACE_CURRENT_BRANCH_LABEL];
    crate::platform::reclaim_terminal_foreground();
    match inquire::Select::new("Where should cruise execute?", options).prompt() {
        Ok(choice) if choice == WORKSPACE_CURRENT_BRANCH_LABEL => Ok(WorkspaceMode::CurrentBranch),
        Ok(_) => Ok(WorkspaceMode::Worktree),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Err(
            CruiseError::Other("workspace mode selection cancelled".to_owned()),
        ),
        Err(error) => Err(CruiseError::Other(format!("selection error: {error}"))),
    }
}

struct FlagPromptChoice<'a>(&'a crate::flags::FlagChoice);

impl std::fmt::Display for FlagPromptChoice<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0.name)?;
        if let Some(description) = &self.0.description {
            write!(formatter, "  {description}")?;
        }
        Ok(())
    }
}

pub(crate) fn select_flag_values(
    config: &crate::config::WorkflowConfig,
    current: &crate::flags::FlagValues,
) -> Result<Option<crate::flags::FlagValues>> {
    let choices = crate::flags::choices(config);
    if choices.is_empty() || !stdin_is_terminal() {
        return Ok(Some(current.clone()));
    }

    let _guard = crate::option_handler::prompt_lock_guard();
    let _blocked = crate::herdr::blocked("Which optional workflow flags should be enabled?");
    let defaults: Vec<_> = choices
        .iter()
        .enumerate()
        .filter_map(|(index, choice)| {
            current
                .get(&choice.name)
                .copied()
                .unwrap_or(false)
                .then_some(index)
        })
        .collect();
    let options = choices.iter().map(FlagPromptChoice).collect();
    crate::platform::reclaim_terminal_foreground();
    match inquire::MultiSelect::new("Which optional workflow flags should be enabled?", options)
        .with_help_message("Space to toggle, Enter to confirm · No selection = all flags off")
        .with_default(&defaults)
        .prompt()
    {
        Ok(selected) => {
            let enabled: Vec<_> = selected
                .into_iter()
                .map(|choice| choice.0.name.clone())
                .collect();
            Ok(Some(crate::flags::apply_enabled(
                current, &choices, &enabled,
            )))
        }
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(error) => Err(CruiseError::Other(format!("selection error: {error}"))),
    }
}

pub async fn run(args: RunArgs) -> Result<()> {
    let _herdr = crate::herdr::start();
    finish_cli_run(run_inner(args).await)
}

pub(crate) fn finish_cli_run(result: Result<()>) -> Result<()> {
    match result {
        Err(CruiseError::StepPaused) => {
            eprintln!("Session paused. Resume with `cruise run`.");
            Ok(())
        }
        other => other,
    }
}

async fn run_inner(args: RunArgs) -> Result<()> {
    if args.auto_requeue_max_attempts.is_some() && !args.all {
        return Err(CruiseError::Other(
            "--auto-requeue-max-attempts requires --all".to_owned(),
        ));
    }
    if let Some(parallelism) = args.parallelism {
        if parallelism == 0 {
            return Err(CruiseError::Other(
                "--parallelism must be at least 1".to_owned(),
            ));
        }
        if !args.all {
            return Err(CruiseError::Other(
                "--parallelism is only valid together with --all".to_owned(),
            ));
        }
    }
    if args.all && args.session.is_some() {
        return Err(CruiseError::Other(
            "Cannot specify both --all and a session ID".to_owned(),
        ));
    }
    if args.dry_run {
        return run_dry_run(&args);
    }

    let client = DaemonClient::connect().await?;
    if args.all {
        run_all(&client, args).await
    } else {
        run_single(&client, args).await
    }
}

async fn run_single(client: &DaemonClient, args: RunArgs) -> Result<()> {
    let context = crate::request_context::RequestContext::capture()?;
    let session_id = match args.session.as_deref() {
        Some(id) => id.to_owned(),
        None => select_pending_session(client).await?,
    };
    let (session, active) = client.run_session(&session_id).await?;
    if !active {
        if matches!(&session.phase, SessionPhase::PrFailed(_)) {
            return Err(CruiseError::Other(format!(
                "Session {session_id} is PR Failed. Use Create PR in `cruise list` to retry publishing."
            )));
        }
        if !session.phase.is_runnable() {
            return Err(CruiseError::Other(format!(
                "Session {session_id} is in '{}' phase and cannot be run. Approve it first with `cruise list`.",
                session.phase.label()
            )));
        }
        if session.plan_error.is_some()
            || client
                .run_sessions()
                .await?
                .into_iter()
                .find(|listed| listed.id == session_id)
                .is_some_and(|listed| !listed.plan_available)
        {
            return Err(CruiseError::Other(format!(
                "Session {session_id} has no usable plan; retry planning first."
            )));
        }
    }
    let request = if active {
        RunRequest {
            max_retries: args.max_retries,
            rate_limit_retries: args.rate_limit_retries,
            cleanup_after_pr: args.cleanup_after_pr_override(),
            context: Some(context),
            ..RunRequest::default()
        }
    } else {
        client_run_request(&session, &args, &context)?
    };

    let stream = client
        .events(EventFilter {
            session: Some(session_id.clone()),
            batch: false,
        })
        .await?;
    client.start_run(&session_id, &request).await?;
    let mut handler = crate::cli_events::TerminalEvents::new(client, &session_id);
    let finished = crate::cli_events::drive(client, &session_id, stream, &mut handler).await?;
    if finished.cancelled_by_user {
        crate::status_eprintln!(
            "\n{} Interrupted -- session saved as Suspended.",
            style("||").yellow().bold()
        );
        return Err(CruiseError::Interrupted);
    }
    match finished.state.phase {
        SessionPhase::Completed => Ok(()),
        SessionPhase::Failed(error) => Err(CruiseError::Other(error)),
        SessionPhase::PrFailed(error) => Err(CruiseError::PrPublicationFailed(error)),
        SessionPhase::Suspended => Err(CruiseError::Interrupted),
        phase => Err(CruiseError::Other(format!(
            "session {session_id} is in '{}' phase and cannot be run",
            phase.label()
        ))),
    }
}

fn client_run_request(
    session: &SessionState,
    args: &RunArgs,
    context: &crate::request_context::RequestContext,
) -> Result<RunRequest> {
    let mut request = RunRequest {
        max_retries: args.max_retries,
        rate_limit_retries: args.rate_limit_retries,
        cleanup_after_pr: args.cleanup_after_pr_override(),
        context: Some(context.clone()),
        ..RunRequest::default()
    };
    let fresh = session.current_step.is_none();
    let after_pr = session.execution_stage == crate::session::SessionExecutionStage::AfterPr;
    if fresh
        && !after_pr
        && !session.exec
        && session.repo.is_none()
        && session.pr_target.is_none()
        && session.workspace_mode == WorkspaceMode::Worktree
        && stdin_is_terminal()
    {
        request.workspace_mode = Some(prompt_workspace_mode()?);
    }

    let needs_config =
        !args.flags.is_empty() || (fresh && session.flags.is_empty() && stdin_is_terminal());
    if needs_config {
        let manager = SessionManager::new(crate::paths::data_dir()?);
        let config = manager.load_config_with_env(
            session,
            crate::config::ConfigEnv::Snapshot(&context.config_env),
        )?;
        crate::config::validate_config(&config)?;
        if !args.flags.is_empty() {
            let overrides = crate::flags::from_cli(&args.flags)?;
            crate::flags::validate_names(&config, &overrides)?;
            let mut values = session.flags.clone();
            values.extend(overrides);
            request.flags = Some(values);
        } else if let Some(values) = select_flag_values(&config, &session.flags)? {
            request.flags = Some(values);
        } else {
            return Err(CruiseError::Other("flag selection cancelled".to_owned()));
        }
    }
    Ok(request)
}

async fn select_pending_session(client: &DaemonClient) -> Result<String> {
    let sessions = client
        .run_sessions()
        .await?
        .into_iter()
        .filter(is_runnable_summary)
        .collect::<Vec<_>>();
    select_from_summaries(&sessions)
}

fn is_runnable_summary(session: &crate::daemon::client::RunSessionSummary) -> bool {
    matches!(
        session.phase.as_str(),
        "Planned" | "Running" | "Failed" | "Suspended"
    )
}

fn select_from_summaries(sessions: &[crate::daemon::client::RunSessionSummary]) -> Result<String> {
    if sessions.is_empty() {
        return Err(CruiseError::Other(
            "No pending sessions. Run `cruise plan` first.".to_owned(),
        ));
    }
    if sessions.len() == 1 {
        let session = &sessions[0];
        eprintln!(
            "{} Selected session: {} -- {}",
            style("->").cyan(),
            session.id,
            crate::display::truncate(session.title.as_deref().unwrap_or(&session.input), 60)
        );
        return Ok(session.id.clone());
    }
    let labels = sessions
        .iter()
        .map(|session| {
            format!(
                "{} | {} | {}",
                session.id,
                session.phase,
                crate::display::truncate(session.title.as_deref().unwrap_or(&session.input), 60)
            )
        })
        .collect::<Vec<_>>();
    let label_refs = labels.iter().map(String::as_str).collect::<Vec<_>>();
    let _blocked = crate::herdr::blocked("Select a session to run");
    let selected = match inquire::Select::new("Select a session to run:", label_refs).prompt() {
        Ok(selected) => selected,
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            return Err(CruiseError::Other("session selection cancelled".to_owned()));
        }
        Err(error) => return Err(CruiseError::Other(format!("selection error: {error}"))),
    };
    let index = labels
        .iter()
        .position(|label| label.as_str() == selected)
        .ok_or_else(|| CruiseError::Other(format!("selected session not found: {selected}")))?;
    Ok(sessions[index].id.clone())
}

async fn run_all(client: &DaemonClient, args: RunArgs) -> Result<()> {
    let context = crate::request_context::RequestContext::capture()?;
    let sessions = client.run_sessions().await?;
    let dashboard = std::io::stderr()
        .is_terminal()
        .then(crate::batch_dashboard::BatchDashboard::start);
    let mut events = BatchEvents::new(client, dashboard.as_ref());
    for session in &sessions {
        if matches!(session.phase.as_str(), "Planned" | "Suspended") {
            events.record_session(&session.id);
            if let Some(dashboard) = &dashboard {
                dashboard.add_summary(
                    &session.id,
                    session.title.as_deref().unwrap_or(&session.input),
                );
            }
        }
    }

    let stream = client
        .events(EventFilter {
            batch: true,
            ..EventFilter::default()
        })
        .await?;
    client
        .start_run_all(&RunAllRequest {
            parallelism: Some(run_all_parallelism(args.parallelism)),
            auto_requeue_max_attempts: args.auto_requeue_max_attempts,
            context: Some(context),
        })
        .await?;
    let cancelled_by_user = drive_batch(client, stream, &mut events).await?;

    let results = load_batch_results(client, &events.session_order).await;
    for state in &results {
        let phase = state.phase.label().to_owned();
        let error = match &state.phase {
            SessionPhase::Failed(error) | SessionPhase::PrFailed(error) => Some(error.clone()),
            _ => None,
        };
        if let Some(dashboard) = &dashboard {
            dashboard.on_event(&ApplicationEvent::BatchSessionFinished {
                id: state.id.clone(),
                phase,
                error,
            });
        }
    }
    for session_id in &events.paused_sessions {
        eprintln!("session {session_id} paused");
    }
    let summary = format_run_all_summary(&results);
    if !summary.is_empty() {
        if dashboard.is_some() {
            crate::batch_dashboard::BatchDashboard::announce(&format!("\n{summary}"));
        } else {
            eprintln!("\n{summary}");
        }
    }
    drop(events);
    drop(dashboard);
    if cancelled_by_user {
        return Err(CruiseError::Interrupted);
    }
    Ok(())
}

async fn load_batch_results(client: &DaemonClient, session_order: &[String]) -> Vec<SessionState> {
    let mut results = Vec::with_capacity(session_order.len());
    for id in session_order {
        if let Ok((state, _)) = client.run_session(id).await {
            results.push(state);
        }
    }
    results
}

fn run_all_parallelism(requested: Option<usize>) -> usize {
    requested.unwrap_or(1)
}

struct BatchEvents<'a> {
    prompts: crate::cli_events::BatchPrompts<'a>,
    dashboard: Option<&'a crate::batch_dashboard::BatchDashboard>,
    session_order: Vec<String>,
    seen_sessions: HashSet<String>,
    paused_sessions: Vec<String>,
    pending_pauses: Vec<String>,
}

impl<'a> BatchEvents<'a> {
    fn new(
        client: &'a DaemonClient,
        dashboard: Option<&'a crate::batch_dashboard::BatchDashboard>,
    ) -> Self {
        Self {
            prompts: crate::cli_events::BatchPrompts::new(client),
            dashboard,
            session_order: Vec::new(),
            seen_sessions: HashSet::new(),
            paused_sessions: Vec::new(),
            pending_pauses: Vec::new(),
        }
    }

    fn record_session(&mut self, id: &str) {
        if self.seen_sessions.insert(id.to_owned()) {
            self.session_order.push(id.to_owned());
            self.prompts.include_session(id);
        }
    }

    fn known_session_ids(&self) -> Vec<String> {
        self.session_order.clone()
    }

    fn record_paused_session(&mut self, id: &str) {
        queue_pause_for_session(&mut self.paused_sessions, &mut self.pending_pauses, id);
    }

    fn take_pause_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_pauses)
    }

    fn handle_event(&mut self, event: &ApplicationEvent) -> Result<()> {
        let belongs_to_batch = match event {
            ApplicationEvent::BatchStarted { .. }
            | ApplicationEvent::BatchTotalChanged { .. }
            | ApplicationEvent::BatchFinished { .. }
            | ApplicationEvent::BatchFailed { .. } => true,
            ApplicationEvent::BatchSessionStarted { id } => {
                self.record_session(id);
                true
            }
            ApplicationEvent::BatchSessionFinished { id, .. } => self.seen_sessions.contains(id),
            ApplicationEvent::RunStarted { session_id }
            | ApplicationEvent::StepStarted { session_id, .. }
            | ApplicationEvent::RunPhase { session_id, .. }
            | ApplicationEvent::RunFinished { session_id, .. }
            | ApplicationEvent::RunFailed { session_id, .. }
            | ApplicationEvent::RunCancelled { session_id }
            | ApplicationEvent::AskUserRequired { session_id, .. }
            | ApplicationEvent::OptionRequired { session_id, .. }
            | ApplicationEvent::AutoRequeued { session_id, .. } => {
                self.seen_sessions.contains(session_id)
            }
            ApplicationEvent::LogChunk {
                session_id, batch, ..
            } => {
                *batch
                    && session_id
                        .as_ref()
                        .is_none_or(|id| self.seen_sessions.contains(id))
            }
            _ => false,
        };
        if !belongs_to_batch {
            return self.prompts.handle_event(event);
        }
        if let Some(dashboard) = self.dashboard {
            dashboard.on_event(event);
        }
        match event {
            ApplicationEvent::BatchStarted { total, parallelism } => self.announce(&format!(
                "run --all: running {total} session(s) with parallelism {parallelism}"
            )),
            ApplicationEvent::BatchTotalChanged { total } => {
                self.announce(&format!("run --all: {total} session(s) queued"));
            }
            ApplicationEvent::AutoRequeued {
                session_id,
                attempt,
                max_attempts,
                error,
            } => self.announce(&format!(
                "Auto-requeued {session_id} ({attempt}/{max_attempts}): {error}"
            )),
            ApplicationEvent::BatchSessionFinished {
                id,
                error: Some(error),
                ..
            } => self.announce(&format!(
                "warning: session {id} encountered an error: {error}"
            )),
            ApplicationEvent::LogChunk {
                stream,
                text,
                batch: true,
                ..
            } => {
                for line in text.lines() {
                    let line = match stream {
                        OutputStream::Info => line.to_owned(),
                        OutputStream::Stdout => format!("  {line}"),
                        OutputStream::Stderr => {
                            format!("  {} {line}", console::style("stderr:").dim())
                        }
                    };
                    self.announce(&line);
                }
            }
            _ => {}
        }
        match self.prompts.handle_event(event) {
            Ok(()) => Ok(()),
            Err(CruiseError::StepPaused) => {
                if let ApplicationEvent::AskUserRequired { session_id, .. }
                | ApplicationEvent::OptionRequired { session_id, .. } = event
                {
                    self.record_paused_session(session_id);
                }
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn handle_pending(&mut self, prompts: &[crate::application::PendingPrompt]) -> Result<()> {
        for prompt in prompts {
            match self
                .prompts
                .handle_pending_prompts(std::slice::from_ref(prompt))
            {
                Ok(()) => {}
                Err(CruiseError::StepPaused) => {
                    self.record_paused_session(&prompt.session_id);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn announce(&self, message: &str) {
        if self.dashboard.is_some() {
            crate::batch_dashboard::BatchDashboard::announce(message);
        } else {
            crate::status_eprintln!("{message}");
        }
    }
}

fn queue_pause_for_session(
    paused_sessions: &mut Vec<String>,
    pending_pauses: &mut Vec<String>,
    session_id: &str,
) {
    if !paused_sessions.iter().any(|paused| paused == session_id) {
        paused_sessions.push(session_id.to_owned());
        pending_pauses.push(session_id.to_owned());
    }
}

async fn drive_batch(
    client: &DaemonClient,
    mut stream: crate::daemon::client::EventStream,
    events: &mut BatchEvents<'_>,
) -> Result<bool> {
    let mut cancelled_by_user = false;
    loop {
        tokio::select! {
            signal = tokio::signal::ctrl_c(), if !cancelled_by_user => {
                signal.map_err(|error| CruiseError::Other(format!("failed to listen for Ctrl+C: {error}")))?;
                cancelled_by_user = true;
                eprintln!("Cancelling run --all...");
                let _ = client.cancel_run_all().await?;
            }
            item = stream.next() => {
                if let Some(Ok(event)) = item {
                    if let ApplicationEvent::BatchFailed { error } = &event {
                        return Err(CruiseError::Other(error.clone()));
                    }
                    let finished = matches!(&event, ApplicationEvent::BatchFinished { .. });
                    events.handle_event(&event)?;
                    for id in events.take_pause_requests() {
                        client.pause(&id).await?;
                    }
                    if finished {
                        return Ok(cancelled_by_user);
                    }
                } else {
                    let status: Value = client.get_json("status").await?;
                    if !status["batchActive"].as_bool().unwrap_or(true) {
                        return Ok(cancelled_by_user);
                    }
                    stream = client
                        .events(EventFilter {
                            batch: true,
                            ..EventFilter::default()
                        })
                        .await?;
                    for id in events.known_session_ids() {
                        let prompts = match client
                            .get_json::<Vec<crate::application::PendingPrompt>>(
                                &format!("sessions/{id}/prompts"),
                            )
                            .await
                        {
                            Ok(prompts) => prompts,
                            Err(CruiseError::IoError(error))
                                if error.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(error) => return Err(error),
                        };
                        events.handle_pending(&prompts)?;
                        for id in events.take_pause_requests() {
                            client.pause(&id).await?;
                        }
                    }
                }
            }
        }
    }
}

fn run_dry_run(args: &RunArgs) -> Result<()> {
    let manager = SessionManager::new(crate::paths::data_dir()?);
    if args.all {
        for session in manager.run_all_candidates()? {
            dry_run_session(&manager, &session.id, args)?;
        }
        return Ok(());
    }
    let session_id = match args.session.as_deref() {
        Some(id) => id.to_owned(),
        None => select_local_session(&manager)?,
    };
    dry_run_session(&manager, &session_id, args)
}

fn select_local_session(manager: &SessionManager) -> Result<String> {
    let sessions = manager
        .list()?
        .into_iter()
        .filter(|session| !session.exec && session.phase.is_runnable())
        .collect::<Vec<_>>();
    if sessions.is_empty() {
        return Err(CruiseError::Other(
            "No pending sessions. Run `cruise plan` first.".to_owned(),
        ));
    }
    if sessions.len() == 1 {
        return Ok(sessions[0].id.clone());
    }
    let labels = sessions
        .iter()
        .map(|session| {
            format!(
                "{} | {} | {}",
                session.id,
                session.phase.label(),
                crate::display::truncate(session.input_or_title(), 60)
            )
        })
        .collect::<Vec<_>>();
    let label_refs = labels.iter().map(String::as_str).collect::<Vec<_>>();
    let _blocked = crate::herdr::blocked("Select a session to run");
    let selected = inquire::Select::new("Select a session to run:", label_refs)
        .prompt()
        .map_err(|error| match error {
            InquireError::OperationCanceled | InquireError::OperationInterrupted => {
                CruiseError::Other("session selection cancelled".to_owned())
            }
            other => CruiseError::Other(format!("selection error: {other}")),
        })?;
    let index = labels
        .iter()
        .position(|label| label.as_str() == selected)
        .ok_or_else(|| CruiseError::Other(format!("selected session not found: {selected}")))?;
    Ok(sessions[index].id.clone())
}

fn dry_run_session(manager: &SessionManager, id: &str, args: &RunArgs) -> Result<()> {
    let mut session = manager.load(id)?;
    if !session.phase.is_runnable() {
        return Err(CruiseError::Other(format!(
            "session {id} is in '{}' phase and cannot be run",
            session.phase.label()
        )));
    }
    let config = manager.load_config(&session)?;
    crate::config::validate_config(&config)?;
    let flags = crate::flags::from_cli(&args.flags)?;
    crate::flags::validate_names(&config, &flags)?;
    session.flags.extend(flags);
    let max_retries = crate::config::resolve_effective_max_retries(args.max_retries, &config);
    crate::config::validate_group_retry_budget(&config, max_retries)?;
    let compiled = crate::workflow::compile(config.clone())?;
    let mut dag = crate::graph::build_graph(&compiled, max_retries)?;
    let after_pr = session.execution_stage == crate::session::SessionExecutionStage::AfterPr;
    let start = if after_pr {
        dag.start.clone()
    } else {
        crate::graph::persistence::prepare_resume(
            &mut dag,
            &manager.dag_path(id),
            session.has_dag,
            session.current_step.as_deref(),
            session.current_step_is_node_id,
            session.execution_id.as_deref(),
        )?
    };
    if !after_pr {
        crate::graph::validation::validate_workflow(
            &compiled,
            &dag,
            &start,
            &crate::flags::preflight_skips(&compiled, &session.flags, &session.skipped_steps),
        )?;
    }
    eprintln!("{}", style(format!("Session: {id}")).dim());
    crate::engine::print_dry_run(&config, session.current_step.as_deref());
    Ok(())
}

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
    for (index, result) in results.iter().enumerate() {
        let title = crate::display::truncate(result.input_or_title(), MAX_INPUT_CHARS);
        let line = match &result.phase {
            SessionPhase::Completed => {
                let pr = result
                    .pr_url
                    .as_deref()
                    .map(|url| format!(" {} {url}", style("->").yellow()))
                    .unwrap_or_default();
                format!(
                    "[{}] {} {}{}",
                    index + 1,
                    style("v").green().bold(),
                    title,
                    pr
                )
            }
            SessionPhase::Failed(error) => format!(
                "[{}] {} {} -- Failed: {}",
                index + 1,
                style("x").red().bold(),
                title,
                error
            ),
            SessionPhase::PrFailed(error) => format!(
                "[{}] {} {} -- PR Failed: {}",
                index + 1,
                style("x").red().bold(),
                title,
                error
            ),
            SessionPhase::Suspended => format!(
                "[{}] {} {} -- Suspended",
                index + 1,
                style("||").yellow().bold(),
                title
            ),
            SessionPhase::Draft
            | SessionPhase::AwaitingInput
            | SessionPhase::AwaitingApproval
            | SessionPhase::Planned
            | SessionPhase::Running => format!("[{}] ? {}", index + 1, title),
        };
        if result.auto_requeue_count > 0 {
            lines.push(format!(
                "{line} (auto-requeued {})",
                result.auto_requeue_count
            ));
        } else {
            lines.push(line);
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_all_defaults_to_one_worker_without_a_cli_override() {
        assert_eq!(run_all_parallelism(None), 1);
        assert_eq!(run_all_parallelism(Some(4)), 4);
    }

    #[test]
    fn dismissed_batch_prompt_queues_only_its_session_for_pause() {
        let mut paused_sessions = Vec::new();
        let mut pending_pauses = Vec::new();

        queue_pause_for_session(&mut paused_sessions, &mut pending_pauses, "session-a");

        assert_eq!(paused_sessions, vec!["session-a".to_owned()]);
        assert_eq!(pending_pauses, vec!["session-a".to_owned()]);
    }
}
