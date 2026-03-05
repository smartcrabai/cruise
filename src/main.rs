mod cli;
mod condition;
mod config;
mod engine;
mod error;
mod file_tracker;
mod resolver;
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
    let args = cli::parse_args();
    engine::run(args).await
}
