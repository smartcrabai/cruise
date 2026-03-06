use console::style;
use inquire::InquireError;

use crate::error::{CruiseError, Result};
use crate::session::{SessionManager, SessionPhase, SessionState, get_cruise_home};

pub async fn run() -> Result<()> {
    let manager = SessionManager::new(get_cruise_home()?);

    loop {
        let sessions = manager.list()?;

        if sessions.is_empty() {
            eprintln!("No sessions found.");
            return Ok(());
        }

        // Build display labels with color-coded phase.
        let labels: Vec<String> = sessions.iter().map(format_session_label).collect();
        let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();

        let selected = match inquire::Select::new("Select a session:", label_refs).prompt() {
            Ok(s) => s,
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
                return Ok(());
            }
            Err(e) => return Err(CruiseError::Other(format!("selection error: {e}"))),
        };

        let idx = labels.iter().position(|l| l.as_str() == selected).unwrap();
        let session = &sessions[idx];

        // Show plan.md content.
        let plan_path = session.plan_path(&manager.sessions_dir());
        if let Ok(content) = std::fs::read_to_string(&plan_path) {
            crate::display::print_bordered(&content, Some("plan.md"));
        }

        // Action menu.
        let can_run = matches!(
            session.phase,
            SessionPhase::Planned | SessionPhase::Failed(_)
        );

        let mut actions = vec![];
        if can_run {
            actions.push("Run");
        }
        actions.push("Delete");
        actions.push("Back");

        let action = match inquire::Select::new("Action:", actions).prompt() {
            Ok(a) => a,
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => "Back",
            Err(e) => return Err(CruiseError::Other(format!("selection error: {e}"))),
        };

        match action {
            "Run" => {
                let run_args = crate::cli::RunArgs {
                    session: Some(session.id.clone()),
                    max_retries: 10,
                    rate_limit_retries: 5,
                    keep_worktree: false,
                    dry_run: false,
                };
                return crate::run_cmd::run(run_args).await;
            }
            "Delete" => {
                manager.delete(&session.id)?;
                eprintln!("{} Session {} deleted.", style("✓").green(), session.id);
                // Loop back to show updated list.
            }
            _ => {
                // "Back" — loop to show session list again.
            }
        }
    }
}

fn format_session_label(s: &SessionState) -> String {
    let phase_str = match &s.phase {
        SessionPhase::Planned => style("Planned").cyan().to_string(),
        SessionPhase::Running => style("Running").yellow().to_string(),
        SessionPhase::Completed => style("Completed").green().to_string(),
        SessionPhase::Failed(_) => style("Failed").red().to_string(),
    };
    let input_preview = crate::display::truncate(&s.input, 60);
    format!("{} | {} | {}", s.id, phase_str, input_preview)
}
