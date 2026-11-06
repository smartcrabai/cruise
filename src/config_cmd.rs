//! `cruise config` -- show or update application-level configuration.

use crate::{cli::ConfigArgs, error::Result};

/// Handle the `cruise config` subcommand.
///
/// - No flags: print the current configuration to stdout.
/// - `--set-parallelism N`: validate and persist the shared `WebUI`/TUI concurrency limit, then print it.
///
/// # Errors
///
/// Returns an error if the config cannot be read or written, or if parallelism is 0.
pub fn run(args: &ConfigArgs) -> Result<()> {
    if args.set_parallelism.is_some() || args.set_auto_requeue_max_attempts.is_some() {
        let mut config = crate::app_config::AppConfig::load()?;
        if let Some(parallelism) = args.set_parallelism {
            config.run_all_parallelism = parallelism;
        }
        if let Some(max) = args.set_auto_requeue_max_attempts {
            config.run_all_auto_requeue_max_attempts = max;
        }
        config.save()?;
        if let Some(parallelism) = args.set_parallelism {
            println!("run_all_parallelism = {parallelism}");
        }
        if let Some(max) = args.set_auto_requeue_max_attempts {
            println!("run_all_auto_requeue_max_attempts = {max}");
        }
    } else {
        let config = crate::app_config::AppConfig::load()?;
        println!("run_all_parallelism = {}", config.run_all_parallelism);
        println!(
            "run_all_auto_requeue_max_attempts = {}",
            config.run_all_auto_requeue_max_attempts
        );
        if let Some(model) = config.model.as_ref() {
            println!("model = {}", serde_json::to_string(model)?);
        }
        if let Some(model) = config.plan_model.as_ref() {
            println!("plan_model = {}", serde_json::to_string(model)?);
        }
    }
    Ok(())
}
