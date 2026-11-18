use std::collections::HashSet;
use std::fmt;
use std::io::{IsTerminal as _, Read};
use std::path::{Path, PathBuf};

use console::style;
use inquire::InquireError;

use crate::application::{
    ApproveRequest, Interactive, NewSessionImage, NewSessionRequest, PlanRequest,
};
use crate::cli::{DEFAULT_RATE_LIMIT_RETRIES, PLAN_STDIN_SENTINEL, PlanArgs};
use crate::config::WorkflowConfig;
use crate::daemon::client::{DaemonClient, EventFilter, SessionEventHandler};
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};
use crate::new_session_history::{HistoryScope, NewSessionHistory};
use crate::resolver::ConfigSource;
use crate::session::{SessionPhase, SessionState};
use crate::workflow::SkippableStepNode;

#[expect(
    clippy::large_enum_variant,
    reason = "local targets carry their resolved config to avoid reparsing"
)]
enum PlanTarget {
    Local {
        yaml: String,
        source: ConfigSource,
        config: WorkflowConfig,
    },
    Repo,
}

struct PreparedPlan {
    pr_target: Option<crate::pr_review_fix::PrTarget>,
    noninteractive: bool,
    input: String,
    images: Vec<PathBuf>,
    has_issue: bool,
    target: PlanTarget,
}

fn prepare_plan(args: &mut PlanArgs) -> Result<PreparedPlan> {
    let pr_target = if let Some(number) = args.pr {
        let target = crate::pr_review_fix::resolve_pr_target(number, args.repo.as_deref())?;
        args.config
            .get_or_insert_with(|| crate::pr_review_fix::BUILTIN_SELECTOR.to_string());
        args.skip_planning = true;
        args.no_force_exec = true;
        Some(target)
    } else {
        None
    };
    let noninteractive = !std::io::stdin().is_terminal();
    if args.grill && noninteractive {
        return Err(CruiseError::Other(
            "--grill requires an interactive terminal (it interviews you one \
             question at a time); it cannot run in a non-TTY context"
                .to_string(),
        ));
    }

    let (raw_input, from_interactive) = if let Some(pr) = &pr_target {
        let extra = match args.input.take() {
            Some(input) if input != PLAN_STDIN_SENTINEL => input,
            Some(_) if !noninteractive => {
                return Err(CruiseError::Other("stdin requires piped input".into()));
            }
            _ if noninteractive => {
                let mut input = String::new();
                std::io::stdin()
                    .read_to_string(&mut input)
                    .map_err(|error| {
                        CruiseError::Other(format!("failed to read stdin: {error}"))
                    })?;
                input
            }
            _ => String::new(),
        };
        (crate::pr_review_fix::plan_text(pr, &extra), false)
    } else if args.issue.is_some() && args.input.is_none() {
        (String::new(), false)
    } else {
        read_plan_input(args.input.take(), noninteractive)?
    };
    let (input, mut images) = if from_interactive {
        crate::attachments::extract_image_paths(&raw_input)
    } else {
        (raw_input, Vec::new())
    };
    images.extend(args.images.iter().map(PathBuf::from));

    let binding = crate::issue_input::parse_binding(&input)?;
    if args.issue.is_some() && binding.is_some() {
        return Err(CruiseError::Other(
            "pass either #N or --issue, not both".to_string(),
        ));
    }
    let has_issue = args.issue.is_some() || binding.is_some();
    let target = resolve_plan_target(args.repo.as_deref(), args.config.as_deref(), has_issue)?;
    if pr_target.is_some()
        && let PlanTarget::Local { config, .. } = &target
    {
        crate::pr_review_fix::validate_workflow(config)?;
        crate::flags::validate_names(config, &crate::flags::from_cli(&args.flags)?)?;
    }

    Ok(PreparedPlan {
        pr_target,
        noninteractive,
        input,
        images,
        has_issue,
        target,
    })
}

