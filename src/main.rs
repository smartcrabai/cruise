mod app_config;
mod ask_handler;
mod cancellation;
mod clean_cmd;
mod cli;
mod condition;
mod config;
mod config_cmd;
mod display;
mod draft_cmd;
mod engine;
mod error;
mod exec_cmd;
mod executor;
mod file_tracker;
mod list_cmd;
mod llm_api;
mod metadata;
mod multiline_input;
mod new_session_history;
mod option_handler;
mod paths;
mod plan_cmd;
mod planning;
mod platform;
mod repo_clone;
mod resolver;
mod run_cmd;
mod sdk_tools;
mod session;
mod spinner;
mod step;
#[cfg(test)]
mod test_binary_support;
#[cfg(test)]
mod test_support;
mod timeout;
mod variable;
mod workflow;
mod workspace;
mod worktree;
mod worktree_pr;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("Error: {}", e.detailed_message());
        std::process::exit(1);
    }
}

async fn run() -> error::Result<()> {
    let cli::Cli {
        plan,
        command,
        input,
        skip_planning,
        repo,
    } = cli::parse_cli();
    match command {
        Some(cli::Commands::PlanWorker(args)) => plan_cmd::run_plan_worker(args).await,
        Some(cli::Commands::Plan(args)) => plan_cmd::run(args).await,
        Some(cli::Commands::Draft(args)) => draft_cmd::run(args),
        Some(cli::Commands::Run(args)) => run_cmd::run(args).await,
        Some(cli::Commands::List(args)) => list_cmd::run(args).await,
        Some(cli::Commands::Clean(args)) => clean_cmd::run(args),
        Some(cli::Commands::Config(args)) => config_cmd::run(&args),
        Some(cli::Commands::Exec(args)) => exec_cmd::run(args).await,
        None if plan.is_some() => plan_cmd::launch_background_plan(
            &plan.unwrap_or_default(),
            skip_planning,
            repo.as_deref(),
        ),
        None => {
            // Backward compat: no subcommand -> treat as `plan`.
            let plan_args = cli::PlanArgs {
                input,
                config: None,
                dry_run: false,
                skip_planning,
                grill: false,
                repo,
                rate_limit_retries: cli::DEFAULT_RATE_LIMIT_RETRIES,
            };
            plan_cmd::run(plan_args).await
        }
    }
}
