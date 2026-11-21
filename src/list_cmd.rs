use std::fmt::{self, Write as _};
use std::io::{IsTerminal, Write};

use console::style;
use inquire::InquireError;

use crate::application::{SessionAction as Capability, SessionSettingsRequest};
use crate::cli::{DEFAULT_RATE_LIMIT_RETRIES, ListArgs};
use crate::daemon::client::list::SessionListItem;
use crate::daemon::client::{DaemonClient, EventFilter};
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};
use crate::request_context::RequestContext;
use crate::session::{SessionManager, SessionPhase, SessionState};
use crate::workflow::{list_skippable_after_pr_steps, list_skippable_steps};

#[expect(
    clippy::too_many_lines,
    reason = "The interactive list loop coordinates the session selection and action menu."
)]
pub async fn run(args: ListArgs) -> Result<()> {
    let client = DaemonClient::connect().await?;
    if args.json {
        let sessions = client.sessions().await?;
        let mut writer = std::io::BufWriter::new(std::io::stdout().lock());
        serde_json::to_writer_pretty(&mut writer, &sessions)
            .map_err(|error| CruiseError::Other(format!("JSON serialization error: {error}")))?;
        writer
            .write_all(b"\n")
            .map_err(|error| CruiseError::Other(format!("write error: {error}")))?;
        return Ok(());
    }

    let context = RequestContext::capture()?;
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    loop {
        let id = match pick_list_choice(&client, interactive).await? {
            None | Some(ListChoice::Quit) => return Ok(()),
            Some(ListChoice::Session { id, .. }) => id,
            Some(choice) => {
                match run_global_action(&choice, &client, &context).await {
                    Ok(()) | Err(CruiseError::StepPaused | CruiseError::Interrupted) => {}
                    Err(error) => eprintln!("{error}"),
                }
                continue;
            }
        };
        let mut details = client.session_details(&id).await?;
        loop {
            if let Ok(content) = client.session_plan(&id).await {
                crate::display::print_bordered(&content, Some("plan.md"));
            }
            let actions = available_session_actions(&details.capabilities);
            crate::platform::reclaim_terminal_foreground();
            let prompt = list_text("cli.list.action_prompt", "Action:");
            let action = match inquire::Select::new(&prompt, actions).prompt() {
                Ok(action) => action,
                Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                    SessionAction::Back
                }
                Err(error) => {
                    return Err(CruiseError::Other(format!("selection error: {error}")));
                }
            };

            match action {
                SessionAction::ViewDiff => {
                    crate::platform::reclaim_terminal_foreground();
                    if let Err(error) =
                        crate::diff_cmd::print_session_diff(&client, &id, false).await
                    {
                        eprintln!("{error}");
                    }
                }
                SessionAction::GeneratePlan => {
                    if let Err(error) = crate::plan_cmd::regenerate_plan_for_session(
                        &client,
                        &id,
                        DEFAULT_RATE_LIMIT_RETRIES,
                    )
                    .await
                    {
                        eprintln!("{} Plan generation failed: {error}", style("✗").red());
                    }
                    if let Ok(updated) = client.session_details(&id).await {
                        details = updated;
                    }
                }
                SessionAction::Approve => {
                    let request = crate::application::ApproveRequest {
                        flags: None,
                        skipped_steps: None,
                        generate_title: true,
                        context: Some(context.clone()),
                    };
                    match client.approve(&id, &request).await {
                        Ok(approved) => {
                            eprintln!(
                                "{} Session {} approved. Run with: {}",
                                style("v").green(),
                                approved.id,
                                style(format!("cruise run {}", approved.id)).cyan()
                            );
                        }
                        Err(error) => eprintln!("{error}"),
                    }
                    if let Ok(updated) = client.session_details(&id).await {
                        details = updated;
                    }
                }
                SessionAction::PublishAsIssue => {
                    let default_trigger_cruise =
                        matches!(&details.state.phase, SessionPhase::Planned);
                    let trigger_cruise =
                        if crate::issue_publish::supports_trigger_cruise(&details.state) {
                            let prompt = list_text(
                                "cli.list.prompt.trigger",
                                "Post an @cruise run comment after creating the issue?",
                            );
                            match inquire::Confirm::new(&prompt)
                                .with_default(default_trigger_cruise)
                                .prompt()
                            {
                                Ok(answer) => answer,
                                Err(
                                    InquireError::OperationCanceled
                                    | InquireError::OperationInterrupted,
                                ) => continue,
                                Err(error) => {
                                    return Err(CruiseError::Other(format!(
                                        "selection error: {error}"
                                    )));
                                }
                            }
                        } else {
                            false
                        };
                    match client.publish(&id, trigger_cruise).await {
                        Ok(url) => {
                            eprintln!("{} Published plan as issue: {url}", style("v").green());
                            break;
                        }
                        Err(error) => {
                            eprintln!(
                                "{} Failed to publish plan as issue: {error}",
                                style("x").red()
                            );
                            if let Ok(updated) = client.session_details(&id).await {
                                details = updated;
                            }
                        }
                    }
                }
                SessionAction::Run | SessionAction::Resume => {
                    return crate::run_cmd::run(crate::cli::RunArgs {
                        session: Some(id),
                        all: false,
                        parallelism: None,
                        auto_requeue_max_attempts: None,
                        max_retries: None,
                        rate_limit_retries: DEFAULT_RATE_LIMIT_RETRIES,
                        dry_run: false,
                        cleanup_after_pr: false,
                        no_cleanup_after_pr: false,
                        flags: vec![],
                    })
                    .await;
                }
                SessionAction::Tell => {
                    let instruction = match prompt_multiline("Instruction:")? {
                        InputResult::Submitted(instruction) => instruction,
                        InputResult::Cancelled => continue,
                    };
                    let request = crate::application::TellRequest {
                        content: instruction,
                        origin: "list".to_owned(),
                        dedupe_key: None,
                        context: Some(context.clone()),
                    };
                    match client.tell(&id, &request).await {
                        Ok(receipt) => eprintln!("Instruction #{} accepted", receipt.id),
                        Err(error) => eprintln!("{error}"),
                    }
                }
                SessionAction::Replan => {
                    let prompt =
                        list_text("cli.list.prompt.replan", "Describe the changes needed:");
                    let text = match prompt_multiline(&prompt)? {
                        InputResult::Submitted(text) => text,
                        InputResult::Cancelled => continue,
                    };
                    crate::plan_cmd::replan_session(&client, &id, text, DEFAULT_RATE_LIMIT_RETRIES)
                        .await?;
                    details = client.session_details(&id).await?;
                }
                SessionAction::CreatePr => {
                    let stream = client
                        .events(EventFilter {
                            session: Some(id.clone()),
                            batch: true,
                        })
                        .await?;
                    if let Err(error) = client.start_create_pr(&id, &context).await {
                        eprintln!("PR creation failed: {error}");
                    } else {
                        let mut handler = crate::cli_events::TerminalEvents::new(&client, &id);
                        match crate::cli_events::drive(&client, &id, stream, &mut handler).await {
                            Ok(finished) => {
                                if let SessionPhase::PrFailed(error) | SessionPhase::Failed(error) =
                                    finished.state.phase
                                {
                                    eprintln!("PR creation failed: {error}");
                                }
                            }
                            Err(error) => eprintln!("PR creation failed: {error}"),
                        }
                    }
                    if let Ok(updated) = client.session_details(&id).await {
                        details = updated;
                    }
                }
                SessionAction::OpenPr => {
                    let url = details.state.pr_url.as_deref().ok_or_else(|| {
                        CruiseError::Other("Open PR action requires pr_url".into())
                    })?;
                    match open_pr_in_browser(url) {
                        Ok(()) => eprintln!("{} Opening PR in browser...", style("v").green()),
                        Err(error) => eprintln!("{} {error}", style("x").red()),
                    }
                }
                SessionAction::MergePr => {
                    if merge_pr_interactive(&client, &id, &context).await? {
                        break;
                    }
                    if let Ok(updated) = client.session_details(&id).await {
                        details = updated;
                    }
                }
                SessionAction::StageLocalMerge | SessionAction::MergeLocal => {
                    let mode = if action == SessionAction::StageLocalMerge {
                        crate::application::LocalMergeMode::Stage
                    } else {
                        crate::application::LocalMergeMode::MergeAndCleanup
                    };
                    if local_merge_interactive(&client, &details.state, mode, &context).await? {
                        break;
                    }
                    if let Ok(updated) = client.session_details(&id).await {
                        details = updated;
                    }
                }
                SessionAction::EditSettings => {
                    if let Err(error) =
                        edit_session_settings_interactive(&client, &details.state, &context).await
                    {
                        eprintln!("{} Edit settings failed: {error}", style("✗").red());
                    }
                    if let Ok(updated) = client.session_details(&id).await {
                        details = updated;
                    }
                }
                SessionAction::ResetToPlanned => {
                    client.reset_session(&id, &context).await?;
                    eprintln!("{} Session {id} reset to Planned.", style("v").green());
                    details = client.session_details(&id).await?;
                }
                SessionAction::Discard => {
                    client.discard_session(&id, &context).await?;
                    eprintln!("{} Session {id} discarded.", style("v").green());
                    break;
                }
                SessionAction::Delete => {
                    client.delete_session(&id, &context).await?;
                    eprintln!("{} Session {id} deleted.", style("v").green());
                    break;
                }
                SessionAction::Back => break,
            }
        }
    }
}