fn resolve_plan_target(
    repo: Option<&str>,
    explicit_config: Option<&str>,
    has_issue: bool,
) -> Result<PlanTarget> {
    match repo.map(str::trim) {
        Some(spec) if !spec.is_empty() => {
            crate::repo_clone::validate_repo_spec(spec)?;
            let ctx = crate::forge::resolve_repo_locator_env(spec)?;
            if has_issue && ctx.kind != crate::forge::ForgeKind::GitHub {
                return Err(CruiseError::Other(
                    "issue input is supported for GitHub repositories only".into(),
                ));
            }
            crate::forge::ensure_forge_available(ctx.kind)?;
            Ok(PlanTarget::Repo)
        }
        _ => {
            let (yaml, source) = crate::resolver::resolve_config(explicit_config)?;
            let config = crate::resolver::load_config_from_source(&yaml, &source)?;
            eprintln!("{}", style(source.display_string()).dim());
            Ok(PlanTarget::Local {
                yaml,
                source,
                config,
            })
        }
    }
}

fn force_exec_opt_out(no_force_exec: bool, grill: bool, has_images: bool) -> Option<&'static str> {
    if no_force_exec {
        Some("--no-force-exec")
    } else if grill {
        Some("--grill")
    } else if has_images {
        Some("image attachment")
    } else {
        None
    }
}

async fn run_force_exec(
    target: PlanTarget,
    input: String,
    rate_limit_retries: usize,
    dry_run: bool,
    background: bool,
    flags: Vec<(String, bool)>,
) -> Result<()> {
    let PlanTarget::Local {
        yaml,
        source,
        config,
    } = target
    else {
        unreachable!();
    };
    let message = if background {
        "force_exec: running in the foreground; no background plan, \
         worktree, or PR will be created"
    } else {
        "force_exec: running the workflow directly in the current directory \
         (no plan, no worktree, no PR)"
    };
    eprintln!("{}", style(message).dim());
    crate::exec_cmd::run_resolved(
        &config,
        &yaml,
        &source,
        crate::exec_cmd::ExecRequest {
            input,
            max_retries: None,
            rate_limit_retries,
            dry_run,
            flags,
        },
    )
    .await
}

async fn run_force_exec_or_continue(
    target: PlanTarget,
    args: &PlanArgs,
    input: &str,
    images: &[PathBuf],
    has_issue: bool,
) -> Result<Option<PlanTarget>> {
    if let PlanTarget::Local { config, .. } = &target
        && config.force_exec()
    {
        if let Some(reason) = force_exec_opt_out(args.no_force_exec, args.grill, !images.is_empty())
            .or(if has_issue { Some("issue input") } else { None })
        {
            eprintln!("{}", style(format!("force_exec ignored: {reason}")).dim());
        } else {
            run_force_exec(
                target,
                input.trim().to_string(),
                args.rate_limit_retries,
                args.dry_run,
                false,
                args.flags.clone(),
            )
            .await?;
            return Ok(None);
        }
    }
    Ok(Some(target))
}

