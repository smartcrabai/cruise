use crate::application::OperationKind;
use crate::webui::dto::SessionDto;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent piece of view state"
)]
pub(crate) struct SessionActionsVm {
    pub show_approve: bool,
    pub show_publish_issue: bool,
    pub show_fix: bool,
    pub show_ask: bool,
    pub show_create_worktree: bool,
    pub show_run: bool,
    pub run_label: String,
    pub show_reset: bool,
    pub show_replan: bool,
    pub show_discard: bool,
    pub show_delete: bool,
    pub show_cancel: bool,
    pub show_generate_plan: bool,
    pub status: String,
}

pub(crate) fn session_actions(
    session: &SessionDto,
    active_operation: Option<crate::application::OperationKind>,
    has_pending_prompt: bool,
) -> SessionActionsVm {
    let is_locally_running = matches!(
        active_operation,
        Some(OperationKind::Run | OperationKind::BatchRun | OperationKind::BatchQueued)
    );
    let is_fixing = matches!(
        active_operation,
        Some(
            OperationKind::Fix
                | OperationKind::Ask
                | OperationKind::Replan
                | OperationKind::Generate
        )
    );
    let status = if is_locally_running {
        "running"
    } else {
        "idle"
    };
    let is_phase_running = session.phase == "Running";
    let is_awaiting_refresh = false;
    let is_active_run = is_locally_running
        || has_pending_prompt
        || session.fix_in_progress
        || is_fixing
        || (is_phase_running && !is_awaiting_refresh);
    let has_plan_error = session.plan_error.is_some();
    let awaiting_approval_with_plan = session.phase == "Awaiting Approval"
        && !is_locally_running
        && !is_fixing
        && !session.fix_in_progress
        && session.plan_available;
    let usable_plan = session.plan_available && !has_plan_error;
    let show_approve = awaiting_approval_with_plan && !has_plan_error;
    let show_publish_issue = (session.phase == "Awaiting Approval" || session.phase == "Planned")
        && usable_plan
        && !is_locally_running
        && !is_active_run;
    let show_fix = awaiting_approval_with_plan;
    let show_ask = awaiting_approval_with_plan && !has_plan_error;
    let show_create_worktree = !is_active_run && session.phase == "Planned" && usable_plan;
    let show_run = !is_active_run
        && !is_awaiting_refresh
        && usable_plan
        && (session.phase == "Suspended" || session.phase == "Failed");
    let run_label = if session.phase == "Failed" {
        "Retry"
    } else {
        "Resume"
    };
    let show_reset = !is_active_run
        && !is_awaiting_refresh
        && (session.phase == "Suspended"
            || session.phase == "Failed"
            || session.phase == "Completed");
    let show_replan = !is_active_run && session.phase == "Planned";
    let show_discard = !is_locally_running && session.phase == "Awaiting Approval";
    let show_delete =
        !is_locally_running && session.phase != "Running" && session.phase != "Awaiting Approval";
    let show_cancel = is_active_run;
    let show_generate_plan = !is_locally_running
        && !is_fixing
        && !session.fix_in_progress
        && (session.phase == "Draft"
            || session.phase == "Awaiting Input"
            || (session.phase == "Awaiting Approval" && has_plan_error));

    SessionActionsVm {
        show_approve,
        show_publish_issue,
        show_fix,
        show_ask,
        show_create_worktree,
        show_run,
        run_label: run_label.to_string(),
        show_reset,
        show_replan,
        show_discard,
        show_delete,
        show_cancel,
        show_generate_plan,
        status: status.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::WorkspaceMode;

    fn make_session(phase: &str) -> SessionDto {
        SessionDto {
            id: "session-1".to_string(),
            phase: phase.to_string(),
            phase_error: None,
            config_source: "default.yaml".to_string(),
            config_path: Some("default.yaml".to_string()),
            base_dir: "/home/user/project".to_string(),
            repo: None,
            input: "test task".to_string(),
            title: None,
            current_step: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            completed_at: None,
            worktree_branch: None,
            workspace_mode: WorkspaceMode::Worktree,
            pr_url: None,
            updated_at: None,
            awaiting_input: false,
            pending_ask_question: None,
            plan_error: None,
            exec: false,
            plan_available: false,
            fix_in_progress: false,
            skipped_steps: vec![],
        }
    }

    fn actions(
        phase: &str,
        plan_available: bool,
        operation: Option<OperationKind>,
        pending_prompt: bool,
    ) -> SessionActionsVm {
        let mut session = make_session(phase);
        session.plan_available = plan_available;
        session_actions(&session, operation, pending_prompt)
    }

    #[test]
    fn awaiting_input_hides_review_and_execution_but_allows_delete() {
        let value = actions("Awaiting Input", false, None, false);
        assert!(!value.show_approve);
        assert!(!value.show_fix);
        assert!(!value.show_ask);
        assert!(!value.show_create_worktree);
        assert!(!value.show_run);
        assert!(!value.show_reset);
        assert!(value.show_delete);
        assert!(!value.show_cancel);
    }

    #[test]
    fn running_phase_is_cancelable_even_after_local_state_reset() {
        let idle = actions("Running", false, None, false);
        assert!(!idle.show_create_worktree);
        assert!(idle.show_cancel);
        assert!(!idle.show_run);
        assert!(!idle.show_delete);

        let local = actions("Running", false, Some(OperationKind::Run), false);
        assert!(local.show_cancel);
    }

    #[test]
    fn awaiting_approval_requires_a_plan_for_review_actions() {
        let ready = actions("Awaiting Approval", true, None, false);
        assert!(ready.show_approve);
        assert!(ready.show_fix);
        assert!(ready.show_ask);
        assert!(ready.show_publish_issue);
        assert!(!ready.show_create_worktree);
        assert!(!ready.show_run);
        assert!(ready.show_discard);
        assert!(!ready.show_delete);

        let missing = actions("Awaiting Approval", false, None, false);
        assert!(!missing.show_approve);
        assert!(!missing.show_fix);
        assert!(!missing.show_ask);
        assert!(!missing.show_publish_issue);
        assert!(missing.show_discard);
    }

    #[test]
    fn plan_error_blocks_approval_and_publish_but_allows_fix_and_retry_planning() {
        let mut approval = make_session("Awaiting Approval");
        approval.plan_available = true;
        approval.plan_error = Some("provider unavailable".to_string());
        let approval = session_actions(&approval, None, false);
        assert!(!approval.show_approve);
        assert!(!approval.show_publish_issue);
        assert!(!approval.show_ask);
        assert!(approval.show_fix);
        assert!(approval.show_generate_plan);

        let mut planned = make_session("Planned");
        planned.plan_available = true;
        planned.plan_error = Some("provider unavailable".to_string());
        let planned = session_actions(&planned, None, false);
        assert!(!planned.show_publish_issue);
        assert!(!planned.show_run);
        assert!(planned.show_replan);
    }

    #[test]
    fn planned_sessions_show_workspace_and_replan_not_review_or_resume() {
        let ready = actions("Planned", true, None, false);
        assert!(ready.show_create_worktree);
        assert!(ready.show_replan);
        assert!(ready.show_publish_issue);
        assert!(!ready.show_fix);
        assert!(!ready.show_ask);
        assert!(!ready.show_approve);
        assert!(!ready.show_run);
        assert!(ready.show_delete);

        let missing = actions("Planned", false, None, false);
        assert!(!missing.show_create_worktree);
        assert!(missing.show_replan);
        assert!(!missing.show_publish_issue);
    }

    #[test]
    fn suspended_and_failed_sessions_show_resume_or_retry_and_reset() {
        let suspended = actions("Suspended", true, None, false);
        assert!(suspended.show_run);
        assert_eq!(suspended.run_label, "Resume");
        assert!(!suspended.show_create_worktree);
        assert!(suspended.show_reset);

        let failed = actions("Failed", true, None, false);
        assert!(failed.show_run);
        assert_eq!(failed.run_label, "Retry");
        assert!(!failed.show_create_worktree);
        assert!(failed.show_reset);
    }

    #[test]
    fn completed_sessions_only_show_reset_and_delete() {
        let value = actions("Completed", false, None, false);
        assert!(value.show_reset);
        assert!(value.show_delete);
        assert!(!value.show_create_worktree);
        assert!(!value.show_run);
    }

    #[test]
    fn publish_is_limited_to_approval_or_planned_with_a_usable_plan() {
        for phase in [
            "Draft",
            "Awaiting Input",
            "Running",
            "Suspended",
            "Failed",
            "Completed",
        ] {
            assert!(!actions(phase, true, None, false).show_publish_issue);
        }
        assert!(!actions("Awaiting Approval", false, None, false).show_publish_issue);
        assert!(actions("Awaiting Approval", true, None, false).show_publish_issue);
        assert!(actions("Planned", true, None, false).show_publish_issue);
        assert!(!actions("Planned", true, Some(OperationKind::Run), false).show_publish_issue);
    }

    #[test]
    fn delete_and_discard_follow_phase_matrix() {
        for phase in ["Planned", "Suspended", "Failed", "Completed"] {
            assert!(actions(phase, false, None, false).show_delete);
        }
        assert!(!actions("Running", false, None, false).show_delete);
        assert!(!actions("Awaiting Approval", false, None, false).show_delete);
        assert!(actions("Awaiting Approval", false, None, false).show_discard);
        for phase in [
            "Draft",
            "Awaiting Input",
            "Planned",
            "Running",
            "Suspended",
            "Failed",
            "Completed",
        ] {
            assert!(!actions(phase, false, None, false).show_discard);
        }
    }

    #[test]
    fn local_run_suppresses_everything_except_cancel() {
        let value = actions("Planned", false, Some(OperationKind::Run), false);
        assert!(value.show_cancel);
        assert!(!value.show_approve);
        assert!(!value.show_fix);
        assert!(!value.show_ask);
        assert!(!value.show_create_worktree);
        assert!(!value.show_run);
        assert!(!value.show_reset);
        assert!(!value.show_replan);
        assert!(!value.show_delete);
        assert_eq!(value.status, "running");
    }

    #[test]
    fn pending_prompts_and_fix_operations_are_active() {
        let pending = actions("Planned", true, None, true);
        assert!(pending.show_cancel);
        assert!(!pending.show_create_worktree);

        for operation in [
            OperationKind::Fix,
            OperationKind::Ask,
            OperationKind::Replan,
            OperationKind::Generate,
        ] {
            let value = actions("Awaiting Approval", true, Some(operation), false);
            assert!(value.show_cancel);
            assert!(!value.show_approve);
            assert!(!value.show_fix);
            assert!(!value.show_ask);
            assert_eq!(value.status, "idle");
        }
    }

    #[test]
    fn fixing_flag_suppresses_review_and_generation_actions() {
        let mut approval = make_session("Awaiting Approval");
        approval.plan_available = true;
        approval.fix_in_progress = true;
        let value = session_actions(&approval, None, false);
        assert!(!value.show_approve);
        assert!(!value.show_fix);
        assert!(!value.show_ask);
        assert!(value.show_discard);

        let mut draft = make_session("Draft");
        draft.fix_in_progress = true;
        assert!(!session_actions(&draft, None, false).show_generate_plan);
    }

    #[test]
    fn draft_sessions_only_offer_generation_and_delete() {
        let value = actions("Draft", false, None, false);
        assert!(value.show_generate_plan);
        assert!(value.show_delete);
        assert!(!value.show_approve);
        assert!(!value.show_fix);
        assert!(!value.show_ask);
        assert!(!value.show_create_worktree);
        assert!(!value.show_run);
        assert!(!value.show_reset);
        assert!(!value.show_replan);
        assert!(!value.show_cancel);

        assert!(!actions("Draft", false, Some(OperationKind::Run), false).show_generate_plan);
        assert!(!actions("Draft", false, Some(OperationKind::Generate), false).show_generate_plan);
    }

    #[test]
    fn serializes_all_fields_in_camel_case() {
        let Ok(value) = serde_json::to_value(actions("Draft", false, None, false)) else {
            panic!("actions serialize");
        };
        for key in [
            "showApprove",
            "showPublishIssue",
            "showFix",
            "showAsk",
            "showCreateWorktree",
            "showRun",
            "runLabel",
            "showReset",
            "showReplan",
            "showDiscard",
            "showDelete",
            "showCancel",
            "showGeneratePlan",
            "status",
        ] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
    }
}
