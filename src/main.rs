mod app_config;
mod cancellation;
mod clean_cmd;
mod cli;
mod condition;
mod config;
mod config_cmd;
mod display;
mod engine;
mod error;
mod file_tracker;
mod list_cmd;
mod llm_api;
mod metadata;
mod multiline_input;
mod new_session_history;
mod option_handler;
mod plan_cmd;
mod planning;
mod platform;
mod resolver;
mod run_cmd;
mod session;
mod spinner;
mod step;
#[cfg(test)]
mod test_binary_support;
#[cfg(test)]
mod test_support;
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
    } = cli::parse_cli();
    match command {
        Some(cli::Commands::PlanWorker(args)) => plan_cmd::run_plan_worker(args).await,
        Some(cli::Commands::Plan(args)) => plan_cmd::run(args).await,
        Some(cli::Commands::Run(args)) => run_cmd::run(args).await,
        Some(cli::Commands::List(args)) => list_cmd::run(args).await,
        Some(cli::Commands::Clean(args)) => clean_cmd::run(args),
        Some(cli::Commands::Config(args)) => config_cmd::run(&args),
        None if plan.is_some() => plan_cmd::launch_background_plan(&plan.unwrap_or_default()),
        None => {
            // Backward compat: no subcommand -> treat as `plan`.
            let plan_args = cli::PlanArgs {
                input,
                config: None,
                dry_run: false,
                rate_limit_retries: cli::DEFAULT_RATE_LIMIT_RETRIES,
            };
            plan_cmd::run(plan_args).await
        }
    }
}