pub async fn run(mut args: PlanArgs) -> Result<()> {
    let PreparedPlan {
        pr_target,
        noninteractive,
        input,
        images,
        has_issue,
        target,
    } = prepare_plan(&mut args)?;
    let Some(target) =
        run_force_exec_or_continue(target, &args, &input, &images, has_issue).await?
    else {
        return Ok(());
    };
    if args.dry_run {
        print_plan_dry_run(&args, &input, &images);
        return Ok(());
    }

    let context = crate::request_context::RequestContext::capture()?;
    let base_dir = context.cwd.clone();
    let task_input = crate::issue_input::resolve_task_input(
        &input,
        args.issue,
        args.repo.as_deref(),
        &base_dir,
        &crate::issue_input::GhGithub,
    )?;
    let request = build_session_request(
        &target,
        args.config.as_deref(),
        task_input,
        &images,
        pr_target.clone(),
        &args.flags,
        args.repo.clone(),
        context.clone(),
    )?;
    let _herdr = crate::herdr::start();
    let client = DaemonClient::connect().await?;
    let mut session = client.create_session(&request).await?;

    if pr_target.is_none() {
        let plan_request = PlanRequest {
            grill: args.grill,
            skip_planning: args.skip_planning,
            no_interactive_planning: args.no_interactive_planning,
            interactive: Interactive::new(!noninteractive || args.skip_planning),
            rate_limit_retries: args.rate_limit_retries,
            context: Some(context.clone()),
            ..PlanRequest::default()
        };
        let stream = client
            .events(EventFilter {
                session: Some(session.id.clone()),
                batch: false,
            })
            .await?;
        client.start_plan(&session.id, &plan_request).await?;
        let mut events = PlanningEvents::new(&client, &session.id);
        let finished = crate::cli_events::drive(&client, &session.id, stream, &mut events).await?;
        if finished.cancelled_by_user {
            client.delete_session(&session.id, &context).await?;
            return Err(CruiseError::Interrupted);
        }
        session = match ensure_plan_succeeded(finished.state) {
            Ok(session) => session,
            Err(error) => {
                crate::status_eprintln!("Plan generation failed: {error}");
                return Err(error);
            }
        };
    }

    if matches!(session.phase, SessionPhase::Planned) {
        eprintln!(
            "\n{} Session {} created (input used as plan).",
            style("v").green().bold(),
            session.id
        );
        eprintln!(
            "  Run with: {}",
            style(format!("cruise run {}", session.id)).cyan()
        );
        return Ok(());
    }
    if !matches!(session.phase, SessionPhase::AwaitingApproval) {
        return Ok(());
    }
    let config = client.session_config(&session.id).await?;
    review_plan(&client, session, &config, &args, &context, noninteractive).await
}

#[expect(
    clippy::too_many_arguments,
    reason = "This request builder maps independent plan CLI inputs into the API DTO."
)]
fn build_session_request(
    target: &PlanTarget,
    explicit_config: Option<&str>,
    task_input: crate::issue_input::ResolvedTaskInput,
    images: &[PathBuf],
    pr_target: Option<crate::pr_review_fix::PrTarget>,
    assignments: &[(String, bool)],
    repo: Option<String>,
    context: crate::request_context::RequestContext,
) -> Result<NewSessionRequest> {
    let (config_path, config_yaml) = match target {
        PlanTarget::Local { yaml, source, .. } => config_request(source, yaml, &context.cwd),
        PlanTarget::Repo => (
            explicit_config.map(|path| request_config_path(path, &context.cwd)),
            None,
        ),
    };
    let issue_images = task_input
        .images
        .into_iter()
        .map(|image| NewSessionImage {
            extension: image.extension.to_string(),
            bytes: image.bytes,
        })
        .collect();
    Ok(NewSessionRequest {
        input: task_input.text,
        base_dir: context.cwd.clone(),
        config_path,
        config_yaml,
        repo,
        pr_number: pr_target.as_ref().map(|target| target.number),
        pr_target,
        issue_number: task_input.issue_number,
        issue_images,
        attachments: images.to_vec(),
        flags: crate::flags::from_cli(assignments)?,
        context: Some(context),
        ..NewSessionRequest::default()
    })
}

fn config_request(
    source: &ConfigSource,
    yaml: &str,
    cwd: &Path,
) -> (Option<PathBuf>, Option<String>) {
    if let Some(path) = source.path() {
        let path = request_config_path(&path.to_string_lossy(), cwd);
        return (Some(path), None);
    }
    match source {
        ConfigSource::Builtin => (
            Some(PathBuf::from(
                crate::new_session_history::BUILTIN_CONFIG_KEY,
            )),
            None,
        ),
        ConfigSource::NamedBuiltin(name) => (
            Some(PathBuf::from(format!(
                "{}{name}",
                crate::resolver::BUILTIN_SELECTOR_PREFIX
            ))),
            None,
        ),
        ConfigSource::Inline => (None, Some(yaml.to_string())),
        ConfigSource::Explicit(_)
        | ConfigSource::EnvVar(_)
        | ConfigSource::Local(_)
        | ConfigSource::UserDir(_) => unreachable!(),
    }
}

