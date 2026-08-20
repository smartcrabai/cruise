use console::style;

use crate::cli::CleanArgs;
use crate::error::Result;
use crate::session::SessionManager;

pub fn run(_args: CleanArgs) -> Result<()> {
    let manager = SessionManager::new(crate::paths::data_dir()?);
    let report = manager.cleanup_by_pr_status()?;
    let pr_deleted = report.deleted.saturating_sub(report.no_pr_deleted);
    if pr_deleted > 0 {
        eprintln!(
            "{} Removed {} session(s) with closed/merged PRs.",
            style("v").green().bold(),
            pr_deleted,
        );
    }
    if report.no_pr_deleted > 0 {
        eprintln!(
            "{} Removed {} terminal no-PR session(s).",
            style("v").green().bold(),
            report.no_pr_deleted,
        );
    }
    if report.deleted == 0 {
        eprintln!("No sessions to clean up.");
    }
    if report.skipped > 0 {
        eprintln!(
            "  {} session(s) skipped (PR still open or check failed).",
            report.skipped
        );
    }
    Ok(())
}