async fn pick_list_choice(client: &DaemonClient, interactive: bool) -> Result<Option<ListChoice>> {
    let sessions = client.sessions().await?;
    if sessions.is_empty() {
        eprintln!(
            "{}",
            crate::i18n::t(crate::i18n::process_locale(), "cli.list.empty")
        );
        if !interactive {
            return Ok(None);
        }
    }
    let (choices, cursor) = if interactive {
        list_choices(sessions)
    } else {
        (sessions.into_iter().map(ListChoice::session).collect(), 0)
    };
    crate::platform::reclaim_terminal_foreground();
    let prompt = if interactive {
        list_text("cli.list.action_prompt", "Action:")
    } else {
        list_text("cli.list.select_session", "Select a session:")
    };
    prompt_result(
        inquire::Select::new(&prompt, choices)
            .with_starting_cursor(cursor)
            .prompt(),
    )
}

fn prompt_result<T>(result: std::result::Result<T, InquireError>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(error) => Err(CruiseError::Other(format!("selection error: {error}"))),
    }
}

fn confirm_global_action(prompt: &str) -> Result<bool> {
    crate::platform::reclaim_terminal_foreground();
    Ok(prompt_result(inquire::Confirm::new(prompt).with_default(false).prompt())? == Some(true))
}

async fn run_global_action(
    choice: &ListChoice,
    client: &DaemonClient,
    context: &RequestContext,
) -> Result<()> {
    use crate::run_cmd::RunAllAvailability;

    match choice {
        ListChoice::NewSession => new_session().await,
        ListChoice::Clean => {
            let prompt = list_text(
                "cli.list.prompt.clean_confirm",
                "Clean up eligible sessions?",
            );
            if confirm_global_action(&prompt)? {
                crate::clean_cmd::clean_and_report(client, context).await?;
            }
            Ok(())
        }
        ListChoice::RunAll => {
            let availability = crate::run_cmd::run_all_availability(client).await?;
            let attach_only = matches!(availability, RunAllAvailability::Active);
            let prompt = match availability {
                RunAllAvailability::Active => list_text(
                    "cli.list.prompt.run_all_attach_confirm",
                    "Run All: Attach to the active batch?",
                ),
                RunAllAvailability::Ready => list_text(
                    "cli.list.prompt.run_all_confirm",
                    "Run All: Start eligible sessions?",
                ),
                RunAllAvailability::NoWork => {
                    eprintln!(
                        "{}",
                        list_text(
                            "cli.list.run_all_no_work",
                            "No Planned or Suspended sessions to run."
                        )
                    );
                    return Ok(());
                }
            };
            if !confirm_global_action(&prompt)? {
                return Ok(());
            }
            crate::run_cmd::run_with_batch_intent(crate::cli::RunArgs {
                session: None,
                all: true,
                parallelism: None,
                auto_requeue_max_attempts: None,
                max_retries: None,
                rate_limit_retries: DEFAULT_RATE_LIMIT_RETRIES,
                dry_run: false,
                cleanup_after_pr: false,
                no_cleanup_after_pr: false,
                flags: vec![],
            }, attach_only)
            .await
        }
        ListChoice::Session { .. } | ListChoice::Quit => unreachable!("not a global action"),
    }
}