fn request_config_path(value: &str, cwd: &Path) -> PathBuf {
    let expanded = crate::new_session_history::expand_tilde(value);
    if expanded == crate::new_session_history::BUILTIN_CONFIG_KEY
        || expanded.starts_with(crate::resolver::BUILTIN_SELECTOR_PREFIX)
    {
        PathBuf::from(expanded)
    } else {
        let path = PathBuf::from(expanded);
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    }
}

fn print_plan_dry_run(args: &PlanArgs, input: &str, images: &[PathBuf]) {
    if let Some(number) = args.issue {
        eprintln!("{}", style(format!("  issue: #{number}")).dim());
    }
    eprintln!(
        "{}",
        style(format!("Would plan: \"{}\"", input.trim())).dim()
    );
    for image in images {
        eprintln!(
            "{}",
            style(format!("  attached image: {}", image.display())).dim()
        );
    }
}

fn read_plan_input(input: Option<String>, noninteractive: bool) -> Result<(String, bool)> {
    let stdin_input = if input.is_none() && noninteractive {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(CruiseError::IoError)?;
        Some(text)
    } else {
        None
    };
    let from_argument_or_stdin = input.is_some() || stdin_input.is_some();
    let text = if let Some(text) = input {
        text
    } else if let Some(text) = stdin_input {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return Err(CruiseError::Other("no input provided on stdin".to_string()));
        }
        trimmed
    } else {
        if noninteractive {
            return Err(CruiseError::Other(
                "no input provided: stdin is not a terminal and no input argument was given"
                    .to_string(),
            ));
        }
        prompt_for_plan_input()?
    };
    Ok((text, !from_argument_or_stdin))
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

fn prompt_for_plan_input() -> Result<String> {
    match prompt_multiline("What would you like to implement?")? {
        InputResult::Submitted(text) => Ok(text),
        InputResult::Cancelled => Err(CruiseError::StepPaused),
    }
}

#[derive(Clone)]
struct FlatNode {
    label: String,
    expanded_step_ids: Vec<String>,
}

fn skip_prompt_nodes(
    config: &WorkflowConfig,
    flags: &crate::flags::FlagValues,
) -> Result<Vec<FlatNode>> {
    let main =
        crate::flags::skip_candidates(&crate::workflow::list_skippable_steps(config)?, flags);
    let after_pr = crate::flags::skip_candidates(
        &crate::workflow::list_skippable_after_pr_steps(config)?,
        flags,
    );
    let mut flat = flatten_nodes(&main, "");
    flat.extend(flatten_nodes(&after_pr, "[after-pr] "));
    Ok(flat)
}

impl fmt::Display for FlatNode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label)
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
            format!("{prefix}{}", node.id)
        } else {
            node.id.rsplit('/').next().unwrap_or(&node.id).to_string()
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
    for id in selected_nodes
        .into_iter()
        .flat_map(|node| node.expanded_step_ids)
    {
        if seen.insert(id.clone()) {
            expanded_ids.push(id);
        }
    }
    expanded_ids
}

fn flat_node_default_indices(flat: &[FlatNode], previously_skipped: &[String]) -> Vec<usize> {
    let skipped: HashSet<&str> = previously_skipped.iter().map(String::as_str).collect();
    flat.iter()
        .enumerate()
        .filter(|(_, node)| {
            !node.expanded_step_ids.is_empty()
                && node
                    .expanded_step_ids
                    .iter()
                    .all(|id| skipped.contains(id.as_str()))
        })
        .map(|(index, _)| index)
        .collect()
}

