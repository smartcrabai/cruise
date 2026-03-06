mod clean_cmd;
mod cli;
mod condition;
mod config;
mod display;
mod engine;
mod error;
mod file_tracker;
mod list_cmd;
mod plan_cmd;
mod resolver;
mod run_cmd;
mod session;
mod spinner;
mod step;
mod variable;
mod worktree;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

async fn run() -> error::Result<()> {
    let cli = cli::parse_cli();
    match cli.command {
        Some(cli::Commands::Plan(args)) => plan_cmd::run(args).await,
        Some(cli::Commands::Run(args)) => run_cmd::run(args).await,
        Some(cli::Commands::List) => list_cmd::run().await,
        Some(cli::Commands::Clean(args)) => clean_cmd::run(args),
        None => {
            // Backward compat: no subcommand → treat as `plan`.
            let plan_args = cli::PlanArgs {
                input: cli.input,
                config: None,
                dry_run: false,
                rate_limit_retries: 5,
            };
            plan_cmd::run(plan_args).await
        }
    }
}