enum NewSessionMode {
    Interactive,
    Background,
    Draft,
}

impl fmt::Display for NewSessionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Interactive => {
                list_text("cli.list.action.plan_interactively", "Plan interactively")
            }
            Self::Background => {
                list_text("cli.list.action.plan_in_background", "Plan in background")
            }
            Self::Draft => list_text("cli.list.action.save_draft", "Save draft"),
        };
        f.write_str(&label)
    }
}

async fn new_session() -> Result<()> {
    crate::platform::reclaim_terminal_foreground();
    let prompt = list_text(
        "cli.list.prompt.new_session_prompt",
        "How would you like to create the session?",
    );
    let modes = vec![
        NewSessionMode::Interactive,
        NewSessionMode::Background,
        NewSessionMode::Draft,
    ];
    match prompt_result(inquire::Select::new(&prompt, modes).prompt())? {
        None => Ok(()),
        Some(NewSessionMode::Interactive) => {
            Box::pin(crate::plan_cmd::run(crate::cli::PlanArgs {
                input: None,
                pr: None,
                issue: None,
                config: None,
                dry_run: false,
                no_force_exec: true,
                skip_planning: false,
                grill: false,
                no_interactive_planning: false,
                repo: None,
                rate_limit_retries: DEFAULT_RATE_LIMIT_RETRIES,
                images: vec![],
                flags: vec![],
            }))
            .await
        }
        Some(NewSessionMode::Background) => {
            let prompt = list_text(
                "cli.list.prompt.task_prompt",
                "What would you like to implement?",
            );
            let InputResult::Submitted(task) = prompt_multiline(&prompt)? else {
                return Ok(());
            };
            crate::plan_cmd::launch_background_task(&task, false, None, &[], true, &[]).await
        }
        Some(NewSessionMode::Draft) => {
            crate::draft_cmd::run(crate::cli::DraftArgs {
                input: None,
                issue: None,
                config: None,
            })
            .await
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionAction {
    ViewDiff,
    Tell,
    CreatePr,
    StageLocalMerge,
    MergeLocal,
    GeneratePlan,
    Approve,
    PublishAsIssue,
    EditSettings,
    Run,
    Replan,
    Resume,
    ResetToPlanned,
    OpenPr,
    MergePr,
    Discard,
    Delete,
    Back,
}

impl SessionAction {
    fn key(self) -> &'static str {
        match self {
            Self::ViewDiff => "cli.list.action.view_diff",
            Self::Tell => "cli.list.action.tell",
            Self::CreatePr => "cli.list.action.create_pr",
            Self::StageLocalMerge => "cli.list.action.stage_local_merge",
            Self::MergeLocal => "cli.list.action.merge_local",
            Self::GeneratePlan => "cli.list.action.generate_plan",
            Self::Approve => "cli.list.action.approve",
            Self::PublishAsIssue => "cli.list.action.publish_as_issue",
            Self::EditSettings => "cli.list.action.edit_settings",
            Self::Run => "cli.list.action.run",
            Self::Replan => "cli.list.action.replan",
            Self::Resume => "cli.list.action.resume",
            Self::ResetToPlanned => "cli.list.action.reset_to_planned",
            Self::OpenPr => "cli.list.action.open_pr",
            Self::MergePr => "cli.list.action.merge_pr",
            Self::Discard => "cli.list.action.discard",
            Self::Delete => "cli.list.action.delete",
            Self::Back => "cli.list.action.back",
        }
    }

    fn english_label(self) -> &'static str {
        match self {
            Self::ViewDiff => "View Diff",
            Self::Tell => "Tell",
            Self::CreatePr => "Create PR",
            Self::StageLocalMerge => "Stage local merge",
            Self::MergeLocal => "Merge locally & cleanup",
            Self::GeneratePlan => "Generate Plan",
            Self::Approve => "Approve",
            Self::PublishAsIssue => "Publish as Issue",
            Self::EditSettings => "Edit Settings",
            Self::Run => "Run",
            Self::Replan => "Replan",
            Self::Resume => "Resume",
            Self::ResetToPlanned => "Reset to Planned",
            Self::OpenPr => "Open PR",
            Self::MergePr => "Merge PR",
            Self::Discard => "Discard",
            Self::Delete => "Delete",
            Self::Back => "Back",
        }
    }
}

impl fmt::Display for SessionAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&list_text(self.key(), self.english_label()))
    }
}