pub(crate) fn select_steps_to_skip(
    config: &WorkflowConfig,
    previously_skipped: &[String],
    flags: &crate::flags::FlagValues,
) -> Result<Option<Vec<String>>> {
    let flat = skip_prompt_nodes(config, flags)?;
    if flat.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let defaults = flat_node_default_indices(&flat, previously_skipped);
    crate::platform::reclaim_terminal_foreground();
    match inquire::MultiSelect::new("Steps to skip (Space to toggle, Enter to confirm):", flat)
        .with_help_message("No selection = no additional steps skipped")
        .with_default(&defaults)
        .prompt()
    {
        Ok(selected) => Ok(Some(collect_expanded_ids(selected))),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(error) => Err(CruiseError::Other(format!("selection error: {error}"))),
    }
}

fn select_skipped_steps_with_history(
    session: &SessionState,
    config: &WorkflowConfig,
    flags: &crate::flags::FlagValues,
) -> Result<Option<Vec<String>>> {
    let key = session
        .config
        .stable_identity(session.repo.as_deref(), &session.id);
    let history = NewSessionHistory::load_best_effort();
    let base_dir = session.base_dir.to_string_lossy();
    let scope = session.repo.as_deref().map_or(
        HistoryScope::Directory(base_dir.as_ref()),
        HistoryScope::Repo,
    );
    let previously_skipped = history
        .latest_entry_for_scope(scope, &key)
        .map_or_else(Vec::new, |entry| entry.skipped_steps.clone());
    select_steps_to_skip(config, &previously_skipped, flags)
}

