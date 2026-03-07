use console::style;

use crate::cli::CleanArgs;
use crate::error::Result;
use crate::session::SessionManager;

pub fn run(_args: CleanArgs) -> Result<()> {
    let manager = SessionManager::new(crate::session::get_cruise_home()?);

    let report = manager.cleanup_by_pr_status()?;

    if report.deleted == 0 {
        eprintln!("No sessions to clean up.");
    } else {
        eprintln!(
            "{} Removed {} session(s) with closed/merged PRs.",
            style("✓").green().bold(),
            report.deleted,
        );
    }
    if report.skipped > 0 {
        eprintln!(
            "  {} session(s) skipped (PR still open or check failed).",
            report.skipped
        );
    }

    Ok(())
}