fn list_text(key: &str, fallback: &str) -> String {
    let translated = crate::i18n::t(crate::i18n::process_locale(), key);
    if translated == key {
        fallback.to_owned()
    } else {
        translated
    }
}

fn available_session_actions(capabilities: &[Capability]) -> Vec<SessionAction> {
    let mut actions = Vec::with_capacity(capabilities.len() + 2);
    for capability in capabilities {
        let action = match capability {
            Capability::ViewDiff => SessionAction::ViewDiff,
            Capability::Tell => SessionAction::Tell,
            Capability::CreatePr => SessionAction::CreatePr,
            Capability::StageLocalMerge => SessionAction::StageLocalMerge,
            Capability::MergeLocally => SessionAction::MergeLocal,
            Capability::Generate => SessionAction::GeneratePlan,
            Capability::Approve => SessionAction::Approve,
            Capability::Publish => SessionAction::PublishAsIssue,
            Capability::EditSettings => SessionAction::EditSettings,
            Capability::RunWorktree | Capability::RunCurrentBranch | Capability::Retry => {
                SessionAction::Run
            }
            Capability::Fix => SessionAction::Replan,
            Capability::Resume => SessionAction::Resume,
            Capability::ResetToPlanned => SessionAction::ResetToPlanned,
            Capability::OpenPr => SessionAction::OpenPr,
            Capability::MergePr => SessionAction::MergePr,
            Capability::Discard => SessionAction::Discard,
            Capability::Delete
            | Capability::Answer
            | Capability::Cancel
            | Capability::Ask
            | Capability::Verify
            | Capability::EditCurrentStep => continue,
        };
        if !actions.contains(&action) {
            actions.push(action);
        }
    }
    if let Some(index) = actions
        .iter()
        .position(|action| *action == SessionAction::Replan)
    {
        let replan = actions.remove(index);
        let insertion = actions
            .iter()
            .position(|action| *action == SessionAction::EditSettings)
            .map_or(actions.len(), |index| index + 1);
        actions.insert(insertion, replan);
    }
    if let Some(index) = actions
        .iter()
        .position(|action| *action == SessionAction::ResetToPlanned)
    {
        let reset = actions.remove(index);
        let insertion = actions
            .iter()
            .position(|action| *action == SessionAction::EditSettings)
            .map_or(actions.len(), |index| index + 1);
        actions.insert(insertion, reset);
    }
    if capabilities.contains(&Capability::Delete) {
        actions.push(SessionAction::Delete);
    }
    actions.push(SessionAction::Back);
    actions
}