#[expect(
    clippy::too_many_lines,
    reason = "The review loop owns the complete interactive plan action flow."
)]
async fn review_plan(
    client: &DaemonClient,
    mut session: SessionState,
    config: &WorkflowConfig,
    args: &PlanArgs,
    context: &crate::request_context::RequestContext,
    noninteractive: bool,
) -> Result<()> {
    loop {
        let plan: String = client
            .get_json(&format!("sessions/{}/plan", session.id))
            .await?;
        crate::display::print_bordered(&plan, Some("plan.md"));
        if noninteractive {
            approve_with_interrupt(
                client,
                &session.id,
                &ApproveRequest {
                    generate_title: true,
                    context: Some(context.clone()),
                    ..ApproveRequest::default()
                },
                context,
            )
            .await?;
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

        let blocked = crate::herdr::blocked("Plan ready: approve, fix, or ask");
        let mut options = vec!["Approve", "Fix", "Ask", "Verify", "Execute now"];
        if session.pr_target.is_none() {
            options.push("Publish as Issue");
        }
        crate::platform::reclaim_terminal_foreground();
        let selected = match inquire::Select::new("Action:", options).prompt() {
            Ok(value) => value,
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                client.discard_plan(&session.id).await?;
                eprintln!("\nCancelled. Session {} discarded.", session.id);
                return Ok(());
            }
            Err(error) => return Err(CruiseError::Other(format!("selection error: {error}"))),
        };
        drop(blocked);

        match selected {
            "Approve" | "Execute now" => {
                let execute_now = selected == "Execute now";
                let flags = if args.flags.is_empty() {
                    let _blocked = crate::herdr::blocked("Select workflow flags");
                    let Some(values) = crate::run_cmd::select_flag_values(config, &session.flags)?
                    else {
                        continue;
                    };
                    values
                } else {
                    session.flags.clone()
                };
                let skipped_steps = {
                    let _blocked = crate::herdr::blocked("Select steps to skip");
                    let Some(steps) = select_skipped_steps_with_history(&session, config, &flags)?
                    else {
                        continue;
                    };
                    steps
                };
                let run_flags = if args.flags.is_empty() {
                    config
                        .flags
                        .keys()
                        .map(|name| (name.clone(), flags.get(name).copied().unwrap_or(false)))
                        .collect()
                } else {
                    args.flags.clone()
                };
                session = approve_with_interrupt(
                    client,
                    &session.id,
                    &ApproveRequest {
                        flags: Some(flags),
                        skipped_steps: Some(skipped_steps),
                        generate_title: true,
                        context: Some(context.clone()),
                    },
                    context,
                )
                .await?;
                if !execute_now {
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
                eprintln!(
                    "\n{} Executing session {}...",
                    style("->").cyan(),
                    session.id
                );
                return crate::run_cmd::run(crate::cli::RunArgs {
                    session: Some(session.id),
                    all: false,
                    parallelism: None,
                    auto_requeue_max_attempts: None,
                    max_retries: None,
                    rate_limit_retries: args.rate_limit_retries,
                    dry_run: false,
                    cleanup_after_pr: false,
                    no_cleanup_after_pr: false,
                    flags: run_flags,
                })
                .await;
            }
            "Fix" => {
                let Some(feedback) = prompt_action_text("Describe the changes needed:")? else {
                    continue;
                };
                let request = PlanRequest {
                    feedback: Some(feedback),
                    interactive: Interactive::new(true),
                    rate_limit_retries: args.rate_limit_retries,
                    context: Some(context.clone()),
                    ..PlanRequest::default()
                };
                session =
                    drive_plan_operation(client, &session.id, PlanOperation::Fix(request)).await?;
            }
            "Ask" => {
                let Some(question) = prompt_action_text("Your question:")? else {
                    continue;
                };
                let request = PlanRequest {
                    question: Some(question),
                    interactive: Interactive::new(true),
                    rate_limit_retries: args.rate_limit_retries,
                    context: Some(context.clone()),
                    ..PlanRequest::default()
                };
                session =
                    drive_plan_operation(client, &session.id, PlanOperation::Ask(request)).await?;
            }
            "Verify" => {
                let request = PlanRequest {
                    interactive: Interactive::new(true),
                    rate_limit_retries: args.rate_limit_retries,
                    context: Some(context.clone()),
                    ..PlanRequest::default()
                };
                session = drive_plan_operation(client, &session.id, PlanOperation::Verify(request))
                    .await?;
            }
            "Publish as Issue" => {
                let trigger_cruise = if crate::issue_publish::supports_trigger_cruise(&session) {
                    let _blocked = crate::herdr::blocked("Publish plan as issue");
                    let Some(answer) = prompt_trigger_cruise()? else {
                        continue;
                    };
                    answer
                } else {
                    false
                };
                match client.publish(&session.id, trigger_cruise).await {
                    Ok(url) => {
                        eprintln!(
                            "\n{} Published plan as issue: {url}",
                            style("v").green().bold()
                        );
                        return Ok(());
                    }
                    Err(error) => eprintln!(
                        "\n{} Failed to publish plan as issue: {error}",
                        style("x").red().bold()
                    ),
                }
            }
            _ => {}
        }
    }
}

fn prompt_action_text(prompt: &str) -> Result<Option<String>> {
    let _blocked = crate::herdr::blocked(prompt);
    match prompt_multiline(prompt)? {
        InputResult::Submitted(text) => Ok(Some(text)),
        InputResult::Cancelled => Ok(None),
    }
}

fn prompt_trigger_cruise() -> Result<Option<bool>> {
    crate::platform::reclaim_terminal_foreground();
    match inquire::Confirm::new("Post an @cruise run comment after creating the issue?")
        .with_default(false)
        .prompt()
    {
        Ok(answer) => Ok(Some(answer)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(error) => Err(CruiseError::Other(format!("selection error: {error}"))),
    }
}

async fn approve_with_interrupt(
    client: &DaemonClient,
    session_id: &str,
    request: &ApproveRequest,
    context: &crate::request_context::RequestContext,
) -> Result<SessionState> {
    let approval = client.approve(session_id, request);
    tokio::pin!(approval);
    tokio::select! {
        result = &mut approval => result,
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|error| {
                CruiseError::Other(format!("failed to listen for Ctrl+C: {error}"))
            })?;
            eprintln!("Cancelling session {session_id}...");
            let cancel_result = client.cancel(session_id).await;
            let _ = approval.await;
            cancel_result?;
            client.delete_session(session_id, context).await?;
            Err(CruiseError::Interrupted)
        }
    }
}

enum PlanOperation {
    Generate(PlanRequest),
    Fix(PlanRequest),
    Ask(PlanRequest),
    Verify(PlanRequest),
    Replan(PlanRequest),
}

