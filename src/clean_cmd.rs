use console::style;

use crate::cli::CleanArgs;
use crate::daemon::client::DaemonClient;
use crate::error::Result;
use crate::request_context::RequestContext;

pub async fn run(_args: CleanArgs) -> Result<()> {
    let client = DaemonClient::connect().await?;
    let context = RequestContext::capture()?;
    clean_and_report(&client, &context).await
}

pub(crate) async fn clean_and_report(
    client: &DaemonClient,
    context: &RequestContext,
) -> Result<()> {
    let report = client.clean(context).await?;
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