async fn local_merge_interactive(
    client: &DaemonClient,
    session: &SessionState,
    mode: crate::application::LocalMergeMode,
    context: &crate::request_context::RequestContext,
) -> Result<bool> {
    use crate::application::{LocalMergeMode, LocalMergeOutcome};

    let branch = session.worktree_branch.as_deref().unwrap_or("unknown");
    let target = crate::local_merge::merge_target_branch(session).unwrap_or("unknown");
    let prompt = match mode {
        LocalMergeMode::Stage => format!("Stage squash of {branch} into {target}?"),
        LocalMergeMode::MergeAndCleanup => format!(
            "Squash {branch} into {target}, commit, and delete the worktree, branch, and session?"
        ),
    };
    match inquire::Confirm::new(&prompt).with_default(false).prompt() {
        Ok(true) => {}
        Ok(false) | Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            return Ok(false);
        }
        Err(error) => return Err(CruiseError::Other(format!("selection error: {error}"))),
    }
    match client.local_merge(&session.id, mode, context).await {
        Ok(LocalMergeOutcome::Staged) => {
            eprintln!(
                "{} Squash staged in base checkout; session retained.",
                style("v").green()
            );
            Ok(false)
        }
        Ok(LocalMergeOutcome::Merged) => {
            eprintln!("{} Merged locally; session cleaned up.", style("v").green());
            Ok(true)
        }
        Ok(LocalMergeOutcome::MergedCleanupFailed { error }) => {
            eprintln!(
                "{} Merge committed but cleanup failed: {error}. Retry cleanup from this session.",
                style("!").yellow()
            );
            Ok(false)
        }
        Err(error) => {
            eprintln!("{} {error}", style("x").red());
            Ok(false)
        }
    }
}

/// Displayable choice for a skip-step multi-select.
struct StepChoice {
    label: String,
    expanded_id: String,
}

impl fmt::Display for StepChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