struct PlanningEvents<'a> {
    terminal: crate::cli_events::TerminalEvents<'a>,
    session_id: String,
    spinner: Option<crate::spinner::Spinner>,
}

impl<'a> PlanningEvents<'a> {
    fn new(client: &'a DaemonClient, session_id: &str) -> Self {
        Self {
            terminal: crate::cli_events::TerminalEvents::new(client, session_id)
                .with_plan_chunks(true),
            session_id: session_id.to_owned(),
            spinner: None,
        }
    }

    fn owns(&self, session_id: &str) -> bool {
        session_id == self.session_id
    }

    fn dispatch(&mut self, event: &crate::application::ApplicationEvent) -> Result<()> {
        if let Some(spinner) = &self.spinner {
            let mut result = Ok(());
            spinner.suspend(|| result = self.terminal.on_event(event));
            result
        } else {
            self.terminal.on_event(event)
        }
    }
}

impl crate::daemon::client::SessionEventHandler for PlanningEvents<'_> {
    fn on_event(&mut self, event: &crate::application::ApplicationEvent) -> Result<()> {
        use crate::application::ApplicationEvent;

        match event {
            ApplicationEvent::PlanStarted { session_id, .. } if self.owns(session_id) => {
                if std::io::stderr().is_terminal() {
                    self.spinner = Some(crate::spinner::Spinner::start("Planning..."));
                } else {
                    crate::status_eprintln!("[plan] creating plan...");
                }
                Ok(())
            }
            ApplicationEvent::PlanFinished { session_id, .. }
            | ApplicationEvent::PlanFailed { session_id, .. }
            | ApplicationEvent::PlanCancelled { session_id }
                if self.owns(session_id) =>
            {
                drop(self.spinner.take());
                self.terminal.on_event(event)
            }
            _ => self.dispatch(event),
        }
    }

    fn on_pending_prompts(&mut self, prompts: &[crate::application::PendingPrompt]) -> Result<()> {
        if let Some(spinner) = &self.spinner {
            let mut result = Ok(());
            spinner.suspend(|| result = self.terminal.on_pending_prompts(prompts));
            result
        } else {
            self.terminal.on_pending_prompts(prompts)
        }
    }
}

async fn drive_plan_operation(
    client: &DaemonClient,
    session_id: &str,
    operation: PlanOperation,
) -> Result<SessionState> {
    let stream = client
        .events(EventFilter {
            session: Some(session_id.to_string()),
            batch: false,
        })
        .await?;
    match operation {
        PlanOperation::Generate(request) => client.start_plan(session_id, &request).await?,
        PlanOperation::Fix(request) => client.fix_plan(session_id, &request).await?,
        PlanOperation::Ask(request) => client.ask_plan(session_id, &request).await?,
        PlanOperation::Verify(request) => client.verify_plan(session_id, &request).await?,
        PlanOperation::Replan(request) => client.replan_plan(session_id, &request).await?,
    }
    let mut events = PlanningEvents::new(client, session_id);
    let finished = crate::cli_events::drive(client, session_id, stream, &mut events).await?;
    if finished.cancelled_by_user {
        return Err(CruiseError::Interrupted);
    }
    ensure_plan_succeeded(finished.state)
}

fn ensure_plan_succeeded(state: SessionState) -> Result<SessionState> {
    if let Some(message) = &state.plan_error {
        return Err(CruiseError::Other(message.clone()));
    }
    if let SessionPhase::Failed(message) = &state.phase {
        return Err(CruiseError::Other(message.clone()));
    }
    Ok(state)
}

