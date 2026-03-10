use console::style;
use inquire::InquireError;

use crate::cli::{DEFAULT_MAX_RETRIES, DEFAULT_RATE_LIMIT_RETRIES};
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
        let mut session = sessions[idx].clone();

        loop {
            // Show plan.md content.
            let plan_path = session.plan_path(&manager.sessions_dir());
            if let Ok(content) = std::fs::read_to_string(&plan_path) {
                crate::display::print_bordered(&content, Some("plan.md"));
            }

            // Action menu.
            let actions = session_actions(&session.phase);

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
                        max_retries: DEFAULT_MAX_RETRIES,
                        rate_limit_retries: DEFAULT_RATE_LIMIT_RETRIES,
                        dry_run: false,
                    };
                    return crate::run_cmd::run(run_args).await;
                }
                "Replan" => {
                    let text = match inquire::Text::new("Describe the changes needed:").prompt() {
                        Ok(t) => t,
                        Err(
                            InquireError::OperationCanceled | InquireError::OperationInterrupted,
                        ) => {
                            continue;
                        }
                        Err(e) => return Err(CruiseError::Other(format!("input error: {e}"))),
                    };
                    crate::plan_cmd::replan_session(
                        &manager,
                        &session,
                        text,
                        DEFAULT_RATE_LIMIT_RETRIES,
                    )
                    .await?;
                    // Re-load so subsequent session_actions(&session.phase) uses fresh state.
                    session = manager.load(&session.id)?;
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

/// Returns the action menu items available for the given session phase.
/// "Run"/"Resume" appears for runnable phases; "Replan" only for Planned.
/// "Reset to Planned" appears for Running, Failed, and Completed.
/// "Delete" and "Back" are always present (in that order) at the end.
fn session_actions(phase: &SessionPhase) -> Vec<&'static str> {
    let mut actions = vec![];
    match phase {
        SessionPhase::Planned => {
            actions.push("Run");
            actions.push("Replan");
        }
        SessionPhase::Running => {
            actions.push("Resume");
            actions.push("Reset to Planned");
        }
        SessionPhase::Failed(_) => {
            actions.push("Run");
            actions.push("Reset to Planned");
        }
        SessionPhase::Completed => {
            actions.push("Reset to Planned");
        }
    }
    actions.push("Delete");
    actions.push("Back");
    actions
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

    // -----------------------------------------------------------------------
    // session_actions
    // -----------------------------------------------------------------------

    #[test]
    fn test_session_actions_planned_has_run_and_replan() {
        // Given: Planned フェーズ
        let phase = SessionPhase::Planned;

        // When
        let actions = session_actions(&phase);

        // Then: "Run" と "Replan" が含まれ、"Delete" と "Back" も含まれる
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
        // Given: Planned フェーズ
        let phase = SessionPhase::Planned;

        // When
        let actions = session_actions(&phase);

        // Then: "Resume" は含まれない（未着手なので Resume ではなく Run）
        assert!(
            !actions.contains(&"Resume"),
            "Planned should NOT have Resume: {actions:?}"
        );
    }

    #[test]
    fn test_session_actions_running_has_resume_not_replan() {
        // Given: Running フェーズ
        let phase = SessionPhase::Running;

        // When
        let actions = session_actions(&phase);

        // Then: "Resume" は含まれるが "Replan" は含まれない
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
        // Given: Failed フェーズ
        let phase = SessionPhase::Failed("some error".to_string());

        // When
        let actions = session_actions(&phase);

        // Then: "Run" は含まれるが "Replan" は含まれない
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
        // Given: Completed フェーズ
        let phase = SessionPhase::Completed;

        // When
        let actions = session_actions(&phase);

        // Then: "Run" も "Resume" も "Replan" も含まれないが "Reset to Planned" は含まれる
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
        // Given: Planned フェーズ
        let phase = SessionPhase::Planned;

        // When
        let actions = session_actions(&phase);

        // Then: "Run" は "Replan" より前に位置する（主要アクションが先頭）
        let run_pos = actions.iter().position(|&a| a == "Run").unwrap();
        let replan_pos = actions.iter().position(|&a| a == "Replan").unwrap();
        assert!(
            run_pos < replan_pos,
            "Run should come before Replan in actions list"
        );
    }

    #[test]
    fn test_session_actions_delete_and_back_always_at_end() {
        // Given: すべてのフェーズで Delete と Back が末尾 2 つに並ぶ
        let phases = [
            SessionPhase::Planned,
            SessionPhase::Running,
            SessionPhase::Completed,
            SessionPhase::Failed("err".to_string()),
        ];

        for phase in &phases {
            // When
            let actions = session_actions(phase);
            let len = actions.len();

            // Then: 末尾が Back、その前が Delete
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

    // -----------------------------------------------------------------------
    // session_actions — Reset to Planned coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_session_actions_planned_exact() {
        assert_eq!(
            session_actions(&SessionPhase::Planned),
            vec!["Run", "Replan", "Delete", "Back"]
        );
    }

    #[test]
    fn test_session_actions_running_has_reset_to_planned() {
        let actions = session_actions(&SessionPhase::Running);
        assert_eq!(
            actions,
            vec!["Resume", "Reset to Planned", "Delete", "Back"]
        );
    }

    #[test]
    fn test_session_actions_completed_has_reset_to_planned() {
        assert_eq!(
            session_actions(&SessionPhase::Completed),
            vec!["Reset to Planned", "Delete", "Back"]
        );
    }

    #[test]
    fn test_session_actions_failed_has_run_and_reset_to_planned() {
        assert_eq!(
            session_actions(&SessionPhase::Failed("exit 1".to_string())),
            vec!["Run", "Reset to Planned", "Delete", "Back"]
        );
    }
}
