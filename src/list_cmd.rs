use std::io::Write;

use console::style;
use inquire::InquireError;
use serde::Serialize;

use crate::cli::{DEFAULT_MAX_RETRIES, DEFAULT_RATE_LIMIT_RETRIES, ListArgs};
use crate::error::{CruiseError, Result};
use crate::multiline_input::{InputResult, prompt_multiline};
use crate::session::{SessionManager, SessionPhase, SessionState, WorkspaceMode, get_cruise_home};

/// CLI-only DTO for JSON output. Stable machine-readable form of `SessionState`.
/// `phase` is always a plain string; `phase_error` carries the failure message for Failed sessions.
#[derive(Debug, Serialize)]
struct ListSessionJson {
    id: String,
    base_dir: String,
    phase: &'static str,
    phase_error: Option<String>,
    config_source: String,
    input: String,
    title: Option<String>,
    current_step: Option<String>,
    created_at: String,
    completed_at: Option<String>,
    worktree_path: Option<String>,
    worktree_branch: Option<String>,
    workspace_mode: WorkspaceMode,
    target_branch: Option<String>,
    pr_url: Option<String>,
    config_path: Option<String>,
    updated_at: Option<String>,
    awaiting_input: bool,
}

/// `Failed(msg)` is normalized to `phase = "Failed"` + `phase_error = Some(msg)`.
fn session_to_json(session: SessionState) -> ListSessionJson {
    let (phase, phase_error): (&'static str, Option<String>) = match session.phase {
        SessionPhase::AwaitingApproval => ("AwaitingApproval", None),
        SessionPhase::Planned => ("Planned", None),
        SessionPhase::Running => ("Running", None),
        SessionPhase::Completed => ("Completed", None),
        SessionPhase::Failed(msg) => ("Failed", Some(msg)),
        SessionPhase::Suspended => ("Suspended", None),
    };
    ListSessionJson {
        id: session.id,
        base_dir: session.base_dir.to_string_lossy().into_owned(),
        phase,
        phase_error,
        config_source: session.config_source,
        input: session.input,
        title: session.title,
        current_step: session.current_step,
        created_at: session.created_at,
        completed_at: session.completed_at,
        worktree_path: session
            .worktree_path
            .map(|p| p.to_string_lossy().into_owned()),
        worktree_branch: session.worktree_branch,
        workspace_mode: session.workspace_mode,
        target_branch: session.target_branch,
        pr_url: session.pr_url,
        config_path: session
            .config_path
            .map(|p| p.to_string_lossy().into_owned()),
        updated_at: session.updated_at,
        awaiting_input: session.awaiting_input,
    }
}

/// Serialize a list of sessions to a JSON array (pretty-printed) followed by a newline.
fn write_sessions_json<W: Write>(mut writer: W, sessions: Vec<SessionState>) -> Result<()> {
    let dtos: Vec<ListSessionJson> = sessions.into_iter().map(session_to_json).collect();
    serde_json::to_writer_pretty(&mut writer, &dtos)
        .map_err(|e| CruiseError::Other(format!("JSON serialization error: {e}")))?;
    writer
        .write_all(b"\n")
        .map_err(|e| CruiseError::Other(format!("write error: {e}")))?;
    Ok(())
}

pub async fn run(args: ListArgs) -> Result<()> {
    let manager = SessionManager::new(get_cruise_home()?);

    if args.json {
        let sessions = manager.list()?;
        write_sessions_json(std::io::BufWriter::new(std::io::stdout()), sessions)?;
        return Ok(());
    }

    loop {
        let Some(mut session) = pick_session(&manager)? else {
            return Ok(());
        };

        loop {
            // Show plan.md content.
            let plan_path = session.plan_path(&manager.sessions_dir());
            if let Ok(content) = std::fs::read_to_string(&plan_path) {
                crate::display::print_bordered(&content, Some("plan.md"));
            }

            // Action menu.
            let actions = session_actions(&session);

            let action = match inquire::Select::new("Action:", actions).prompt() {
                Ok(a) => a,
                Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => "Back",
                Err(e) => return Err(CruiseError::Other(format!("selection error: {e}"))),
            };

            match action {
                "Approve" => {
                    if let Err(err) =
                        crate::metadata::refresh_session_title_from_session(&manager, &mut session)
                    {
                        eprintln!("warning: failed to refresh session title: {err}");
                    }
                    session.approve();
                    manager.save(&session)?;
                    eprintln!(
                        "{} Session {} approved. Run with: {}",
                        style("✓").green(),
                        session.id,
                        style(format!("cruise run {}", session.id)).cyan()
                    );
                }
                "Run" | "Resume" => {
                    let run_args = crate::cli::RunArgs {
                        session: Some(session.id.clone()),
                        all: false,
                        max_retries: DEFAULT_MAX_RETRIES,
                        rate_limit_retries: DEFAULT_RATE_LIMIT_RETRIES,
                        dry_run: false,
                    };
                    return crate::run_cmd::run(run_args).await;
                }
                "Replan" => {
                    let text = match prompt_multiline("Describe the changes needed:")? {
                        InputResult::Submitted(t) => t,
                        InputResult::Cancelled => continue,
                    };
                    crate::plan_cmd::replan_session(
                        &manager,
                        &mut session,
                        text,
                        DEFAULT_RATE_LIMIT_RETRIES,
                    )
                    .await?;
                    // Re-load so subsequent session_actions(&session) uses fresh state.
                    session = manager.load(&session.id)?;
                }
                "Open PR" => {
                    let url = session.pr_url.as_deref().ok_or_else(|| {
                        CruiseError::Other("Open PR action requires pr_url".into())
                    })?;
                    match open_pr_in_browser(url) {
                        Ok(()) => {
                            eprintln!("{} Opening PR in browser…", style("✓").green());
                        }
                        Err(e) => {
                            eprintln!("{} {e}", style("✗").red());
                        }
                    }
                }
                "Reset to Planned" => {
                    session.reset_to_planned();
                    manager.save(&session)?;
                    eprintln!(
                        "{} Session {} reset to Planned.",
                        style("✓").green(),
                        session.id
                    );
                }
                "Delete" => {
                    manager.delete(&session.id)?;
                    eprintln!("{} Session {} deleted.", style("✓").green(), session.id);
                    break;
                }
                _ => {
                    // "Back" — return to the session list.
                    break;
                }
            }
        }
    }
}

