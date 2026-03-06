use console::style;

use crate::cli::CleanArgs;
use crate::error::Result;
use crate::session::SessionManager;

pub fn run(args: CleanArgs) -> Result<()> {
    let manager = SessionManager::new(crate::session::get_cruise_home()?);

    let report = manager.cleanup_old(args.days)?;

    if report.deleted == 0 {
        eprintln!("No sessions to clean up.");
    } else {
        eprintln!(
            "{} Removed {} session(s) older than {} day(s).",
            style("✓").green().bold(),
            report.deleted,
            args.days
        );
    }

    Ok(())
}
