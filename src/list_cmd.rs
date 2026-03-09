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
        let can_run = session.phase.is_runnable();

        let mut actions = vec![];
        if can_run {
            actions.push(if session.phase.is_running() {
                "Resume"
            } else {
                "Run"
            });
        }
        actions.push("Delete");
        actions.push("Back");

        let action = match inquire::Select::new("Action:", actions).prompt() {
            Ok(a) => a,
            Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => "Back",
            Err(e) => return Err(CruiseError::Other(format!("selection error: {e}"))),
        };

        match action {
            "Run" | "Resume" => {
                let run_args = crate::cli::RunArgs {
                    session: Some(session.id.clone()),
                    all: false,
                    max_retries: 10,
                    rate_limit_retries: 5,
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
    let (icon, phase_str) = match &s.phase {
        SessionPhase::Planned => (style("●").cyan(), style("Planned").cyan()),
        SessionPhase::Running => (style("▶").yellow(), style("Running").yellow()),
        SessionPhase::Completed => (style("✓").green(), style("Completed").green()),
        SessionPhase::Failed(_) => (style("✗").red(), style("Failed").red()),
    };
    let date = format_session_date(&s.id);
    let suffix = format_suffix(s);
    let input_preview = crate::display::truncate(&s.input, 60);
    format!("{icon} {date} {phase_str} {input_preview}{suffix}")
}

/// "YYYYMMDDHHmmss" → "MM/DD HH:MM"
fn format_session_date(id: &str) -> String {
    let (Some(month), Some(day), Some(hour), Some(min)) =
        (id.get(4..6), id.get(6..8), id.get(8..10), id.get(10..12))
    else {
        return id.to_string();
    };
    format!("{month}/{day} {hour}:{min}")
}

/// Running 時は " [step_name]"、Completed+PR 時は " PR#N" を返す。
fn format_suffix(s: &SessionState) -> String {
    match &s.phase {
        SessionPhase::Running => s
            .current_step
            .as_ref()
            .map(|step| format!(" [{step}]"))
            .unwrap_or_default(),
        SessionPhase::Completed => s
            .pr_url
            .as_ref()
            .map(|url| {
                let num = url.trim_end_matches('/').rsplit('/').next().unwrap();
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
        // Given: 標準的な 14 桁のセッション ID
        let id = "20260306143000";

        // When
        let result = format_session_date(id);

        // Then: "MM/DD HH:MM" 形式に変換される
        assert_eq!(result, "03/06 14:30");
    }

    #[test]
    fn test_format_session_date_twelve_digit_id_is_accepted() {
        // Given: 12 桁（秒なし）の ID
        let id = "202603061430";

        // When
        let result = format_session_date(id);

        // Then: "03/06 14:30" として変換される
        assert_eq!(result, "03/06 14:30");
    }

    #[test]
    fn test_format_session_date_midnight() {
        // Given: 00:00 のセッション
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
        // Given: Running フェーズ、current_step あり
        let mut s = make_session("20260306143000", "add feature", SessionPhase::Running);
        s.current_step = Some("implement".to_string());

        // When
        let result = format_suffix(&s);

        // Then: "[implement]" 形式
        assert_eq!(result, " [implement]");
    }

    #[test]
    fn test_format_suffix_running_without_step_returns_empty() {
        // Given: Running フェーズ、current_step なし
        let s = make_session("20260306143000", "add feature", SessionPhase::Running);

        // When
        let result = format_suffix(&s);

        // Then: 空文字
        assert_eq!(result, "");
    }

    #[test]
    fn test_format_suffix_completed_with_pr_url_returns_pr_number() {
        // Given: Completed フェーズ、PR URL あり
        let mut s = make_session("20260306143000", "add feature", SessionPhase::Completed);
        s.pr_url = Some("https://github.com/owner/repo/pull/42".to_string());

        // When
        let result = format_suffix(&s);

        // Then: "PR#42" 形式
        assert_eq!(result, " PR#42");
    }

    #[test]
    fn test_format_suffix_completed_without_pr_url_returns_empty() {
        // Given: Completed フェーズ、PR URL なし
        let s = make_session("20260306143000", "add feature", SessionPhase::Completed);

        // When
        let result = format_suffix(&s);

        // Then: 空文字
        assert_eq!(result, "");
    }

    #[test]
    fn test_format_suffix_planned_returns_empty() {
        // Given: Planned フェーズ
        let s = make_session("20260306143000", "add feature", SessionPhase::Planned);

        // When
        let result = format_suffix(&s);

        // Then: 空文字
        assert_eq!(result, "");
    }

    #[test]
    fn test_format_suffix_failed_returns_empty() {
        // Given: Failed フェーズ
        let s = make_session(
            "20260306143000",
            "add feature",
            SessionPhase::Failed("timeout".to_string()),
        );

        // When
        let result = format_suffix(&s);

        // Then: 空文字
        assert_eq!(result, "");
    }

    // -----------------------------------------------------------------------
    // format_session_label (新フォーマットへの期待値)
    // -----------------------------------------------------------------------

    /// ANSI エスケープを除去してラベル内容を検証するヘルパー。
    fn strip(s: &str) -> String {
        console::strip_ansi_codes(s).to_string()
    }

    #[test]
    fn test_format_session_label_planned_contains_icon_date_phase_input() {
        // Given: Planned セッション
        let s = make_session(
            "20260306143000",
            "add hello world feature",
            SessionPhase::Planned,
        );

        // When
        let label = strip(&format_session_label(&s));

        // Then: アイコン・日時・フェーズ・input を含む
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
        // Given: Running フェーズ、current_step あり
        let mut s = make_session("20260307150000", "implement auth", SessionPhase::Running);
        s.current_step = Some("test".to_string());

        // When
        let label = strip(&format_session_label(&s));

        // Then: ▶ アイコンとステップ情報を含む
        assert!(label.contains('▶'), "should contain ▶ icon: {label}");
        assert!(label.contains("Running"), "should contain Running: {label}");
        assert!(label.contains("[test]"), "should contain step: {label}");
    }

    #[test]
    fn test_format_session_label_completed_with_pr_contains_checkmark_and_pr() {
        // Given: Completed フェーズ、PR URL あり
        let mut s = make_session("20260307090000", "refactor db", SessionPhase::Completed);
        s.pr_url = Some("https://github.com/owner/repo/pull/42".to_string());

        // When
        let label = strip(&format_session_label(&s));

        // Then: ✓ アイコンと PR 番号を含む
        assert!(label.contains('✓'), "should contain ✓ icon: {label}");
        assert!(
            label.contains("Completed"),
            "should contain Completed: {label}"
        );
        assert!(label.contains("PR#42"), "should contain PR#42: {label}");
    }

    #[test]
    fn test_format_session_label_failed_contains_cross_icon() {
        // Given: Failed フェーズ
        let s = make_session(
            "20260307103000",
            "fix login bug",
            SessionPhase::Failed("exit 1".to_string()),
        );

        // When
        let label = strip(&format_session_label(&s));

        // Then: ✗ アイコンを含む
        assert!(label.contains('✗'), "should contain ✗ icon: {label}");
        assert!(label.contains("Failed"), "should contain Failed: {label}");
    }

    #[test]
    fn test_format_session_label_long_input_is_truncated() {
        // Given: 非常に長い input
        let long_input = "a".repeat(200);
        let s = make_session("20260306143000", &long_input, SessionPhase::Planned);

        // When
        let label = strip(&format_session_label(&s));

        // Then: 省略記号 "…" が含まれ、ラベル全体は 200 文字以下に収まる
        assert!(
            label.contains('…'),
            "long input should be truncated: {label}"
        );
    }
}