/// Prompts the user to select a session from the list.
/// Returns `Ok(None)` if the list is empty or the user cancels.
fn pick_session(manager: &crate::session::SessionManager) -> Result<Option<SessionState>> {
    let sessions = manager.list()?;
    if sessions.is_empty() {
        eprintln!("No sessions found.");
        return Ok(None);
    }
    let labels: Vec<String> = sessions.iter().map(format_session_label).collect();
    let label_refs: Vec<&str> = labels.iter().map(std::string::String::as_str).collect();
    let selected = match inquire::Select::new("Select a session:", label_refs).prompt() {
        Ok(s) => s,
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => {
            return Ok(None);
        }
        Err(e) => return Err(CruiseError::Other(format!("selection error: {e}"))),
    };
    let Some(idx) = labels.iter().position(|l| l.as_str() == selected) else {
        return Err(CruiseError::Other(format!(
            "selected label not found: {selected}"
        )));
    };
    Ok(Some(sessions[idx].clone()))
}

/// Returns the action menu items available for the given session.
/// "Run"/"Resume" appears for runnable phases; "Replan" only for Planned.
/// "Open PR" appears for Completed sessions that have a PR URL.
/// "Reset to Planned" appears for Running, Failed, Completed, and Suspended.
/// "Delete" and "Back" are always present (in that order) at the end.
fn session_actions(session: &SessionState) -> Vec<&'static str> {
    let mut actions = vec![];
    match &session.phase {
        SessionPhase::AwaitingApproval => {
            actions.push("Approve");
        }
        SessionPhase::Planned => {
            actions.push("Run");
            actions.push("Replan");
        }
        SessionPhase::Running | SessionPhase::Suspended => {
            actions.push("Resume");
            actions.push("Reset to Planned");
        }
        SessionPhase::Failed(_) => {
            actions.push("Run");
            actions.push("Reset to Planned");
        }
        SessionPhase::Completed => {
            if session.pr_url.is_some() {
                actions.push("Open PR");
            }
            actions.push("Reset to Planned");
        }
    }
    actions.push("Delete");
    actions.push("Back");
    actions
}

fn open_pr_in_browser(pr_url: &str) -> crate::error::Result<()> {
    let status = std::process::Command::new("gh")
        .args(["pr", "view", pr_url, "--web"])
        .status()
        .map_err(|e| CruiseError::Other(format!("failed to run gh: {e}")))?;
    if !status.success() {
        return Err(CruiseError::Other(format!(
            "gh pr view --web exited with {status}"
        )));
    }
    Ok(())
}

fn format_session_label(s: &SessionState) -> String {
    let (icon, phase_str) = match &s.phase {
        SessionPhase::AwaitingApproval => {
            (style("○").magenta(), style("Awaiting Approval").magenta())
        }
        SessionPhase::Planned => (style("●").cyan(), style("Planned").cyan()),
        SessionPhase::Running => (style("▶").yellow(), style("Running").yellow()),
        SessionPhase::Completed => (style("✓").green(), style("Completed").green()),
        SessionPhase::Failed(_) => (style("✗").red(), style("Failed").red()),
        SessionPhase::Suspended => (style("⏸").yellow(), style("Suspended").yellow()),
    };
    let date = format_session_date(&s.id);
    let suffix = format_suffix(s);
    let input_preview = crate::display::truncate(s.title_or_input(), 60);
    format!("{icon} {date} {phase_str} {input_preview}{suffix}")
}

