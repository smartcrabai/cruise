mod app_config;
pub mod application;
mod artifacts;
mod ask_handler;
mod attachments;
mod backend;
mod batch_dashboard;
#[cfg_attr(not(test), expect(dead_code))]
mod batch_run;
mod builtin_workflows;
mod cancellation;
mod clean_cmd;
mod cli;
mod commit_guard;
mod condition;
mod config;
mod config_cmd;
mod configs;
mod console_mode;
#[expect(
    unused_imports,
    reason = "library compatibility exports are unused in the CLI module copy"
)]
mod dag;
mod desktop_notifications;
mod display;
mod draft_cmd;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the CLI module copy includes a public library compatibility API"
    )
)]
mod engine;
mod error;
mod exec_cmd;
mod executor;
mod file_tracker;
mod forge;
mod graph;
mod herdr;
mod issue_publish;
mod list_cmd;
mod metadata;
mod multiline_input;
pub mod new_session_draft;
mod new_session_history;
mod option_handler;
mod paths;
mod plan_cmd;
mod planning;
mod platform;
mod pr_merge;
mod repo_clone;
mod resolver;
mod retry;
mod run_cmd;
mod run_observer;
mod sdk_tools;
mod session;
mod session_config;
mod session_edit;
mod spinner;
mod ssh_cmd;
mod step;
#[cfg(test)]
mod test_binary_support;
#[cfg(test)]
mod test_support;
mod timeout;
mod tui;
mod variable;
mod webui;
mod workflow;
#[cfg_attr(not(test), expect(dead_code))]
mod workflow_call;
mod workflow_cmd;
mod workflow_generate;
mod workflow_packages;
mod workspace;
mod worktree;
mod worktree_pr;
mod yaml_metadata;

// Multi-threaded runtime: parallel `run --all` workers execute blocking
// terminal prompts inline in their tasks; on a single-threaded runtime any
// open menu would freeze every other concurrently running session.
#[tokio::main]
async fn main() {
    if let Err(e) = Box::pin(run()).await {
        eprintln!("Error: {}", e.detailed_message());
        std::process::exit(1);
    }
}

async fn run() -> error::Result<()> {
    let cli::Cli {
        plan,
        command,
        input,
        no_force_exec,
        skip_planning,
        repo,
        images,
    } = cli::parse_cli();
    match command {
        Some(cli::Commands::PlanWorker(args)) => plan_cmd::run_plan_worker(args).await,
        Some(cli::Commands::Plan(args)) => Box::pin(plan_cmd::run(args)).await,
        Some(cli::Commands::Draft(args)) => draft_cmd::run(args),
        Some(cli::Commands::Run(args)) => run_cmd::run(args).await,
        Some(cli::Commands::List(args)) => list_cmd::run(args).await,
        Some(cli::Commands::Clean(args)) => clean_cmd::run(args),
        Some(cli::Commands::Config(args)) => config_cmd::run(&args),
        Some(cli::Commands::Exec(args)) => exec_cmd::run(args).await,
        Some(cli::Commands::Ssh(args)) => ssh_cmd::run(&args),
        Some(cli::Commands::Webui(args)) => webui::run(args).await,
        Some(cli::Commands::Workflow(cli::WorkflowCommand::List)) => workflow_cmd::list(),
        Some(cli::Commands::Workflow(cli::WorkflowCommand::Eject(args))) => {
            workflow_cmd::eject(&args.name, args.to == cli::EjectDestination::Project)
        }
        Some(cli::Commands::Workflow(cli::WorkflowCommand::Generate(args))) => {
            workflow_generate::run(args).await
        }
        Some(cli::Commands::Workflow(cli::WorkflowCommand::Add(args))) => {
            workflow_cmd::add(&args.spec, args.name, args.yes)
        }
        Some(cli::Commands::Workflow(cli::WorkflowCommand::Update(args))) => {
            workflow_cmd::update(&args.name, args.yes)
        }
        Some(cli::Commands::Workflow(cli::WorkflowCommand::Remove(args))) => {
            workflow_cmd::remove(&args.name)
        }
        None if plan.is_some() => {
            Box::pin(plan_cmd::launch_background_plan(
                &plan.unwrap_or_default(),
                skip_planning,
                repo.as_deref(),
                &images,
                no_force_exec,
            ))
            .await
        }
        None if input.is_none()
            && !skip_planning
            && !no_force_exec
            && repo.is_none()
            && images.is_empty() =>
        {
            tui::run().await
        }
        None => {
            // Backward compat: positional input and planning options use `plan`.
            let plan_args = cli::PlanArgs {
                input,
                config: None,
                dry_run: false,
                no_force_exec,
                skip_planning,
                formal_spec: false,
                grill: false,
                no_interactive_planning: false,
                repo,
                rate_limit_retries: cli::DEFAULT_RATE_LIMIT_RETRIES,
                images,
            };
            Box::pin(plan_cmd::run(plan_args)).await
        }
    }
}