/// Edit session flags, skipped steps, and the current resume step.
#[expect(
    clippy::too_many_lines,
    reason = "interactive UI with multiple branches"
)]
async fn edit_session_settings_interactive(
    client: &DaemonClient,
    session: &SessionState,
    context: &crate::request_context::RequestContext,
) -> Result<()> {
    use crate::application::CurrentStepUpdateDto;

    let manager = SessionManager::new(crate::paths::data_dir()?);
    let config = manager.load_config(session).ok();
    let declared_flags = config
        .as_ref()
        .map(crate::flags::choices)
        .unwrap_or_default();
    let flags = if declared_flags.is_empty() {
        None
    } else {
        let mut values = session.flags.clone();
        for flag in &declared_flags {
            let current = match session.flags.get(&flag.name) {
                Some(true) => "on",
                Some(false) => "off",
                None => "unset",
            };
            let keep = format!("Keep (current: {current})");
            let options = vec![keep.clone(), "on".to_string(), "off".to_string()];
            let message = format!("Flag '{}':", flag.name);
            let mut prompt = inquire::Select::new(&message, options);
            if let Some(description) = flag.description.as_deref() {
                prompt = prompt.with_help_message(description);
            }
            match prompt.prompt() {
                Ok(choice) if choice == keep => {}
                Ok(choice) => {
                    values.insert(flag.name.clone(), choice == "on");
                }
                Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                    return Ok(());
                }
                Err(error) => {
                    return Err(CruiseError::Other(format!("selection error: {error}")));
                }
            }
        }
        Some(values)
    };

    let (main_nodes, after_pr_nodes) = match config.as_ref() {
        Some(config) => (
            list_skippable_steps(config).unwrap_or_default(),
            list_skippable_after_pr_steps(config).unwrap_or_default(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let effective_flags = flags.as_ref().unwrap_or(&session.flags);
    let skip_main_nodes = crate::flags::skip_candidates(&main_nodes, effective_flags);
    let skip_after_nodes = crate::flags::skip_candidates(&after_pr_nodes, effective_flags);
    let skip_main_ids: Vec<String> = skip_main_nodes
        .iter()
        .flat_map(|node| node.expanded_step_ids.clone())
        .collect();
    let after_ids: Vec<String> = skip_after_nodes
        .iter()
        .flat_map(|node| node.expanded_step_ids.clone())
        .collect();
    let resume_ids: Vec<&str> =
        if session.execution_stage == crate::session::SessionExecutionStage::AfterPr {
            after_pr_nodes
                .iter()
                .flat_map(|node| node.expanded_step_ids.iter().map(String::as_str))
                .collect()
        } else {
            main_nodes
                .iter()
                .flat_map(|node| node.expanded_step_ids.iter().map(String::as_str))
                .collect()
        };
    let choices: Vec<StepChoice> = skip_main_ids
        .iter()
        .map(|id| StepChoice {
            label: id.clone(),
            expanded_id: id.clone(),
        })
        .chain(after_ids.iter().map(|id| StepChoice {
            label: format!("[after-pr] {id}"),
            expanded_id: id.clone(),
        }))
        .collect();
    let skipped_steps = if choices.is_empty() {
        eprintln!(
            "{}",
            list_text(
                "cli.list.prompt.no_steps",
                "(no steps available for selected flags)"
            )
        );
        crate::flags::drop_disabled_skips(
            session.skipped_steps.clone(),
            main_nodes.iter().chain(&after_pr_nodes),
            effective_flags,
        )
    } else {
        let defaults: Vec<usize> = choices
            .iter()
            .enumerate()
            .filter(|(_, choice)| session.skipped_steps.contains(&choice.expanded_id))
            .map(|(index, _)| index)
            .collect();
        let prompt = list_text("cli.list.prompt.skip_steps", "Steps to skip:");
        match inquire::MultiSelect::new(&prompt, choices)
            .with_default(&defaults)
            .prompt()
        {
            Ok(selected) => selected
                .into_iter()
                .map(|choice| choice.expanded_id)
                .collect(),
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                return Ok(());
            }
            Err(error) => return Err(CruiseError::Other(format!("selection error: {error}"))),
        }
    };

    let current_step_update = if matches!(
        &session.phase,
        SessionPhase::Failed(_) | SessionPhase::Suspended
    ) && !resume_ids.is_empty()
    {
        let keep = format!(
            "Keep ({})",
            session.current_step.as_deref().unwrap_or("from beginning")
        );
        let clear = "From beginning (clear)".to_string();
        let mut choices = vec![keep.clone(), clear.clone()];
        choices.extend(resume_ids.iter().map(|id| (*id).to_owned()));
        let prompt = list_text("cli.list.prompt.resume_step", "Resume from step:");
        match inquire::Select::new(&prompt, choices).prompt() {
            Ok(choice) if choice == keep => CurrentStepUpdateDto::Unchanged,
            Ok(choice) if choice == clear => CurrentStepUpdateDto::Clear,
            Ok(step_name) => CurrentStepUpdateDto::Set(step_name),
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                return Ok(());
            }
            Err(error) => return Err(CruiseError::Other(format!("selection error: {error}"))),
        }
    } else {
        CurrentStepUpdateDto::Unchanged
    };

    client
        .update_session_settings(
            &session.id,
            &SessionSettingsRequest {
                config_path: None,
                skipped_steps,
                current_step_update,
                flags,
            },
            context,
        )
        .await?;
    eprintln!("{} Settings updated.", style("v").green());
    Ok(())
}

/// Preview, choose a method, confirm and merge. Returns true when cleanup removed the session.
async fn merge_pr_interactive(
    client: &DaemonClient,
    id: &str,
    context: &crate::request_context::RequestContext,
) -> Result<bool> {
    use crate::application::MergePrOutcome;

    let status = match client.inspect_merge(id).await {
        Ok(status) => status,
        Err(error) => {
            eprintln!("{} {error}", style("x").red());
            return Ok(false);
        }
    };
    eprintln!("PR state: {}", status.state);
    eprintln!("Mergeable: {}", status.mergeable);
    eprintln!("Review decision: {}", status.review_decision);
    if status.checks.is_empty() {
        eprintln!("Checks: none");
    }
    for check in &status.checks {
        eprintln!("Check: {} ({})", check.name, check.status);
    }
    if status.state != "OPEN" {
        eprintln!(
            "{} PR is {}. Run `cruise clean` to remove the session.",
            style("!").yellow(),
            status.state
        );
        return Ok(false);
    }
    let methods = vec!["Squash", "Merge", "Rebase"];
    let prompt = list_text("cli.list.prompt.merge_method", "Merge method:");
    let method_label = match inquire::Select::new(&prompt, methods).prompt() {
        Ok(method) => method,
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            return Ok(false);
        }
        Err(error) => return Err(CruiseError::Other(format!("selection error: {error}"))),
    };
    let method = match method_label {
        "Merge" => crate::pr_merge::PrMergeMethod::Merge,
        "Rebase" => crate::pr_merge::PrMergeMethod::Rebase,
        _ => crate::pr_merge::PrMergeMethod::Squash,
    };
    let prompt = format!(
        "Merge PR with {method_label} (mergeable: {}, review: {})?",
        status.mergeable, status.review_decision
    );
    match inquire::Confirm::new(&prompt).with_default(false).prompt() {
        Ok(true) => {}
        Ok(false) | Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            return Ok(false);
        }
        Err(error) => return Err(CruiseError::Other(format!("selection error: {error}"))),
    }
    match client.merge_pr(id, method, context).await {
        Ok(MergePrOutcome::Cleaned) => {
            eprintln!("{} PR merged; session cleaned up.", style("v").green());
            Ok(true)
        }
        Ok(MergePrOutcome::Pending) => {
            eprintln!(
                "{} PR is still open (for example in a merge queue). The session was kept.",
                style("!").yellow()
            );
            Ok(false)
        }
        Err(error) => {
            eprintln!("{} {error}", style("x").red());
            Ok(false)
        }
    }
}