/// "`YYYYMMDDHHmmss`" → "MM/DD HH:MM"
fn format_session_date(id: &str) -> String {
    let (Some(month), Some(day), Some(hour), Some(min)) =
        (id.get(4..6), id.get(6..8), id.get(8..10), id.get(10..12))
    else {
        return id.to_string();
    };
    format!("{month}/{day} {hour}:{min}")
}

/// Returns " \[`step_name`\]" for Running/Suspended, or " PR#N" for Completed with PR URL.
fn format_suffix(s: &SessionState) -> String {
    match &s.phase {
        SessionPhase::Running | SessionPhase::Suspended => s
            .current_step
            .as_ref()
            .map(|step| format!(" [{step}]"))
            .unwrap_or_default(),
        SessionPhase::Completed => s
            .pr_url
            .as_ref()
            .map(|url| {
                let num = url.trim_end_matches('/').rsplit('/').next().unwrap_or("");
                format!(" PR#{num}")
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // -----------------------------------------------------------------------
    // session_actions
    // -----------------------------------------------------------------------

    #[test]
    fn test_session_actions_planned_has_run_and_replan() {
        // Given: Planned phase
        let session = make_session("20260306143000", "task", SessionPhase::Planned);

        // When
        let actions = session_actions(&session);

        // Then: contains "Run" and "Replan"; also contains "Delete" and "Back"
        assert!(
            actions.contains(&"Run"),
            "Planned should have Run: {actions:?}"
        );
        assert!(
            actions.contains(&"Replan"),
            "Planned should have Replan: {actions:?}"
        );
        assert!(
            actions.contains(&"Delete"),
            "should always have Delete: {actions:?}"
        );
        assert!(
            actions.contains(&"Back"),
            "should always have Back: {actions:?}"
        );
    }

    #[test]
    fn test_session_actions_planned_has_no_resume() {
        // Given: Planned phase
        let session = make_session("20260306143000", "task", SessionPhase::Planned);

        // When
        let actions = session_actions(&session);

        // Then: "Resume" is absent (Run is used for a fresh start, not Resume)
        assert!(
            !actions.contains(&"Resume"),
            "Planned should NOT have Resume: {actions:?}"
        );
    }

    #[test]
    fn test_session_actions_running_has_resume_not_replan() {
        // Given: Running phase
        let session = make_session("20260306143000", "task", SessionPhase::Running);

        // When
        let actions = session_actions(&session);

        // Then: "Resume" is present but "Replan" is absent
        assert!(
            actions.contains(&"Resume"),
            "Running should have Resume: {actions:?}"
        );
        assert!(
            !actions.contains(&"Replan"),
            "Running should NOT have Replan: {actions:?}"
        );
        assert!(
            !actions.contains(&"Run"),
            "Running should NOT have Run (use Resume): {actions:?}"
        );
    }

    #[test]
    fn test_session_actions_failed_has_run_not_replan() {
        // Given: Failed phase
        let session = make_session(
            "20260306143000",
            "task",
            SessionPhase::Failed("some error".to_string()),
        );

        // When
        let actions = session_actions(&session);

        // Then: "Run" is present but "Replan" is absent
        assert!(
            actions.contains(&"Run"),
            "Failed should have Run: {actions:?}"
        );
        assert!(
            !actions.contains(&"Replan"),
            "Failed should NOT have Replan: {actions:?}"
        );
    }

    #[test]
    fn test_session_actions_completed_has_no_run_no_replan_has_reset() {
        // Given: Completed phase, no pr_url
        let session = make_session("20260306143000", "task", SessionPhase::Completed);

        // When
        let actions = session_actions(&session);

        // Then: "Run", "Resume", and "Replan" are absent; "Reset to Planned" is present
        assert!(
            !actions.contains(&"Run"),
            "Completed should NOT have Run: {actions:?}"
        );
        assert!(
            !actions.contains(&"Resume"),
            "Completed should NOT have Resume: {actions:?}"
        );
        assert!(
            !actions.contains(&"Replan"),
            "Completed should NOT have Replan: {actions:?}"
        );
        assert!(
            actions.contains(&"Reset to Planned"),
            "Completed should have Reset to Planned: {actions:?}"
        );
        assert!(
            actions.contains(&"Delete"),
            "should always have Delete: {actions:?}"
        );
        assert!(
            actions.contains(&"Back"),
            "should always have Back: {actions:?}"
        );
    }

    #[test]
    fn test_session_actions_planned_run_before_replan() {
        // Given: Planned phase
        let session = make_session("20260306143000", "task", SessionPhase::Planned);

        // When
        let actions = session_actions(&session);

        // Then: "Run" appears before "Replan" (primary action first)
        let run_pos = actions
            .iter()
            .position(|&a| a == "Run")
            .unwrap_or_else(|| panic!("unexpected None"));
        let replan_pos = actions
            .iter()
            .position(|&a| a == "Replan")
            .unwrap_or_else(|| panic!("unexpected None"));
        assert!(
            run_pos < replan_pos,
            "Run should come before Replan in actions list"
        );
    }

    #[test]
    fn test_session_actions_delete_and_back_always_at_end() {
        // Given: Delete and Back are the last two entries across all phases
        let sessions = [
            make_session("20260306143000", "task", SessionPhase::AwaitingApproval),
            make_session("20260306143000", "task", SessionPhase::Planned),
            make_session("20260306143000", "task", SessionPhase::Running),
            make_session("20260306143000", "task", SessionPhase::Completed),
            make_session(
                "20260306143000",
                "task",
                SessionPhase::Failed("err".to_string()),
            ),
        ];

        for session in &sessions {
            let phase = &session.phase;
            // When
            let actions = session_actions(session);
            let len = actions.len();

            // Then: Back is last, Delete is second-to-last
            assert!(
                len >= 2,
                "actions must have at least 2 items for {phase:?}: {actions:?}"
            );
            assert_eq!(
                actions[len - 1],
                "Back",
                "Back should be last for {phase:?}: {actions:?}"
            );
            assert_eq!(
                actions[len - 2],
                "Delete",
                "Delete should be second-to-last for {phase:?}: {actions:?}"
            );
        }
    }

    fn make_session(id: &str, input: &str, phase: SessionPhase) -> SessionState {
        let mut s = SessionState::new(
            id.to_string(),
            PathBuf::from("/tmp"),
            "cruise.yaml".to_string(),
            input.to_string(),
        );
        s.phase = phase;
        s
    }

    // -----------------------------------------------------------------------
    // format_session_date
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_session_date_standard_id_returns_mm_dd_hh_mm() {
        // Given: standard 14-digit session ID
        let id = "20260306143000";

        // When
        let result = format_session_date(id);

        // Then: converted to "MM/DD HH:MM" format
        assert_eq!(result, "03/06 14:30");
    }

    #[test]
    fn test_format_session_date_twelve_digit_id_is_accepted() {
        // Given: 12-digit (no seconds) ID
        let id = "202603061430";

        // When
        let result = format_session_date(id);

        // Then: converted to "03/06 14:30"
        assert_eq!(result, "03/06 14:30");
    }

    #[test]
    fn test_format_session_date_midnight() {
        // Given: session at midnight (00:00)
        let id = "20260101000000";

        // When
        let result = format_session_date(id);

        // Then
        assert_eq!(result, "01/01 00:00");
    }

    // -----------------------------------------------------------------------
    // format_suffix
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_suffix_running_with_step_returns_step_bracket() {
        // Given: Running phase, current_step present
        let mut s = make_session("20260306143000", "add feature", SessionPhase::Running);
        s.current_step = Some("implement".to_string());

        // When
        let result = format_suffix(&s);

        // Then: "[implement]" format
        assert_eq!(result, " [implement]");
    }

    #[test]
    fn test_format_suffix_running_without_step_returns_empty() {
        // Given: Running phase, no current_step
        let s = make_session("20260306143000", "add feature", SessionPhase::Running);

        // When
        let result = format_suffix(&s);

        // Then: empty string
        assert_eq!(result, "");
    }

    #[test]
    fn test_format_suffix_completed_with_pr_url_returns_pr_number() {
        // Given: Completed phase, PR URL present
        let mut s = make_session("20260306143000", "add feature", SessionPhase::Completed);
        s.pr_url = Some("https://github.com/owner/repo/pull/42".to_string());

        // When
        let result = format_suffix(&s);

        // Then: "PR#42" format
        assert_eq!(result, " PR#42");
    }

    #[test]
    fn test_format_suffix_completed_without_pr_url_returns_empty() {
        // Given: Completed phase, no PR URL
        let s = make_session("20260306143000", "add feature", SessionPhase::Completed);

        // When
        let result = format_suffix(&s);

        // Then: empty string
        assert_eq!(result, "");
    }

    #[test]
    fn test_format_suffix_planned_returns_empty() {
        // Given: Planned phase
        let s = make_session("20260306143000", "add feature", SessionPhase::Planned);

        // When
        let result = format_suffix(&s);

        // Then: empty string
        assert_eq!(result, "");
    }

    #[test]
    fn test_format_suffix_failed_returns_empty() {
        // Given: Failed phase
        let s = make_session(
            "20260306143000",
            "add feature",
            SessionPhase::Failed("timeout".to_string()),
        );

        // When
        let result = format_suffix(&s);

        // Then: empty string
        assert_eq!(result, "");
    }

    // -----------------------------------------------------------------------
    // format_session_label (expected values for new format)
    // -----------------------------------------------------------------------

    /// Helper to strip ANSI escapes and verify label content.
    fn strip(s: &str) -> String {
        console::strip_ansi_codes(s).to_string()
    }

    #[test]
    fn test_format_session_label_planned_contains_icon_date_phase_input() {
        // Given: Planned session
        let s = make_session(
            "20260306143000",
            "add hello world feature",
            SessionPhase::Planned,
        );

        // When
        let label = strip(&format_session_label(&s));

        // Then: contains icon, date, phase, and input
        assert!(label.contains('●'), "should contain ● icon: {label}");
        assert!(
            label.contains("03/06 14:30"),
            "should contain date: {label}"
        );
        assert!(label.contains("Planned"), "should contain phase: {label}");
        assert!(
            label.contains("add hello world feature"),
            "should contain input: {label}"
        );
    }

    #[test]
    fn test_format_session_label_running_contains_running_icon_and_step() {
        // Given: Running phase, current_step present
        let mut s = make_session("20260307150000", "implement auth", SessionPhase::Running);
        s.current_step = Some("test".to_string());

        // When
        let label = strip(&format_session_label(&s));

        // Then: contains ▶ icon and step info
        assert!(label.contains('▶'), "should contain ▶ icon: {label}");
        assert!(label.contains("Running"), "should contain Running: {label}");
        assert!(label.contains("[test]"), "should contain step: {label}");
    }

    #[test]
    fn test_format_session_label_completed_with_pr_contains_checkmark_and_pr() {
        // Given: Completed phase, PR URL present
        let mut s = make_session("20260307090000", "refactor db", SessionPhase::Completed);
        s.pr_url = Some("https://github.com/owner/repo/pull/42".to_string());

        // When
        let label = strip(&format_session_label(&s));

        // Then: contains ✓ icon and PR number
        assert!(label.contains('✓'), "should contain ✓ icon: {label}");
        assert!(
            label.contains("Completed"),
            "should contain Completed: {label}"
        );
        assert!(label.contains("PR#42"), "should contain PR#42: {label}");
    }

    #[test]
    fn test_format_session_label_failed_contains_cross_icon() {
        // Given: Failed phase
        let s = make_session(
            "20260307103000",
            "fix login bug",
            SessionPhase::Failed("exit 1".to_string()),
        );

        // When
        let label = strip(&format_session_label(&s));

        // Then: contains ✗ icon
        assert!(label.contains('✗'), "should contain ✗ icon: {label}");
        assert!(label.contains("Failed"), "should contain Failed: {label}");
    }

    #[test]
    fn test_format_session_label_long_input_is_truncated() {
        // Given: very long input
        let long_input = "a".repeat(200);
        let s = make_session("20260306143000", &long_input, SessionPhase::Planned);

        // When
        let label = strip(&format_session_label(&s));

        // Then: contains ellipsis "…" and total label length is 200 chars or less
        assert!(
            label.contains('…'),
            "long input should be truncated: {label}"
        );
    }

    #[test]
    fn test_format_session_label_prefers_title_over_input() {
        // Given: a session with both raw input and a generated title
        let mut s = make_session(
            "20260306143000",
            "raw task input that should not be the primary label",
            SessionPhase::Planned,
        );
        s.title = Some("Generated session title".to_string());

        // When
        let label = strip(&format_session_label(&s));

        // Then: the generated title is shown instead of the raw input
        assert!(
            label.contains("Generated session title"),
            "should contain generated title: {label}"
        );
        assert!(
            !label.contains("raw task input that should not be the primary label"),
            "should not contain raw input when title is present: {label}"
        );
    }

    #[test]
    fn test_format_session_label_falls_back_to_input_when_title_missing() {
        // Given: a session without a generated title
        let s = make_session(
            "20260306143000",
            "raw task input remains visible",
            SessionPhase::Planned,
        );

        // When
        let label = strip(&format_session_label(&s));

        // Then: the raw input remains the visible fallback
        assert!(
            label.contains("raw task input remains visible"),
            "should contain raw input fallback: {label}"
        );
    }

    // -----------------------------------------------------------------------
    // session_actions — Reset to Planned coverage
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // session_actions — Suspended
    // -----------------------------------------------------------------------

    #[test]
    fn test_session_actions_suspended_exact() {
        // Given / When / Then: Suspended action list matches expectations
        assert_eq!(
            session_actions(&make_session("test", "test", SessionPhase::Suspended)),
            vec!["Resume", "Reset to Planned", "Delete", "Back"]
        );
    }

    // -----------------------------------------------------------------------
    // format_suffix — Suspended
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_suffix_suspended_with_step_returns_step_bracket() {
        // Given: Suspended phase, current_step present
        let mut s = make_session("20260310143000", "add feature", SessionPhase::Suspended);
        s.current_step = Some("implement".to_string());

        // When
        let result = format_suffix(&s);

        // Then: "[implement]" format
        assert_eq!(result, " [implement]");
    }

    #[test]
    fn test_format_suffix_suspended_without_step_returns_empty() {
        // Given: Suspended phase, no current_step
        let s = make_session("20260310143000", "add feature", SessionPhase::Suspended);

        // When
        let result = format_suffix(&s);

        // Then: empty string
        assert_eq!(result, "");
    }

    // -----------------------------------------------------------------------
    // format_session_label — Suspended
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_session_label_suspended_contains_phase_and_step() {
        // Given: Suspended phase, current_step present
        let mut s = make_session("20260310150000", "fix auth", SessionPhase::Suspended);
        s.current_step = Some("test".to_string());

        // When
        let label = strip(&format_session_label(&s));

        // Then: contains "Suspended" phase and the suspended step name
        assert!(
            label.contains("Suspended"),
            "should contain Suspended: {label}"
        );
        assert!(label.contains("[test]"), "should contain step: {label}");
    }

    // -----------------------------------------------------------------------
    // session_actions — Delete/Back tail check (all phases including Suspended)
    // -----------------------------------------------------------------------

    #[test]
    fn test_session_actions_delete_and_back_always_at_end_including_suspended() {
        // Given: all phases including Suspended
        let phases = [
            SessionPhase::Planned,
            SessionPhase::Running,
            SessionPhase::Completed,
            SessionPhase::Failed("err".to_string()),
            SessionPhase::Suspended,
        ];

        for phase in &phases {
            // When
            let actions = session_actions(&make_session("test", "test", phase.clone()));
            let len = actions.len();

            // Then: Back is last, Delete is second-to-last
            assert!(
                len >= 2,
                "actions must have at least 2 items for {phase:?}: {actions:?}"
            );
            assert_eq!(
                actions[len - 1],
                "Back",
                "Back should be last for {phase:?}: {actions:?}"
            );
            assert_eq!(
                actions[len - 2],
                "Delete",
                "Delete should be second-to-last for {phase:?}: {actions:?}"
            );
        }
    }

    #[test]
    fn test_session_actions_planned_exact() {
        let session = make_session("20260306143000", "task", SessionPhase::Planned);
        assert_eq!(
            session_actions(&session),
            vec!["Run", "Replan", "Delete", "Back"]
        );
    }

    #[test]
    fn test_session_actions_running_has_reset_to_planned() {
        let session = make_session("20260306143000", "task", SessionPhase::Running);
        assert_eq!(
            session_actions(&session),
            vec!["Resume", "Reset to Planned", "Delete", "Back"]
        );
    }

    #[test]
    fn test_session_actions_completed_has_reset_to_planned() {
        // Given: Completed + no pr_url
        let session = make_session("20260306143000", "task", SessionPhase::Completed);
        assert_eq!(
            session_actions(&session),
            vec!["Reset to Planned", "Delete", "Back"]
        );
    }

    #[test]
    fn test_session_actions_failed_has_run_and_reset_to_planned() {
        let session = make_session(
            "20260306143000",
            "task",
            SessionPhase::Failed("exit 1".to_string()),
        );
        assert_eq!(
            session_actions(&session),
            vec!["Run", "Reset to Planned", "Delete", "Back"]
        );
    }

    // -----------------------------------------------------------------------
    // session_actions — Open PR coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_session_actions_completed_with_pr_url_exact_order() {
        // Given: Completed + pr_url present
        let mut session = make_session("20260306143000", "task", SessionPhase::Completed);
        session.pr_url = Some("https://github.com/owner/repo/pull/10".to_string());

        // When
        let actions = session_actions(&session);

        // Then: order is ["Open PR", "Reset to Planned", "Delete", "Back"]
        assert_eq!(
            actions,
            vec!["Open PR", "Reset to Planned", "Delete", "Back"]
        );
    }

    // -----------------------------------------------------------------------
    // open_pr_in_browser
    // -----------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn test_open_pr_in_browser_calls_gh_view_web() {
        use std::os::unix::fs::PermissionsExt;
        use std::{fs, io::Read};

        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap_or_else(|e| panic!("{e:?}"));
        let log_path = tmp.path().join("gh.log");

        // fake gh: records args to log file then exits 0
        let script_path = bin_dir.join("gh");
        fs::write(
            &script_path,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"{}\"\n",
                log_path.display()
            ),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let mut perms = fs::metadata(&script_path)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap_or_else(|e| panic!("{e:?}"));

        let _guard = crate::test_binary_support::PathEnvGuard::prepend(&bin_dir);

        let url = "https://github.com/owner/repo/pull/42";
        let result = open_pr_in_browser(url);

        assert!(result.is_ok(), "should succeed: {result:?}");

        // Verify log: "pr view <url> --web" was passed
        let mut log_content = String::new();
        fs::File::open(&log_path)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .read_to_string(&mut log_content)
            .unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            log_content.contains("pr view"),
            "gh should receive 'pr view': {log_content}"
        );
        assert!(
            log_content.contains(url),
            "gh should receive the PR url: {log_content}"
        );
        assert!(
            log_content.contains("--web"),
            "gh should receive '--web': {log_content}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_open_pr_in_browser_gh_failure_returns_error() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap_or_else(|e| panic!("{e:?}"));
        let bin_dir = tmp.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap_or_else(|e| panic!("{e:?}"));

        // fake gh: always exits 1
        let script_path = bin_dir.join("gh");
        fs::write(&script_path, "#!/bin/sh\nexit 1\n").unwrap_or_else(|e| panic!("{e:?}"));
        let mut perms = fs::metadata(&script_path)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap_or_else(|e| panic!("{e:?}"));

        let _guard = crate::test_binary_support::PathEnvGuard::prepend(&bin_dir);

        let result = open_pr_in_browser("https://github.com/owner/repo/pull/1");

        assert!(result.is_err(), "should fail when gh exits non-zero");
    }

    // -----------------------------------------------------------------------
    // AwaitingApproval phase — actions and labels
    // -----------------------------------------------------------------------

    #[test]
    fn test_session_actions_awaiting_approval_has_approve() {
        // Given: AwaitingApproval phase
        let session = make_session("20260311100000", "task", SessionPhase::AwaitingApproval);

        // When
        let actions = session_actions(&session);

        // Then: contains "Approve" action
        assert!(
            actions.contains(&"Approve"),
            "AwaitingApproval should have Approve: {actions:?}"
        );
    }

    #[test]
    fn test_session_actions_awaiting_approval_has_no_run_no_resume() {
        // Given: AwaitingApproval phase
        let session = make_session("20260311100000", "task", SessionPhase::AwaitingApproval);

        // When
        let actions = session_actions(&session);

        // Then: neither "Run" nor "Resume" since it is not yet approved
        assert!(
            !actions.contains(&"Run"),
            "AwaitingApproval should NOT have Run: {actions:?}"
        );
        assert!(
            !actions.contains(&"Resume"),
            "AwaitingApproval should NOT have Resume: {actions:?}"
        );
    }

    #[test]
    fn test_session_actions_awaiting_approval_exact_order() {
        // Given: AwaitingApproval phase
        let session = make_session("20260311100000", "task", SessionPhase::AwaitingApproval);

        // When / Then: order is Approve → Delete → Back
        assert_eq!(session_actions(&session), vec!["Approve", "Delete", "Back"]);
    }

    #[test]
    fn test_format_session_label_awaiting_approval_contains_phase_text() {
        // Given: AwaitingApproval phase session
        let s = make_session(
            "20260311100000",
            "pending task",
            SessionPhase::AwaitingApproval,
        );

        // When
        let label = strip(&format_session_label(&s));

        // Then: contains "Awaiting Approval" text and icon
        assert!(
            label.contains("Awaiting Approval"),
            "label should contain 'Awaiting Approval': {label}"
        );
        assert!(label.contains('○'), "label should contain ○ icon: {label}");
        assert!(
            label.contains("pending task"),
            "label should contain input: {label}"
        );
    }

    #[test]
    fn test_format_session_label_awaiting_approval_not_planned_text() {
        // Given: AwaitingApproval phase session
        let s = make_session(
            "20260311100001",
            "some task",
            SessionPhase::AwaitingApproval,
        );

        // When
        let label = strip(&format_session_label(&s));

        // Then: "Planned" text is absent (to avoid phase confusion)
        assert!(
            !label.contains("Planned"),
            "AwaitingApproval label should NOT contain 'Planned': {label}"
        );
    }

    // ── format_session_label: multiline input ─────────────────────────────────

    #[test]
    fn test_format_session_label_multiline_input_shows_first_line_only() {
        // Given: session.input contains multiple lines (e.g. input with embedded newlines)
        let s = make_session(
            "20260306143000",
            "line1\nline2\nline3",
            SessionPhase::Planned,
        );

        // When
        let label = strip(&format_session_label(&s));

        // Then: only the first line appears in the label; remaining lines are absent
        assert!(
            label.contains("line1"),
            "label must contain first line: {label}"
        );
        assert!(
            !label.contains("line2"),
            "label must NOT contain second line: {label}"
        );
        assert!(
            !label.contains("line3"),
            "label must NOT contain third line: {label}"
        );
    }

    #[test]
    fn test_format_session_label_multiline_input_does_not_contain_newline_char() {
        // Given: multi-line input
        let s = make_session(
            "20260306143000",
            "implement feature\nwith extra detail",
            SessionPhase::Planned,
        );

        // When
        let label = strip(&format_session_label(&s));

        // Then: label contains no newline characters (displayable as a single list row)
        assert!(
            !label.contains('\n'),
            "label must not contain newline character: {label:?}"
        );
    }

    // -----------------------------------------------------------------------
    // session_to_json
    // -----------------------------------------------------------------------

    #[test]
    fn test_session_to_json_failed_phase_has_phase_string_and_error() {
        // Given: a session in Failed phase with an error message
        let session = make_session(
            "20260306143000",
            "task",
            SessionPhase::Failed("db error".to_string()),
        );

        // When
        let dto = session_to_json(session);

        // Then: phase is "Failed" and phase_error contains the message
        assert_eq!(dto.phase, "Failed");
        assert_eq!(dto.phase_error, Some("db error".to_string()));
    }

    #[test]
    fn test_session_to_json_all_non_failed_phases_have_null_phase_error() {
        // Given: all non-Failed phases
        let cases = [
            (SessionPhase::AwaitingApproval, "AwaitingApproval"),
            (SessionPhase::Planned, "Planned"),
            (SessionPhase::Running, "Running"),
            (SessionPhase::Completed, "Completed"),
            (SessionPhase::Suspended, "Suspended"),
        ];

        for (phase, expected_str) in cases {
            // When
            let session = make_session("20260306143000", "task", phase);
            let dto = session_to_json(session);

            // Then: phase string matches and phase_error is None
            assert_eq!(
                dto.phase, expected_str,
                "phase string mismatch for {expected_str}"
            );
            assert_eq!(
                dto.phase_error, None,
                "phase_error should be None for {expected_str}"
            );
        }
    }

    #[test]
    fn test_session_to_json_path_fields_are_strings() {
        // Given: session with base_dir and optional path fields set
        let mut session = make_session("20260306143000", "task", SessionPhase::Planned);
        session.worktree_path = Some(PathBuf::from("/tmp/worktree"));
        session.config_path = Some(PathBuf::from("/home/user/config.yaml"));

        // When
        let dto = session_to_json(session);

        // Then: path fields are serialized as strings
        assert_eq!(dto.base_dir, "/tmp");
        assert_eq!(dto.worktree_path, Some("/tmp/worktree".to_string()));
        assert_eq!(dto.config_path, Some("/home/user/config.yaml".to_string()));
    }

    #[test]
    fn test_session_to_json_null_optional_paths_are_none() {
        let session = make_session("20260306143000", "task", SessionPhase::Planned);
        let dto = session_to_json(session);
        assert_eq!(dto.worktree_path, None);
        assert_eq!(dto.config_path, None);
    }

    #[test]
    fn test_session_to_json_id_and_input_are_preserved() {
        let session = make_session(
            "20260306143000",
            "my task description",
            SessionPhase::Planned,
        );
        let dto = session_to_json(session);
        assert_eq!(dto.id, "20260306143000");
        assert_eq!(dto.input, "my task description");
    }

    // -----------------------------------------------------------------------
    // write_sessions_json
    // -----------------------------------------------------------------------

    #[test]
    fn test_write_sessions_json_empty_list_produces_empty_json_array() {
        // Given: an empty session list
        let sessions: Vec<SessionState> = vec![];
        let mut buf = Vec::new();

        // When
        write_sessions_json(&mut buf, sessions).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: output parses as a JSON array with 0 entries
        let output = String::from_utf8(buf).unwrap_or_else(|e| panic!("{e:?}"));
        let value: serde_json::Value =
            serde_json::from_str(&output).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(value.is_array(), "output should be a JSON array");
        assert_eq!(
            value
                .as_array()
                .unwrap_or_else(|| panic!("expected JSON array"))
                .len(),
            0,
            "empty input should produce an empty array"
        );
    }

    #[test]
    fn test_write_sessions_json_multiple_sessions_produces_array_with_correct_ids() {
        // Given: two sessions with distinct IDs
        let sessions = vec![
            make_session("20260306143000", "task A", SessionPhase::Planned),
            make_session("20260306144500", "task B", SessionPhase::Completed),
        ];
        let mut buf = Vec::new();

        // When
        write_sessions_json(&mut buf, sessions).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: JSON array contains 2 entries with the expected IDs
        let output = String::from_utf8(buf).unwrap_or_else(|e| panic!("{e:?}"));
        let value: serde_json::Value =
            serde_json::from_str(&output).unwrap_or_else(|e| panic!("{e:?}"));
        let arr = value
            .as_array()
            .unwrap_or_else(|| panic!("expected JSON array"));
        assert_eq!(arr.len(), 2, "should have 2 sessions");
        assert_eq!(arr[0]["id"], "20260306143000");
        assert_eq!(arr[1]["id"], "20260306144500");
    }

    #[test]
    fn test_write_sessions_json_failed_phase_is_normalized() {
        // Given: a session in Failed phase
        let sessions = vec![make_session(
            "20260306143000",
            "task",
            SessionPhase::Failed("some error".to_string()),
        )];
        let mut buf = Vec::new();

        // When
        write_sessions_json(&mut buf, sessions).unwrap_or_else(|e| panic!("{e:?}"));

        // Then: JSON entry has phase="Failed" and phase_error="some error"
        let output = String::from_utf8(buf).unwrap_or_else(|e| panic!("{e:?}"));
        let value: serde_json::Value =
            serde_json::from_str(&output).unwrap_or_else(|e| panic!("{e:?}"));
        let entry = &value
            .as_array()
            .unwrap_or_else(|| panic!("expected JSON array"))[0];
        assert_eq!(entry["phase"], "Failed");
        assert_eq!(entry["phase_error"], "some error");
    }

    #[test]
    fn test_write_sessions_json_output_ends_with_newline() {
        let sessions: Vec<SessionState> = vec![];
        let mut buf = Vec::new();
        write_sessions_json(&mut buf, sessions).unwrap_or_else(|e| panic!("{e:?}"));
        assert!(
            buf.ends_with(b"\n"),
            "JSON output should end with a newline"
        );
    }
}
