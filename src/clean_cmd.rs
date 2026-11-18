use console::style;

use crate::cli::CleanArgs;
use crate::error::Result;

pub async fn run(_args: CleanArgs) -> Result<()> {
    let client = crate::daemon::client::DaemonClient::connect().await?;
    let context = crate::request_context::RequestContext::capture()?;
    let report = client.clean(&context).await?;
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