fn open_pr_in_browser(pr_url: &str) -> crate::error::Result<()> {
    crate::platform::open_url(pr_url)
        .map_err(|e| CruiseError::Other(format!("failed to open {pr_url}: {e}")))
}

fn format_session_label(session: &SessionListItem) -> String {
    let (icon, phase_str) = match session.phase.as_str() {
        "Draft" => (
            style("○").dim(),
            style(list_text("cli.list.phase.draft", "Draft"))
                .dim()
                .to_string(),
        ),
        "AwaitingInput" => (
            style("?").yellow(),
            style(list_text("cli.list.phase.awaiting_input", "Awaiting Input"))
                .yellow()
                .to_string(),
        ),
        "AwaitingApproval" if session.plan_error.is_some() => (
            style("✗").red(),
            style(list_text("cli.list.phase.plan_failed", "Plan Failed"))
                .red()
                .to_string(),
        ),
        "AwaitingApproval" if !session.plan_available => (
            style("-").yellow(),
            style(list_text("cli.list.phase.planning", "Planning"))
                .yellow()
                .to_string(),
        ),
        "AwaitingApproval" => (
            style("o").magenta(),
            style(list_text(
                "cli.list.phase.awaiting_approval",
                "Awaiting Approval",
            ))
            .magenta()
            .to_string(),
        ),
        "Planned" => (
            style("o").cyan(),
            style(list_text("cli.list.phase.planned", "Planned"))
                .cyan()
                .to_string(),
        ),
        "Running" => (
            style(">").yellow(),
            style(list_text("cli.list.phase.running", "Running"))
                .yellow()
                .to_string(),
        ),
        "Completed" => (
            style("v").green(),
            style(list_text("cli.list.phase.completed", "Completed"))
                .green()
                .to_string(),
        ),
        "Failed" => (
            style("x").red(),
            style(list_text("cli.list.phase.failed", "Failed"))
                .red()
                .to_string(),
        ),
        "PrFailed" => (
            style("x").red(),
            style(list_text("cli.list.phase.pr_failed", "PR Failed"))
                .red()
                .to_string(),
        ),
        "Suspended" => (
            style("||").yellow(),
            style(list_text("cli.list.phase.suspended", "Suspended"))
                .yellow()
                .to_string(),
        ),
        _ => (
            style("?").dim(),
            style(session.phase.as_str()).dim().to_string(),
        ),
    };
    let date = format_session_date(&session.id);
    let suffix = format_suffix(session);
    let title = session
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(&session.input);
    let preview = crate::display::truncate(title, 60);
    format!("{icon} {date} {phase_str} {preview}{suffix}")
}

/// "`YYYYMMDDHHmmss`" -> "MM/DD HH:MM"
fn format_session_date(id: &str) -> String {
    let (Some(month), Some(day), Some(hour), Some(min)) =
        (id.get(4..6), id.get(6..8), id.get(8..10), id.get(10..12))
    else {
        return id.to_string();
    };
    format!("{month}/{day} {hour}:{min}")
}

fn format_suffix(session: &SessionListItem) -> String {
    let mut suffix = match session.phase.as_str() {
        "Running" | "Suspended" => session
            .current_step
            .as_ref()
            .map(|step| format!(" [{step}]"))
            .unwrap_or_default(),
        "Completed" => session
            .pr_url
            .as_ref()
            .map(|url| {
                let number = url.trim_end_matches('/').rsplit('/').next().unwrap_or("");
                format!(" PR#{number}")
            })
            .unwrap_or_default(),
        _ => String::new(),
    };
    if session.auto_requeue_count > 0 {
        let _ = write!(suffix, " auto-requeued {}", session.auto_requeue_count);
    }
    suffix
}
enum ListChoice {
    NewSession,
    Clean,
    RunAll,
    Session { id: String, label: String },
    Quit,
}

