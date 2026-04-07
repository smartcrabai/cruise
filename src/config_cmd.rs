//! `cruise config` — show or update application-level configuration.

use crate::{cli::ConfigArgs, error::Result};

/// Handle the `cruise config` subcommand.
///
/// - No flags: print the current configuration to stdout.
/// - `--set-parallelism N`: validate and persist the new value, then print it.
///
/// # Errors
///
/// Returns an error if the config cannot be read or written, or if `N` is 0.
pub fn run(args: &ConfigArgs) -> Result<()> {
    if let Some(parallelism) = args.set_parallelism {
        let config = crate::app_config::AppConfig {
            run_all_parallelism: parallelism,
        };
        config.save()?;
        println!("run_all_parallelism = {parallelism}");
    } else {
        let config = crate::app_config::AppConfig::load()?;
        println!("run_all_parallelism = {}", config.run_all_parallelism);
    }
    Ok(())
}