pub async fn launch_background_plan(
    plan_input: &str,
    skip_planning: bool,
    repo: Option<&str>,
    images: &[String],
    no_force_exec: bool,
    flags: &[(String, bool)],
) -> Result<()> {
    let input = read_background_plan_input(plan_input)?;
    let binding = crate::issue_input::parse_binding(&input)?;
    let target = resolve_plan_target(repo, None, binding.is_some())?;
    let detected_images = images.iter().map(PathBuf::from).collect::<Vec<_>>();

    if let PlanTarget::Local { config, .. } = &target
        && config.force_exec()
    {
        match force_exec_opt_out(no_force_exec, false, !detected_images.is_empty())
            .or(binding.as_ref().map(|_| "issue input"))
        {
            Some(reason) => {
                eprintln!("{}", style(format!("force_exec ignored: {reason}")).dim());
            }
            None => {
                return run_force_exec(
                    target,
                    input.trim().to_string(),
                    DEFAULT_RATE_LIMIT_RETRIES,
                    false,
                    true,
                    flags.to_vec(),
                )
                .await;
            }
        }
    }

    let context = crate::request_context::RequestContext::capture()?;
    let task_input = crate::issue_input::resolve_task_input(
        &input,
        None,
        repo,
        &context.cwd,
        &crate::issue_input::GhGithub,
    )?;
    let request = build_session_request(
        &target,
        None,
        task_input,
        &detected_images,
        None,
        flags,
        repo.map(ToString::to_string),
        context.clone(),
    )?;
    let client = DaemonClient::connect().await?;
    let session = client.create_session(&request).await?;
    let plan_request = PlanRequest {
        skip_planning,
        interactive: Interactive::new(false),
        rate_limit_retries: DEFAULT_RATE_LIMIT_RETRIES,
        context: Some(context),
        ..PlanRequest::default()
    };
    let stream = client
        .events(EventFilter {
            session: Some(session.id.clone()),
            batch: false,
        })
        .await?;
    client.start_plan(&session.id, &plan_request).await?;

    if skip_planning {
        // Skip-planning does no model work; wait for it so the session is
        // already `Planned` when this command returns (scripts run it next).
        let state = client
            .wait_for_terminal_from(
                &session.id,
                stream,
                &mut |_: &crate::application::ApplicationEvent| Ok(()),
            )
            .await?;
        ensure_plan_succeeded(state)?;
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

pub async fn regenerate_plan_for_session(
    client: &DaemonClient,
    session_id: &str,
    rate_limit_retries: usize,
) -> Result<()> {
    let session: SessionState = client.get_json(&format!("sessions/{session_id}")).await?;
    let context = crate::request_context::RequestContext::capture()?;
    let request = PlanRequest {
        interactive: Interactive::new(true),
        rate_limit_retries,
        context: Some(context),
        ..PlanRequest::default()
    };
    let operation = if matches!(
        session.phase,
        SessionPhase::Planned | SessionPhase::AwaitingApproval
    ) {
        PlanOperation::Replan(request)
    } else {
        PlanOperation::Generate(request)
    };
    drive_plan_operation(client, session_id, operation).await?;
    Ok(())
}

pub async fn replan_session(
    client: &DaemonClient,
    session_id: &str,
    feedback: String,
    rate_limit_retries: usize,
) -> Result<()> {
    let request = PlanRequest {
        feedback: Some(feedback),
        interactive: Interactive::new(true),
        rate_limit_retries,
        context: Some(crate::request_context::RequestContext::capture()?),
        ..PlanRequest::default()
    };
    drive_plan_operation(client, session_id, PlanOperation::Fix(request)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{FlatNode, flat_node_default_indices};

    #[test]
    fn skipped_step_defaults_require_every_expanded_leaf() {
        let nodes = vec![
            FlatNode {
                label: "single".to_string(),
                expanded_step_ids: vec!["single".to_string()],
            },
            FlatNode {
                label: "parent".to_string(),
                expanded_step_ids: vec!["parent/a".to_string(), "parent/b".to_string()],
            },
        ];
        assert_eq!(
            flat_node_default_indices(&nodes, &["single".to_string()]),
            vec![0]
        );
        assert_eq!(
            flat_node_default_indices(&nodes, &["parent/a".to_string(), "parent/b".to_string()]),
            vec![1]
        );
    }
}