impl ListChoice {
    fn session(session: SessionListItem) -> Self {
        let label = format_session_label(&session);
        Self::Session {
            id: session.id,
            label,
        }
    }
}

impl fmt::Display for ListChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::NewSession => list_text("cli.list.action.new_session", "New Session"),
            Self::Clean => list_text("cli.list.action.clean", "Clean"),
            Self::RunAll => list_text("cli.list.action.run_all", "Run All"),
            Self::Quit => list_text("cli.list.action.quit", "Quit"),
            Self::Session { label, .. } => return f.write_str(label),
        };
        f.write_str(&label)
    }
}

fn list_choices(sessions: Vec<SessionListItem>) -> (Vec<ListChoice>, usize) {
    let mut choices = vec![
        ListChoice::NewSession,
        ListChoice::Clean,
        ListChoice::RunAll,
    ];
    let cursor = if sessions.is_empty() {
        0
    } else {
        choices.len()
    };
    choices.extend(sessions.into_iter().map(ListChoice::session));
    choices.push(ListChoice::Quit);
    (choices, cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_row(id: &str, title: &str) -> SessionListItem {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "base_dir": "/project",
            "phase": "Draft",
            "config_source": "cruise.yml",
            "input": "task",
            "title": title,
            "created_at": "2026-01-01T12:00:00Z",
            "workspace_mode": "Worktree",
            "awaiting_input": false,
            "plan_available": false,
            "pending_instructions": 0,
            "auto_requeue_count": 0
        }))
        .unwrap_or_else(|error| panic!("list row fixture: {error}"))
    }

    #[test]
    fn empty_selector_offers_global_actions_and_starts_on_new_session() {
        let (choices, cursor) = list_choices(vec![]);
        assert!(matches!(
            choices.as_slice(),
            [
                ListChoice::NewSession,
                ListChoice::Clean,
                ListChoice::RunAll,
                ListChoice::Quit
            ]
        ));
        assert!(matches!(choices.get(cursor), Some(ListChoice::NewSession)));
    }

    #[test]
    fn populated_selector_preserves_session_order_and_starts_on_first_session() {
        let [first_id, second_id] = ["20260101120100", "20260101120000"];
        let rows = vec![session_row(first_id, "First"), session_row(second_id, "Second")];
        let (choices, cursor) = list_choices(rows);
        assert!(matches!(
            choices.as_slice(),
            [ListChoice::NewSession, ListChoice::Clean, ListChoice::RunAll,
                ListChoice::Session { id: first, .. }, ListChoice::Session { id: second, .. }, ListChoice::Quit]
                if first == first_id && second == second_id
        ));
        assert!(matches!(
            choices.get(cursor),
            Some(ListChoice::Session { id, .. }) if id == first_id
        ));
    }

    #[test]
    fn identical_display_labels_retain_distinct_session_ids() {
        let [first_id, second_id] = ["20260101120001", "20260101120002"];
        let rows = vec![
            session_row(first_id, "Same title"),
            session_row(second_id, "Same title"),
        ];
        let (choices, _) = list_choices(rows);
        let sessions: Vec<_> = choices
            .iter()
            .filter_map(|choice| match choice {
                ListChoice::Session { id, label } => Some((id, label)),
                _ => None,
            })
            .collect();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].1, sessions[1].1);
        assert_eq!(sessions[0].0, first_id);
        assert_eq!(sessions[1].0, second_id);
    }

    #[test]
    fn action_menu_uses_daemon_capabilities() {
        let capabilities = [
            Capability::RunWorktree,
            Capability::Publish,
            Capability::Fix,
            Capability::EditSettings,
            Capability::Delete,
        ];
        assert_eq!(
            available_session_actions(&capabilities),
            vec![
                SessionAction::Run,
                SessionAction::PublishAsIssue,
                SessionAction::EditSettings,
                SessionAction::Replan,
                SessionAction::Delete,
                SessionAction::Back,
            ]
        );

        let running = [Capability::Tell, Capability::ResetToPlanned];
        assert_eq!(
            available_session_actions(&running),
            vec![
                SessionAction::Tell,
                SessionAction::ResetToPlanned,
                SessionAction::Back,
            ]
        );
        let failed = [
            Capability::Retry,
            Capability::ResetToPlanned,
            Capability::EditSettings,
            Capability::Delete,
        ];
        assert_eq!(
            available_session_actions(&failed),
            vec![
                SessionAction::Run,
                SessionAction::EditSettings,
                SessionAction::ResetToPlanned,
                SessionAction::Delete,
                SessionAction::Back,
            ]
        );
    }
}
